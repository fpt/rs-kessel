# Voice Agent - Developer Guide

## Overview

A macOS voice assistant with local and cloud LLM support, continuous voice I/O, tool calling, and Claude Code activity monitoring.

- **Platform**: macOS 26+ (requires Apple SpeechTranscriber)
- **Swift**: swift-tools-version 6.1, `.swiftLanguageMode(.v5)` on all targets
- **Rust**: workspace in `crates/` with two members: `lib` (library) and `app` (binary)

## Architecture

```
Mic -> AVAudioEngine -> SpeechAnalyzer/SpeechTranscriber (STT)
    -> Swift CLI (main.swift)
    -> UniFFI bridge
    -> Rust Agent (lib.rs)
    -> ReAct loop (react.rs) with LLM provider + tool registry
    -> Response
    -> AVSpeechSynthesizer (TTS) -> Speaker
```

### Rust Crates (`crates/`)

| File | Purpose |
|------|---------|
| `lib/src/lib.rs` | Agent struct, UniFFI exports, provider factory |
| `lib/src/llm.rs` | LlmProvider trait, OpenAiProvider (Responses API) |
| `lib/src/llm_local.rs` | LlamaLocalProvider (in-process llama-cpp-2 FFI) |
| `lib/src/react.rs` | Provider-agnostic ReAct loop |
| `lib/src/tool.rs` | ToolRegistry, ToolHandler trait, ToolAccess trait, built-in tools, ToolSession (read-tracking + permissions) |
| `lib/src/skill.rs` | SkillRegistry, lookup_skill tool |
| `lib/src/memory.rs` | ConversationMemory (thread-safe) |
| `lib/src/state_capsule.rs` | State capsule for context injection |
| `lib/src/state_updater.rs` | Rule-based state extraction from responses |
| `lib/src/harmony.rs` | Harmony template parser (for gpt-oss models) |
| `lib/src/agent.udl` | UniFFI interface definition |
| `app/src/main.rs` | Standalone Rust CLI (for testing without Swift) |

### Swift Packages (`swift/Sources/`)

| Package | Purpose |
|---------|---------|
| `VoiceAgentCLI` | Main entry point, text/voice mode, watcher integration |
| `Audio` | AudioCapture (mic -> SpeechTranscriber), VoiceProcessingIO |
| `TTS` | AVSpeechSynthesizer wrapper |
| `Watcher` | SessionJSONLWatcher, SocketReceiver, EventPipeline |
| `Util` | Config, Logger, HarmonyParser, SkillLoader, ModelDownloader |
| `AgentBridge` | Generated UniFFI Swift bindings |
| `AgentBridgeFFI` | C module map for FFI |
| `LLM` | LanguageClient protocol (experimental) |

### Key Patterns

- `ChatMessage` has `#[serde(skip)]` fields for tool state; use helper methods (`ChatMessage::user()`, `ChatMessage::assistant()`, etc.) not struct literals
- ReAct loop in `react.rs` is provider-agnostic; each provider serializes to its own wire format in `chat_with_tools()`
- OpenAI provider uses Responses API (`/v1/responses`) with `function_call`/`function_call_output` input items
- Local LLM tool calling: `apply_chat_template_oaicompat()` -> grammar-constrained generation -> `parse_response_oaicompat()`
- `ToolAccess` trait abstracts `ToolRegistry` and `FilteredToolRegistry` for restricted tool access
- Built-in tools (`create_default_registry`): `read`, `glob`, `grep`, `write`, `edit`, `bash`, `tasks`, `lookup_skill`, `read_situation_messages` (+ `capture_screen`/`find_window`/`apply_ocr` from `lib.rs`, + MCP tools)
- **Mutation safety** (`ToolSession`, shared per-agent): `edit` and overwriting `write` require the file to have been `read` first (read-first, like klein-cli). `write`/`edit` and non-whitelisted `bash` commands prompt on the terminal (`1` yes / `2` yes-to-all (remembered for the session) / `3` no). `bash` runs a default allowlist (make, go, gcc, uv, cargo, ls, ps, cd, pwd, grep, …; extend via `VOICE_AGENT_BASH_ALLOW`) without prompting. Non-interactive contexts (no TTY) deny mutations unless `VOICE_AGENT_AUTO_APPROVE=1`.
- Half-duplex: `AudioCapture.mute()`/`unmute()` drops audio buffers during TTS playback

