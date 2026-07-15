//! GalliumProvider: wraps a local gallium-core CausalLM as an LlmProvider.
//!
//! Prompt formatting and response parsing are delegated to a [`ModelProtocol`]
//! adapter. See [`protocol`] for available protocols:
//!
//! - [`HarmonyProtocol`] — GPT-OSS: full ReAct with tool calling via Harmony format
//! - [`GemmaProtocol`] — Gemma 4: native function-calling + optional thinking
//! - [`QwenProtocol`]   — Qwen 3.5: ChatML template, plain chat
//!
//! ## Generation and decoding
//!
//! `run_generate_ids` runs the model and returns raw token IDs. All paths decode
//! with `skip_special=false` so that `parse_response` and `parse_tool_call` have
//! access to special-token markers (e.g. `<channel|>` for Gemma thinking,
//! `<|channel|>final` for Harmony channels).
//!
//! ## Tool calling
//!
//! When `protocol.supports_tools()` is true, `chat_with_tools()`:
//!
//! 1. Formats the prompt via `protocol.format_prompt_with_tools()`.
//! 2. Runs generation; `protocol.tool_stop_tokens()` are added to the EOS set so
//!    generation stops as soon as the model signals a tool call.
//! 3. Decodes with skip_special=false and calls `protocol.parse_tool_call()`.
//!    If a tool call is detected, returns `LlmResponse::ToolCalls`.
//! 4. Otherwise extracts the response text via `protocol.parse_response()`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use gallium_core::{generate, CausalLM, SamplingParams};
use tokenizers::Tokenizer;

use crate::llm::{ChatMessage, LlmProvider, LlmResponse, ToolCallInfo, ToolDefinition};
use crate::protocol::{GemmaProtocol, HarmonyProtocol, ModelProtocol, QwenProtocol};

pub struct GalliumProvider {
    model: RefCell<Box<dyn CausalLM>>,
    tokenizer: Tokenizer,
    params: SamplingParams,
    /// EOS token IDs (includes <|end|>, </s>, <|call|>, model-specific terminators).
    eos_tokens: Vec<u32>,
    max_new_tokens: usize,
    protocol: Box<dyn ModelProtocol>,
}

// GalliumProvider is used only from single-threaded binary context (REPL) or
// under a Mutex (UniFFI). The RefCell is never accessed from multiple threads concurrently.
unsafe impl Send for GalliumProvider {}
unsafe impl Sync for GalliumProvider {}

impl GalliumProvider {
    pub fn new(
        model: Box<dyn CausalLM>,
        tokenizer: Tokenizer,
        params: SamplingParams,
        max_new_tokens: usize,
        protocol: Box<dyn ModelProtocol>,
    ) -> Self {
        let tool_stops = protocol.tool_stop_tokens();
        // Use get_vocab(true) — includes both the base BPE vocabulary AND added tokens.
        // get_added_vocabulary().get_vocab() misses tokens like <|im_end|> that appear
        // in the base BPE vocab for some models (e.g. Qwen3.5) rather than the added layer.
        let eos_tokens: Vec<u32> = tokenizer
            .get_vocab(true)
            .into_iter()
            .filter(|(k, _)| {
                // NOTE: do NOT match the bare "<|end|>" token — in Harmony it's a
                // message/channel separator (analysis → commentary → final), not a
                // turn terminator. The turn ends on "<|return|>" or "<|call|>".
                k.contains("eos")
                    || k == "<|endoftext|>"
                    || k.contains("</s>")
                    || k.contains("<end_of_turn>")
                    || k.contains("<|im_end|>")
                    || k == "<|call|>"              // Harmony tool call terminator
                    || k == "<|return|>"            // Harmony end-of-turn terminator
                    || tool_stops.contains(&k.as_str()) // protocol-specific tool stops
            })
            .map(|(_, v)| v)
            .collect();

        tracing::info!(
            "GalliumProvider: {} EOS tokens, max_new_tokens={}",
            eos_tokens.len(),
            max_new_tokens
        );

        Self {
            model: RefCell::new(model),
            tokenizer,
            params,
            eos_tokens,
            max_new_tokens,
            protocol,
        }
    }

    /// Encode `prompt`, run generation, return the raw generated token IDs.
    fn run_generate_ids(&self, prompt: &str) -> Result<Vec<u32>> {
        let encoding = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| anyhow::anyhow!("tokenization error: {e}"))?;
        let prompt_tokens: Vec<u32> = encoding.get_ids().to_vec();
        tracing::info!("GalliumProvider: prompt_tokens={}", prompt_tokens.len());

        let mut generated_ids: Vec<u32> = Vec::new();
        let mut model = self.model.borrow_mut();
        generate(
            model.as_mut(),
            &prompt_tokens,
            &self.params,
            self.max_new_tokens,
            &self.eos_tokens,
            |id| generated_ids.push(id),
        )
        .map_err(|e| anyhow::anyhow!("generate error: {e}"))?;

