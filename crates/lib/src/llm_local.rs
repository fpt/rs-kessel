//! In-process llama.cpp provider via FFI (llama-cpp-2 crate).
//!
//! Loads a GGUF model directly — no server needed.

use anyhow::Result;
use std::collections::HashSet;
use std::num::NonZeroU32;
use std::path::Path;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{
    AddBos, ChatTemplateResult, GrammarTriggerType, LlamaChatTemplate, LlamaModel,
};
use llama_cpp_2::openai::OpenAIChatTemplateParams;
use llama_cpp_2::sampling::LlamaSampler;

use crate::llm::{ChatMessage, ChatRole, LlmProvider, LlmResponse, TokenUsage, ToolCallInfo, ToolDefinition};

pub struct LlamaLocalProvider {
    backend: LlamaBackend,
    model: LlamaModel,
    template: LlamaChatTemplate,
    temperature: f32,
    max_tokens: u32,
    n_ctx: u32,
}

// LlamaModel is Send+Sync. LlamaBackend and LlamaChatTemplate are safe to share.
unsafe impl Send for LlamaLocalProvider {}
unsafe impl Sync for LlamaLocalProvider {}

impl LlamaLocalProvider {
    pub fn new(
        model_path: &str,
        temperature: f32,
        max_tokens: u32,
        n_ctx: u32,
    ) -> Result<Self> {
        tracing::info!("Initializing local llama.cpp provider (FFI)");
        tracing::info!("  Model path: {}", model_path);
        tracing::info!("  Context size: {}", n_ctx);

        let mut backend = LlamaBackend::init()
            .map_err(|e| anyhow::anyhow!("Failed to init llama backend: {}", e))?;
        backend.void_logs();

        // On iOS simulator, Metal doesn't support residency sets — use CPU only.
        // Elsewhere, offload layers to the GPU backend (Metal/CUDA/Vulkan,
        // depending on the build features). On a CPU-only build these layers are
        // simply ignored by llama.cpp.
        let use_gpu = if cfg!(target_os = "ios") && cfg!(target_abi = "sim") {
            tracing::info!("  iOS simulator detected — using CPU only (no GPU)");
            false
        } else {
            true
        };

        if !use_gpu {
            // Prevent Metal residency set assertions on simulator
            unsafe { std::env::set_var("GGML_METAL_NO_RESIDENCY", "1"); }
        }

        // Layers to offload to the GPU. Override with KESSEL_GPU_LAYERS to
        // fit a smaller VRAM budget (e.g. a 6 GB card can't hold a 5 GB model
        // plus KV cache, so partial offload like 20 avoids an OOM). Default 999
        // = offload everything.
        let gpu_layers: u32 = if use_gpu {
            std::env::var("KESSEL_GPU_LAYERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(999)
        } else {
            0
        };
        tracing::info!("  GPU layers to offload: {}", gpu_layers);
        let model_params = LlamaModelParams::default()
            .with_n_gpu_layers(gpu_layers);

        let model = LlamaModel::load_from_file(&backend, Path::new(model_path), &model_params)
            .map_err(|e| anyhow::anyhow!("Failed to load model: {}", e))?;

        tracing::info!("  Model loaded: {} params", model.n_params());
        tracing::info!("  Context train: {}", model.n_ctx_train());

        let template = model
            .chat_template(None)
            .unwrap_or_else(|_| {
                tracing::warn!("No chat template in model, using chatml fallback");
                LlamaChatTemplate::new("chatml").expect("chatml is a valid template")
            });

        Ok(Self {
            backend,
            model,
            template,
            temperature,
            max_tokens,
            n_ctx,
        })
    }

    /// Serialize ChatMessages to OpenAI-compatible JSON array string.
    fn messages_to_json(messages: &[ChatMessage]) -> String {
        let json_messages: Vec<serde_json::Value> = messages
            .iter()
            .flat_map(|msg| {
                let role = match msg.role {
                    ChatRole::System => "system",
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                    ChatRole::Tool => "tool",
                };

                // Assistant with tool calls
                if let Some(ref calls) = msg.tool_calls {
                    let tool_calls: Vec<serde_json::Value> = calls
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "id": c.id,
                                "type": "function",
                                "function": {
                                    "name": c.name,
                                    "arguments": serde_json::to_string(&c.arguments).unwrap_or_default()
                                }
                            })
                        })
                        .collect();
                    return vec![serde_json::json!({
                        "role": role,
                        "content": if msg.content.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(msg.content.clone()) },
                        "tool_calls": tool_calls
                    })];
                }

                // Tool result
                if let Some(ref call_id) = msg.tool_call_id {
                    return vec![serde_json::json!({
                        "role": "tool",
                        "content": msg.content,
                        "tool_call_id": call_id
                    })];
                }

                // Regular message
                vec![serde_json::json!({
                    "role": role,
                    "content": msg.content
                })]
            })
            .collect();

        serde_json::to_string(&json_messages).unwrap_or_else(|_| "[]".to_string())
    }

    /// Serialize ToolDefinitions to OpenAI-compatible tools JSON array string.
    fn tools_to_json(tools: &[ToolDefinition]) -> String {
        let json_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect();

        serde_json::to_string(&json_tools).unwrap_or_else(|_| "[]".to_string())
    }

    /// Apply chat template with optional tools, returning ChatTemplateResult.
    fn apply_template(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatTemplateResult> {
        let messages_json = Self::messages_to_json(messages);
        let tools_json = tools.map(Self::tools_to_json);

        let params = OpenAIChatTemplateParams {
            messages_json: &messages_json,
            tools_json: tools_json.as_deref(),
            tool_choice: None,
            json_schema: None,
            grammar: None,
            reasoning_format: None,
            chat_template_kwargs: None,
            add_generation_prompt: true,
            use_jinja: true,
            parallel_tool_calls: false,
            enable_thinking: false,
            add_bos: true,
            add_eos: false,
            parse_tool_calls: tools.is_some(),
        };

        self.model
            .apply_chat_template_oaicompat(&self.template, &params)
            .map_err(|e| anyhow::anyhow!("Failed to apply chat template: {}", e))
    }

    /// Core generation loop. Tokenize, decode, sample until done.
    /// Returns (generated_text, token_usage).
    fn generate(&self, template_result: &ChatTemplateResult) -> Result<(String, TokenUsage)> {
        let tokens = self
            .model
            .str_to_token(&template_result.prompt, AddBos::Never)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let n_prompt = tokens.len() as u32;
        let n_ctx = self.n_ctx.max(n_prompt + self.max_tokens);

        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(n_ctx);

        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| anyhow::anyhow!("Failed to create context: {}", e))?;

        // Feed prompt tokens
        let mut batch = LlamaBatch::new(n_ctx as usize, 1);
        let last_index = tokens.len().saturating_sub(1) as i32;
        for (i, token) in (0_i32..).zip(tokens.iter().copied()) {
            batch
                .add(token, i, &[0], i == last_index)
                .map_err(|e| anyhow::anyhow!("Failed to add token to batch: {}", e))?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| anyhow::anyhow!("Initial decode failed: {}", e))?;

        // Build preserved token set
        let mut preserved = HashSet::new();
        for token_str in &template_result.preserved_tokens {
            if let Ok(toks) = self.model.str_to_token(token_str, AddBos::Never) {
                if toks.len() == 1 {
                    preserved.insert(toks[0]);
                }
            }
        }

        // Build sampler
        let mut sampler = self.build_sampler(template_result, &preserved)?;

        // Generate tokens
        let mut n_cur = batch.n_tokens();
        let batch_start = n_cur;
        let max_tokens = n_cur + self.max_tokens as i32;
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut generated_text = String::new();

        while n_cur <= max_tokens {
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);

            if self.model.is_eog_token(token) {
                break;
            }

            let special = preserved.contains(&token);
            let output_bytes = self
                .model
                .token_to_piece_bytes(token, 8, special, None)
                .or_else(|_| self.model.token_to_piece_bytes(token, 256, special, None))
                .map_err(|e| anyhow::anyhow!("Token decode failed: {}", e))?;

            let mut output_string = String::with_capacity(32);
            let _ = decoder.decode_to_string(&output_bytes, &mut output_string, false);
            generated_text.push_str(&output_string);

            // Check additional stop sequences
            if template_result
                .additional_stops
                .iter()
                .any(|stop| !stop.is_empty() && generated_text.ends_with(stop))
            {
                break;
            }

            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| anyhow::anyhow!("Failed to add generated token: {}", e))?;
            n_cur += 1;

            ctx.decode(&mut batch)
                .map_err(|e| anyhow::anyhow!("Decode failed: {}", e))?;
        }

        // Trim stop sequences from end
        for stop in &template_result.additional_stops {
            if !stop.is_empty() && generated_text.ends_with(stop) {
                let new_len = generated_text.len().saturating_sub(stop.len());
                generated_text.truncate(new_len);
                break;
            }
        }

        let n_output = (n_cur - batch_start) as u64;
        let usage = TokenUsage {
            input_tokens: n_prompt as u64,
            output_tokens: n_output,
            total_tokens: n_prompt as u64 + n_output,
        };
        tracing::info!(
            "Local LLM usage: input={}, output={}, total={}",
            usage.input_tokens, usage.output_tokens, usage.total_tokens
        );

        Ok((generated_text, usage))
    }

    /// Build appropriate sampler chain based on template result.
    fn build_sampler(
        &self,
        template_result: &ChatTemplateResult,
        preserved: &HashSet<llama_cpp_2::token::LlamaToken>,
    ) -> Result<LlamaSampler> {
        if let Some(ref grammar) = template_result.grammar {
            if template_result.grammar_lazy && !template_result.grammar_triggers.is_empty() {
                // Lazy grammar: only activates when triggered
                let mut trigger_patterns = Vec::new();
                let mut trigger_tokens = Vec::new();

                for trigger in &template_result.grammar_triggers {
                    match trigger.trigger_type {
                        GrammarTriggerType::Token => {
                            if let Some(token) = trigger.token {
                                trigger_tokens.push(token);
                            }
                        }
                        GrammarTriggerType::Word => {
                            if let Ok(toks) =
                                self.model.str_to_token(&trigger.value, AddBos::Never)
                            {
                                if toks.len() == 1 && preserved.contains(&toks[0]) {
                                    trigger_tokens.push(toks[0]);
                                } else {
                                    trigger_patterns.push(regex_escape(&trigger.value));
                                }
                            }
                        }
                        GrammarTriggerType::Pattern => {
                            trigger_patterns.push(trigger.value.clone());
                        }
                        GrammarTriggerType::PatternFull => {
                            trigger_patterns.push(anchor_pattern(&trigger.value));
                        }
                    }
                }

                match LlamaSampler::grammar_lazy_patterns(
                    &self.model,
                    grammar,
                    "root",
                    &trigger_patterns,
                    &trigger_tokens,
                ) {
                    Ok(grammar_sampler) => {
                        tracing::debug!("Using lazy grammar sampler");
                        return Ok(LlamaSampler::chain_simple([
                            grammar_sampler,
                            LlamaSampler::temp(self.temperature),
                            LlamaSampler::dist(1234),
                        ]));
                    }
                    Err(e) => {
                        tracing::warn!("Lazy grammar sampler failed, falling back: {}", e);
                    }
                }
            } else {
                // Strict grammar
                match LlamaSampler::grammar(&self.model, grammar, "root") {
                    Ok(grammar_sampler) => {
                        tracing::debug!("Using strict grammar sampler");
                        return Ok(LlamaSampler::chain_simple([
                            grammar_sampler,
                            LlamaSampler::temp(self.temperature),
                            LlamaSampler::dist(1234),
                        ]));
                    }
                    Err(e) => {
                        tracing::warn!("Grammar sampler failed, falling back: {}", e);
                    }
                }
            }
        }

        // Fallback: no grammar
        Ok(LlamaSampler::chain_simple([
            LlamaSampler::temp(self.temperature),
            LlamaSampler::dist(1234),
        ]))
    }

    /// Parse the OpenAI-compatible JSON from parse_response_oaicompat into LlmResponse.
    fn parse_oai_response_with_usage(json_str: &str, usage: TokenUsage) -> Result<LlmResponse> {
        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse OAI response JSON: {}", e))?;

        // Check for tool_calls
        if let Some(tool_calls) = parsed.get("tool_calls").and_then(|v| v.as_array()) {
            if !tool_calls.is_empty() {
                let calls: Vec<ToolCallInfo> = tool_calls
                    .iter()
                    .filter_map(|tc| {
                        let id = tc
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("call_0")
                            .to_string();
                        let func = tc.get("function")?;
                        let name = func.get("name")?.as_str()?.to_string();
                        let args_str = func.get("arguments").and_then(|v| v.as_str())?;
                        let arguments: serde_json::Value =
                            serde_json::from_str(args_str).unwrap_or_default();
                        Some(ToolCallInfo {
                            id,
                            name,
                            arguments,
                        })
                    })
                    .collect();

                if !calls.is_empty() {
                    tracing::info!("Local LLM returned {} tool calls", calls.len());
                    return Ok(LlmResponse::ToolCalls(calls, Some(usage)));
                }
            }
        }

        // Text response
        let content = parsed
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(LlmResponse::Text {
            content,
            reasoning: None,
            usage: Some(usage),
        })
    }
}