## Configuration

YAML configs in `configs/`. System prompt supports `{language}` template variable.

```yaml
llm:
  modelPath: "../models/Qwen3-8B-Q4_K_M.gguf"  # For local provider (local path), OR
  # modelPath: "hf:LiquidAI/LFM2.5-8B-A1B-GGUF/LFM2.5-8B-A1B-Q4_K_M.gguf"  # auto-download
  modelRepo: "Qwen/Qwen3-8B-GGUF"              # HuggingFace repo (Swift auto-download)
  modelFile: "Qwen3-8B-Q4_K_M.gguf"            # File in repo
  baseURL: "https://api.openai.com/v1"          # For OpenAI provider
  model: "gpt-5.4-mini"
  apiKey: ""                                     # Or OPENAI_API_KEY env var
  harmonyTemplate: false
  temperature: 0.7
  maxTokens: 2048
  reasoningEffort: "medium"                      # For reasoning models

agent:
  systemPromptPath: "system-prompt.md"           # Relative to config dir
  maxTurns: 50
  language: "en"                                 # "en" or "ja"

tts:
  enabled: true
  voice: "com.apple.voice.enhanced.en-US.Zoe"
  rate: 0.5
  pitchMultiplier: 1.0
  volume: 1.0

stt:
  enabled: true
  locale: "en-US"                                # BCP47 locale
  censor: false

watcher:
  enabled: true
  debounceInterval: 3.0
```

Provider selection logic: if `modelPath` is set -> `LlamaLocalProvider`; else if `baseURL` is set -> `OpenAiProvider`.

### Model auto-download (`lib/src/model_downloader.rs`)

`modelPath` may be a HuggingFace spec instead of a local path:
`hf:ORG/REPO[@REVISION]/path/to/file.gguf` (e.g.
`hf:LiquidAI/LFM2.5-8B-A1B-GGUF/LFM2.5-8B-A1B-Q4_K_M.gguf`). On first use the
local provider downloads it into the HuggingFace hub cache
(`HF_HUB_CACHE`/`HUGGINGFACE_HUB_CACHE`/`HF_HOME`/`~/.cache/huggingface/hub`),
laid out as `models--org--name/{blobs,snapshots/<commit>,refs}` like
`huggingface_hub`. Downloads are **transactional** (stream to
`blobs/<etag>.incomplete`, atomic-rename on success) and **resumable** (a
leftover `.incomplete` continues via an HTTP `Range` request). Set `HF_TOKEN` /
`HUGGING_FACE_HUB_TOKEN` for gated/private repos. A plain `modelPath` that is an
existing file is used as-is.

## Skills