        tracing::info!("GalliumProvider: generated {} tokens", generated_ids.len());
        Ok(generated_ids)
    }

    /// Convenience: generate and decode with skip_special=false (for parse_response / parse_tool_call).
    fn run_generate(&self, prompt: &str) -> Result<String> {
        let ids = self.run_generate_ids(prompt)?;
        let raw = self
            .tokenizer
            .decode(&ids, false)
            .map_err(|e| anyhow::anyhow!("decode error: {e}"))?;
        tracing::debug!("GalliumProvider raw output: {:?}", raw);
        Ok(raw)
    }
}

impl LlmProvider for GalliumProvider {
    fn chat(&self, messages: &[ChatMessage]) -> Result<String> {
        let prompt = self.protocol.format_prompt(messages);
        tracing::debug!("GalliumProvider prompt ({} chars)", prompt.len());
        let raw = self.run_generate(&prompt)?;
        Ok(self.protocol.parse_response(&raw))
    }

    fn supports_tools(&self) -> bool {
        self.protocol.supports_tools()
    }

    fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let prompt = self.protocol.format_prompt_with_tools(messages, tools);
        tracing::debug!("GalliumProvider tool prompt ({} chars)", prompt.len());
        // Decode with skip_special=false so parse_tool_call can see all markers.
        let raw = self.run_generate(&prompt)?;

        if let Some((func_name, args)) = self.protocol.parse_tool_call(&raw) {
            tracing::info!("GalliumProvider: tool call '{}'", func_name);
            let call_id = format!(
                "call_{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_nanos()
            );
            return Ok(LlmResponse::ToolCalls(
                vec![ToolCallInfo { id: call_id, name: func_name, arguments: args }],
                None,
            ));
        }

        // No tool call — extract response text.
        Ok(LlmResponse::Text {
            content: self.protocol.parse_response(&raw),
            reasoning: None,
            usage: None,
        })
    }
}

// ============================================================================
// Loader — construct a GalliumProvider from a `gallium:` model-path spec
// ============================================================================

/// Which hand-written model implementation to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arch {
    GptOss,
    Qwen35,
    Gemma4,
}