impl LlmProvider for LlamaLocalProvider {
    fn chat(&self, messages: &[ChatMessage]) -> Result<String> {
        let template_result = self.apply_template(messages, None)?;

        tracing::debug!(
            "Prompt length: {} chars, {} tokens (approx)",
            template_result.prompt.len(),
            template_result.prompt.len() / 4
        );

        let (text, _usage) = self.generate(&template_result)?;

        tracing::debug!("Generated: {}", text);
        Ok(text)
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let template_result = self.apply_template(messages, Some(tools))?;

        tracing::debug!(
            "Prompt: {} chars, grammar: {}, lazy: {}",
            template_result.prompt.len(),
            template_result.grammar.is_some(),
            template_result.grammar_lazy,
        );

        let (generated, usage) = self.generate(&template_result)?;

        tracing::debug!("Raw generated: {}", generated);

        // Parse response using llama.cpp's built-in parser
        if template_result.parse_tool_calls {
            match template_result.parse_response_oaicompat(&generated, false) {
                Ok(parsed_json) => {
                    tracing::debug!("Parsed OAI response: {}", parsed_json);
                    return Self::parse_oai_response_with_usage(&parsed_json, usage);
                }
                Err(e) => {
                    tracing::warn!("Failed to parse tool calls, returning as text: {}", e);
                }
            }
        }

        Ok(LlmResponse::Text {
            content: generated,
            reasoning: None,
            usage: Some(usage),
        })
    }
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '.' | '^' | '$' | '|' | '(' | ')' | '*' | '+' | '?' | '[' | ']' | '{' | '}' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn anchor_pattern(pattern: &str) -> String {
    if pattern.is_empty() {
        return "^$".to_string();
    }
    let mut anchored = String::new();
    if !pattern.starts_with('^') {
        anchored.push('^');
    }
    anchored.push_str(pattern);
    if !pattern.ends_with('$') {
        anchored.push('$');
    }
    anchored
}