Skills are `SKILL.md` files with YAML frontmatter loaded from:
1. `skills/` directory (relative to config file's parent)
2. `~/.claude/plugins/` (recursive)

The `claude-activity-report` skill is used by the watcher via `chat_once(input, skillName:)`.

## Build & Run

```bash
# Rust
cd crates && cargo build --release
cd crates && cargo test

# UniFFI (after .udl changes)
bash scripts/gen_uniffi.sh
cp vendor/uniffi-swift/agent_core.swift swift/Sources/AgentBridge/

# Swift
cd swift && swift build

# Run
cd swift && swift run voice-agent --config ../configs/openai.yaml
cd swift && swift run voice-agent --config ../configs/qwen3.yaml

# Local model standalone (Rust only, no Swift)
MODEL_PATH=../models/Qwen3-8B-Q4_K_M.gguf cargo run -p app
```

## Windows CLI (`win/`)

A C# console frontend (text REPL) that consumes the same Rust `agent_core`
library through **UniFFI C# bindings** — the Windows analogue of the Swift CLI.
Mirrors the `../rs-gallium` approach (cdylib + `uniffi-bindgen-cs` + .NET).

```bash
# 1a. Cloud-only cdylib (no llama.cpp; uses whatever cargo is on PATH):
cd crates && cargo build --release --no-default-features   # -> crates/target/release/agent_core.dll

# 1b. With in-process llama.cpp (local GGUF models): use the helper script.
#     It enters the MSVC dev env (vcvars64), forces cmake's Ninja generator, and
#     uses rustup's MSVC toolchain so the C++ libs (common.lib/llama.lib, built by
#     cl.exe) match what rustc links. Building with the GNU toolchain fails with
#     "could not find native static library `common`".
#     Requires: VS Build Tools w/ "Desktop development with C++" (cl.exe + Windows
#     SDK + bundled Ninja), and an up-to-date rustup MSVC toolchain (rustup update stable).
scripts/build-win-local.bat

# 2. Generate C# bindings into win/vendor/ (install once:
#    cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.9.0+v0.28.3)
bash scripts/gen_uniffi_cs.sh

# 3. Build & run the CLI (net8.0, x64). The build copies agent_core.dll next to
#    the exe as uniffi_agent_core.dll (the DllImport name the bindings expect).
dotnet build win/VoiceAgentCLI/VoiceAgentCLI.csproj -c Release
win/VoiceAgentCLI/bin/Release/net8.0/voice-agent.exe --config configs/default.yaml
```

- `win/VoiceAgentCLI/Program.cs` — text REPL (`/reset`, `/history`, `/help`, `/quit`)
- `win/VoiceAgentCLI/AppConfig.cs` — YAML loader (YamlDotNet) for the same `configs/*.yaml` schema; API key falls back to `OPENAI_API_KEY`
- `win/vendor/agent_core.cs` — generated bindings (gitignored; namespace `uniffi.agent_core`, `DllImport("uniffi_agent_core")`)
- No TTS/STT/watcher yet — cloud + local chat only.

## Claude Code Watcher

Monitors Claude Code via hooks (PostToolUse, Stop events) sent over a Unix domain socket.

- **Hook script**: `scripts/voice-agent-hook.sh` forwards stdin JSON to `/tmp/voice-agent-<uid>.sock`
- **Install**: `bash scripts/install-voice-agent-hook.sh` copies hook and updates `~/.claude/settings.json`
- **SocketReceiver** (`swift/Sources/Watcher/`): listens on the socket, parses ndjson
- **EventPipeline**: debounces events, summarizes via `EventSummarizer`, calls `agent.chatOnce()` with the `claude-activity-report` skill
- **SessionJSONLWatcher**: also watches Claude Code's session JSONL file for events

## Project Structure

```
voice-agent/
├── configs/                    # YAML configurations
│   ├── default.yaml            # Default (OpenAI, English)
│   ├── openai.yaml             # OpenAI with watcher
│   ├── openai-ja.yaml          # OpenAI, Japanese
│   ├── qwen3.yaml              # Local Qwen3-8B
│   └── system-prompt.md        # System prompt template ({language})
├── skills/                     # Project-local skills
│   └── claude-activity-report/SKILL.md
├── crates/                     # Rust workspace
│   ├── lib/src/                # Agent core library (agent_core)
│   └── app/src/                # Standalone Rust CLI
├── swift/                      # Swift package
│   └── Sources/
│       ├── VoiceAgentCLI/      # Main entry point
│       ├── Audio/              # SpeechTranscriber, AudioCapture
│       ├── TTS/                # AVSpeechSynthesizer
│       ├── Watcher/            # Claude Code monitoring
│       ├── Util/               # Config, Logger, SkillLoader
│       ├── AgentBridge/        # UniFFI Swift bindings
│       └── AgentBridgeFFI/     # C module map
├── scripts/
│   ├── gen_uniffi.sh           # Generate UniFFI bindings
│   ├── install-voice-agent-hook.sh  # Install Claude Code hook
│   ├── voice-agent-hook.sh          # Hook script (stdin -> socket)
│   └── ...
├── vendor/uniffi-swift/        # Generated UniFFI outputs
└── models/                     # GGUF models (gitignored)
```

## Troubleshooting

**"library 'agent_core' not found"**: `cd crates && cargo build --release`

**"no such module 'agent_coreFFI'"**: `bash scripts/gen_uniffi.sh`

**UniFFI checksum mismatch**: Regenerate bindings and copy: `bash scripts/gen_uniffi.sh && cp vendor/uniffi-swift/agent_core.swift swift/Sources/AgentBridge/`

**Local model OOM**: Use a smaller quantization or model. Qwen3-8B Q4_K_M (5GB) works on M3 16GB.