impl Arch {
    fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "gptoss" => Ok(Arch::GptOss),
            "qwen35" | "qwen3" => Ok(Arch::Qwen35),
            "gemma4" => Ok(Arch::Gemma4),
            other => anyhow::bail!(
                "unknown gallium arch '{other}' (expected: gpt-oss, qwen35, gemma4)"
            ),
        }
    }

    fn protocol(self) -> Box<dyn ModelProtocol> {
        match self {
            Arch::GptOss => Box::new(HarmonyProtocol),
            Arch::Qwen35 => Box::new(QwenProtocol),
            Arch::Gemma4 => {
                // Gemma 4 supports an optional thinking channel.
                if env_flag("KESSEL_GALLIUM_THINKING") {
                    Box::new(GemmaProtocol::with_thinking())
                } else {
                    Box::new(GemmaProtocol::new())
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Safetensors,
    Gguf,
}

impl Format {
    fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "safetensors" | "st" => Ok(Format::Safetensors),
            "gguf" | "q" => Ok(Format::Gguf),
            other => anyhow::bail!(
                "unknown gallium format '{other}' (expected: safetensors, gguf)"
            ),
        }
    }
}

/// Does a model-path select the native gallium backend?
pub fn is_gallium_spec(path: &str) -> bool {
    path.starts_with("gallium:")
}

/// Build a [`GalliumProvider`] from a `gallium:<arch>:<format>:<source>` spec.
///
/// `<source>` is either an `hf:` HuggingFace spec or a local filesystem path:
/// - **gguf** — `hf:ORG/REPO/path/to/file.gguf`, or a local `.gguf` file.
/// - **safetensors** — `hf:ORG/REPO` (a repo of shards), or a local directory.
///
/// Extra knobs come from the environment: `KESSEL_GALLIUM_TOKENIZER_REPO`
/// (tokenizer.json source repo), `KESSEL_GALLIUM_DTYPE` (`f16`/`bf16`/`f32`,
/// safetensors only, default `f16`), `KESSEL_GALLIUM_THINKING` (Gemma 4).
pub fn load_gallium_provider(
    spec: &str,
    temperature: Option<f32>,
    max_tokens: u32,
) -> Result<GalliumProvider> {
    use candle_core::{DType, Device};

    // gallium:<arch>:<format>:<source> — splitn(4) keeps the source intact even
    // though it may itself contain ':' (e.g. `hf:...` or a Windows `C:\` path).
    let rest = spec
        .strip_prefix("gallium:")
        .ok_or_else(|| anyhow::anyhow!("not a gallium spec: {spec}"))?;
    let parts: Vec<&str> = rest.splitn(3, ':').collect();
    if parts.len() != 3 {
        anyhow::bail!(
            "malformed gallium spec '{spec}' \
             (expected gallium:<arch>:<format>:<source>)"
        );
    }
    let arch = Arch::parse(parts[0])?;
    let format = Format::parse(parts[1])?;
    let source = parts[2];

    let tok_repo = std::env::var("KESSEL_GALLIUM_TOKENIZER_REPO").ok();
    let model_path = resolve_source(source, format, tok_repo.as_deref())?;

    let device = Device::Cpu;
    let params = SamplingParams {
        temperature: temperature.unwrap_or(0.7),
        ..Default::default()
    };

    let (model, tokenizer): (Box<dyn CausalLM>, Tokenizer) = match format {
        Format::Gguf => {
            tracing::info!("Loading GGUF gallium model from {:?}", model_path);
            let (metadata, vb) = gallium_core::load_gguf(&model_path, &device)?;
            let dir = model_path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let tokenizer = load_tokenizer(&dir.join("tokenizer.json"))?;
            let model: Box<dyn CausalLM> = match arch {
                Arch::GptOss => Box::new(gallium_models::gpt_oss_q::GptOssQ::load(&metadata, &vb, &device)?),
                Arch::Qwen35 => Box::new(gallium_models::qwen35_q::Qwen35Q::load(&metadata, &vb, &device)?),
                Arch::Gemma4 => Box::new(gallium_models::gemma4_q::Gemma4Q::load(&metadata, &vb, &device)?),
            };
            (model, tokenizer)
        }
        Format::Safetensors => {
            let dtype = match std::env::var("KESSEL_GALLIUM_DTYPE")
                .unwrap_or_else(|_| "f16".to_string())
                .as_str()
            {
                "f32" => DType::F32,
                "f16" => DType::F16,
                "bf16" => DType::BF16,
                other => anyhow::bail!("unsupported KESSEL_GALLIUM_DTYPE '{other}'"),
            };
            tracing::info!("Loading safetensors gallium model from {:?}", model_path);
            let shards: Vec<PathBuf> = std::fs::read_dir(&model_path)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|ext| ext == "safetensors").unwrap_or(false))
                .collect();
            if shards.is_empty() {
                anyhow::bail!("no .safetensors files in {:?}", model_path);
            }
            let config_path = model_path.join("config.json");
            let vb = gallium_models::loader::load_safetensors(&shards, dtype, &device)?;
            let tokenizer = load_tokenizer(&model_path.join("tokenizer.json"))?;
            let model: Box<dyn CausalLM> = match arch {
                Arch::GptOss => {
                    let cfg: gallium_models::gpt_oss::GptOssConfig =
                        gallium_models::loader::load_config(&config_path)?;
                    Box::new(gallium_models::gpt_oss::GptOss::load(&cfg, vb, &shards, &device)?)
                }
                Arch::Qwen35 => {
                    let full: serde_json::Value = gallium_models::loader::load_config(&config_path)?;
                    let text = full.get("text_config").unwrap_or(&full);
                    let cfg: gallium_models::qwen35::Qwen35Config = serde_json::from_value(text.clone())
                        .map_err(|e| anyhow::anyhow!("Qwen35 config error: {e}"))?;
                    Box::new(gallium_models::qwen35::Qwen35::load(&cfg, vb, &device)?)
                }
                Arch::Gemma4 => {
                    let full: serde_json::Value = gallium_models::loader::load_config(&config_path)?;
                    let text = full.get("text_config").unwrap_or(&full);
                    let cfg: gallium_models::gemma4::Gemma4Config = serde_json::from_value(text.clone())
                        .map_err(|e| anyhow::anyhow!("Gemma4 config error: {e}"))?;
                    Box::new(gallium_models::gemma4::Gemma4::load(&cfg, vb, &device)?)
                }
            };
            (model, tokenizer)
        }
    };

    tracing::info!("Gallium model loaded.");
    Ok(GalliumProvider::new(
        model,
        tokenizer,
        params,
        max_tokens as usize,
        arch.protocol(),
    ))
}

fn load_tokenizer(path: &std::path::Path) -> Result<Tokenizer> {
    Tokenizer::from_file(path)
        .map_err(|e| anyhow::anyhow!("failed to load tokenizer from {:?}: {e}", path))
}

/// Resolve `<source>` to a local model path, downloading from HuggingFace if the
/// source is an `hf:` spec (otherwise it is treated as a local path as-is).
fn resolve_source(source: &str, format: Format, tok_repo: Option<&str>) -> Result<PathBuf> {
    let Some(hf) = source.strip_prefix("hf:") else {
        return Ok(PathBuf::from(source));
    };
    download_from_hub(hf, format, tok_repo)
}

