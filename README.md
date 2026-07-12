# Kessel

A voice assistant for macOS that runs locally or with cloud LLMs. Supports continuous voice conversation, tool calling via ReAct loop, and Claude Code activity monitoring.

## Features

- **Dual LLM backend**: Local models via llama.cpp FFI (Qwen3.5-9B, etc.) or OpenAI Responses API
- **Voice I/O**: Apple SpeechTranscriber (STT) + AVSpeechSynthesizer (TTS)
- **Tool calling**: ReAct loop with built-in tools (shell, file read/write, web fetch) and extensible skill system
- **Claude Code watcher**: Monitors Claude Code activity via hooks and reports changes aloud
- **Multi-language**: English and Japanese with configurable system prompt templates
- **Half-duplex**: Mutes mic during TTS playback to prevent echo

## Requirements

- macOS 26+ (for Apple SpeechTranscriber)
- Rust toolchain
- Xcode Command Line Tools
- 16GB RAM recommended for local models

## Quick Start

### With OpenAI (easiest)

```bash
# Build
cd crates && cargo build --release
bash scripts/gen_uniffi.sh
cd ../swift && swift build

# Run
export OPENAI_API_KEY=sk-...
swift run kessel-cli --config ../configs/openai.yaml
```

### With local model (Qwen3.5-9B)

```bash
# Build
cd crates && cargo build --release
bash scripts/gen_uniffi.sh
cd ../swift && swift build

# Run (model auto-downloads on first run)
swift run kessel-cli --config ../configs/qwen3.yaml
```

## Configuration

YAML configs live in `configs/`. Key sections:

```yaml
llm:
  modelPath: "../models/Qwen3.5-9B-Q4_K_M.gguf"  # Local model (omit for cloud)
  baseURL: "https://api.openai.com/v1"           # API endpoint (omit for local)
  model: "gpt-5.6-luna"
  harmonyTemplate: false
  temperature: 0.7
  maxTokens: 2048

agent:
  systemPromptPath: "system-prompt.md"  # Supports {language} template variable
  maxTurns: 50
  language: "en"                        # "en" or "ja"

tts:
  enabled: true
  voice: "com.apple.voice.enhanced.en-US.Zoe"
  rate: 0.5

stt:
  enabled: true
  locale: "en-US"

watcher:
  enabled: true
  debounceInterval: 3.0
```

Available configs: `default.yaml`, `openai.yaml`, `openai-ja.yaml`, `qwen3.yaml`

## Claude Code Integration

The watcher monitors Claude Code activity and provides spoken summaries.

```bash
# Install the hook into ~/.claude/settings.json
bash scripts/install-claude-hook.sh

# Run kessel-cli with watcher enabled
swift run kessel-cli --config ../configs/openai.yaml
```

When Claude Code edits files, runs tests, or commits code, kessel-cli speaks a brief summary.

## Skills

Skills are loaded from `skills/` (project directory) and `~/.claude/plugins/`. Each skill is a `SKILL.md` file with YAML frontmatter:

```markdown
---
name: my-skill
description: "What this skill does"
---
Prompt body injected as system context...
```

## Commands

| Command    | Description                    |
|------------|--------------------------------|
| `/reset`   | Clear conversation history     |
| `/quit`    | Exit                           |
| `/history` | Show conversation              |
| `/voices`  | List available TTS voices      |
| `/stop`    | Stop current TTS playback      |

## Architecture

```
Mic -> SpeechTranscriber (STT) -> Swift CLI -> UniFFI -> Rust Agent
  -> ReAct loop (LLM + tools) -> Response -> AVSpeechSynthesizer (TTS)
```

- **Swift** (`swift/`): CLI, audio pipeline, TTS, watcher, config
- **Rust** (`crates/lib`): Agent core, LLM providers, ReAct loop, tools, skills, memory
- **UniFFI**: Rust-Swift bridge via generated FFI bindings

### LLM Providers

| Provider | Backend | Tool Calling | Notes |
|----------|---------|-------------|-------|
| `LlamaLocalProvider` | llama-cpp-2 FFI | Grammar-constrained | No server needed |
| `OpenAiProvider` | Responses API | Native | Supports reasoning models |

## Development

```bash
# Build Rust
cd crates && cargo build --release

# Run tests
cd crates && cargo test

# Regenerate UniFFI bindings (after .udl changes)
bash scripts/gen_uniffi.sh
cp vendor/uniffi-swift/kessel_core.swift swift/Sources/AgentBridge/

# Build Swift
cd swift && swift build
```

## License

MIT