/// Download a model from the HuggingFace hub into the shared HF cache.
///
/// `hf` is `ORG/REPO[/path/to/file.gguf]`. For gguf the trailing path component
/// past `ORG/REPO` is the file to fetch; for safetensors the whole repo of
/// shards (plus config.json/tokenizer.json) is fetched and its dir returned.
fn download_from_hub(hf: &str, format: Format, tok_repo: Option<&str>) -> Result<PathBuf> {
    use hf_hub::api::sync::Api;

    let api = Api::new()?;
    match format {
        Format::Safetensors => {
            let repo_id = hf;
            tracing::info!("Fetching safetensors repo from HuggingFace: {repo_id}");
            let repo = api.model(repo_id.to_string());
            let info = repo.info()?;
            let shards: Vec<String> = info
                .siblings
                .iter()
                .map(|s| s.rfilename.clone())
                .filter(|name| name.ends_with(".safetensors"))
                .collect();
            if shards.is_empty() {
                anyhow::bail!("no .safetensors files found in {repo_id}");
            }
            let config_local = repo.get("config.json")?;
            api.model(tok_repo.unwrap_or(repo_id).to_string()).get("tokenizer.json")?;
            for shard in &shards {
                repo.get(shard)?;
            }
            Ok(config_local.parent().unwrap().to_path_buf())
        }
        Format::Gguf => {
            // Split ORG/REPO/<file...>: the repo id is the first two path
            // segments, the remainder (may contain '/') is the gguf filename.
            let mut segs = hf.splitn(3, '/');
            let org = segs.next().unwrap_or_default();
            let name = segs
                .next()
                .ok_or_else(|| anyhow::anyhow!("gguf hf spec needs ORG/REPO/FILE: {hf}"))?;
            let filename = segs
                .next()
                .ok_or_else(|| anyhow::anyhow!("gguf hf spec needs a file: {hf}"))?;
            let repo_id = format!("{org}/{name}");
            tracing::info!("Fetching {filename} from HuggingFace: {repo_id}");
            let repo = api.model(repo_id.clone());
            let tok_local = api
                .model(tok_repo.unwrap_or(&repo_id).to_string())
                .get("tokenizer.json")?;
            let gguf_local = repo.get(filename)?;
            let gguf_dir = gguf_local.parent().unwrap();
            let tok_dest = gguf_dir.join("tokenizer.json");
            if !tok_dest.exists() {
                std::fs::copy(&tok_local, &tok_dest)?;
            }
            Ok(gguf_local)
        }
    }
}

fn env_flag(key: &str) -> bool {
    matches!(
        std::env::var(key).ok().as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_gallium_spec_matches_only_prefix() {
        assert!(is_gallium_spec("gallium:qwen35:gguf:hf:a/b/c.gguf"));
        assert!(!is_gallium_spec("hf:unsloth/Qwen3.5-9B-GGUF/x.gguf"));
        assert!(!is_gallium_spec("/models/x.gguf"));
    }

    #[test]
    fn arch_parse_accepts_aliases_and_rejects_junk() {
        assert_eq!(Arch::parse("gpt-oss").unwrap(), Arch::GptOss);
        assert_eq!(Arch::parse("gpt_oss").unwrap(), Arch::GptOss);
        assert_eq!(Arch::parse("qwen35").unwrap(), Arch::Qwen35);
        assert_eq!(Arch::parse("Gemma4").unwrap(), Arch::Gemma4);
        assert!(Arch::parse("llama").is_err());
    }

    #[test]
    fn format_parse() {
        assert_eq!(Format::parse("gguf").unwrap(), Format::Gguf);
        assert_eq!(Format::parse("safetensors").unwrap(), Format::Safetensors);
        assert!(Format::parse("onnx").is_err());
    }

    #[test]
    fn malformed_spec_is_rejected() {
        // Missing the <source> field.
        let msg = match load_gallium_provider("gallium:qwen35:gguf", None, 128) {
            Ok(_) => panic!("expected malformed-spec error"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains("malformed gallium spec"), "got: {msg}");
    }

    #[test]
    fn local_source_passes_through_untouched() {
        // hf-less source is treated as a local path; a Windows drive letter's
        // colon must survive splitn(3).
        assert_eq!(
            resolve_source("/models/qwen.gguf", Format::Gguf, None).unwrap(),
            PathBuf::from("/models/qwen.gguf"),
        );
        let spec = "gallium:qwen35:gguf:C:\\models\\qwen.gguf";
        let rest = spec.strip_prefix("gallium:").unwrap();
        let parts: Vec<&str> = rest.splitn(3, ':').collect();
        assert_eq!(parts[2], "C:\\models\\qwen.gguf");
    }
}
