# Kessel

A voice-and-text assistant that runs on **macOS and Windows**, backed by local models (llama.cpp, in-process) or cloud LLMs. Tool calling via a ReAct loop, MCP client support, and Claude Code activity monitoring.

The agent core is Rust; each platform has a native frontend on top of it.

## Platforms

| | Frontend | Speech | Claude Code watcher |
|---|---|---|---|
| **macOS 26+** | Swift — `kessel` | Apple SpeechTranscriber (STT) + AVSpeechSynthesizer (TTS) | yes |
| **Windows** | C# / .NET 8 — `kessel.exe` | `System.Speech` recognizer + synthesizer | not yet |
| **any** | Rust — `kessel-cli` | none (text only) | — |

`kessel-cli` is the headless core: a text REPL, plus an `app-server` mode that
exposes the agent as a JSON-RPC backend for another agent (see
[App-server](#app-server)). It is the same binary on every platform and needs no
frontend.

## Features

- **Dual LLM backend**: local models via llama.cpp FFI (Gemma 4, Qwen3.5, LFM2 — auto-downloaded) or the OpenAI Responses API
- **Tool calling**: ReAct loop with built-in tools — file `read`/`write`/`edit`, `multi_edit` (atomic multi-file batch), `glob`, `grep`, `bash`, `tasks`, and skills
- **MCP client**: connect to external MCP servers over stdio or Streamable HTTP; their tools join the agent's toolset ([details](#mcp))
- **Voice I/O**: continuous conversation, half-duplex (mic muted during playback to prevent echo)
- **Claude Code watcher** (macOS): monitors Claude Code via hooks and reports activity aloud
- **Multi-language**: English and Japanese, with configurable system-prompt templates

## Requirements

**Common**
- Rust toolchain
- 16GB RAM recommended for local models (see [model sizes](CLAUDE.md#troubleshooting))

**macOS**
- macOS 26+ (Apple SpeechTranscriber)
- Xcode Command Line Tools

**Windows**
- .NET 8 SDK
- For local llama.cpp models: Visual Studio Build Tools with "Desktop development with C++", and an MSVC rustup toolchain. Cloud-only builds need neither.

## Quick Start

### macOS

```bash
# Build both binaries and install them to ~/bin
make install

# Run with OpenAI
export OPENAI_API_KEY=sk-...
kessel --config configs/openai.yaml

# ...or a local model (auto-downloads on first run)
kessel --config configs/gemma4.yaml
```

`make install` produces two binaries — they are **not** interchangeable:

- `kessel` — the voice app
- `kessel-cli` — the headless Rust core (text REPL + `app-server`)

### Windows

```bash
# Builds the Rust cdylib + kessel-cli.exe, then the C# frontend
make build-win

# Generate the C# bindings once (see CLAUDE.md for the one-time install)
bash scripts/gen_uniffi_cs.sh

# Run
win/KesselCli/bin/Release/net8.0-windows/kessel.exe --config configs/default.yaml
```

The Windows REPL has two modes, toggled with **Shift+Tab**: `text` (type → printed
reply) and `listen` (speak → reply printed *and* spoken).

Cloud-only (no llama.cpp, no C++ toolchain needed):

```bash
cd crates && cargo build --release --no-default-features -p kessel-core -p kessel-cli
```

## Configuration

YAML configs live in `configs/`. The same schema is read by every frontend.

```yaml
llm:
  # Local model: `hf:` specs auto-download into the HuggingFace cache.
  modelPath: "hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf"
  # ...or omit modelPath and set a cloud endpoint instead:
  baseURL: "https://api.openai.com/v1"
  model: "gpt-5.6-luna"
  temperature: 0.7
  maxTokens: 2048

agent:
  systemPromptPath: "system-prompt.md"  # supports the {language} variable
  maxTurns: 50
  language: "en"                        # "en" or "ja"

# External MCP servers — see the MCP section below.
mcpServers:
  - command: "godevmcp"
    args: ["serve"]
  - url: "http://127.0.0.1:27182/mcp"

tts:
  enabled: true
  voice: "com.apple.voice.enhanced.en-US.Zoe"   # macOS voice ids

stt:
  enabled: true
  locale: "en-US"

watcher:          # macOS only
  enabled: true
  debounceInterval: 3.0
```

Available configs: `default.yaml`, `openai.yaml`, `openai-ja.yaml`, `qwen3.yaml`,
`gemma4.yaml`, `lfm2.yaml`.

Model files are cached in the standard HuggingFace location, honoring
`HF_HUB_CACHE` / `HUGGINGFACE_HUB_CACHE` / `HF_HOME` (all work on Windows), so a
model already pulled by `huggingface-cli` is reused rather than re-downloaded.

## MCP

Kessel is an **MCP client**: it connects to external MCP servers at startup and
registers their tools alongside its built-ins, so the model can call them like
any other tool. This works on **both macOS and Windows** — the client lives in
the Rust core, not in a frontend.

Two transports, chosen per entry:

| Entry has | Transport |
|-----------|-----------|
| `command` (+ optional `args`) | **stdio** — kessel spawns the server as a subprocess |
| `url` | **Streamable HTTP** — with `Mcp-Session-Id`, JSON and SSE responses |

```yaml
mcpServers:
  - command: "npx"                          # stdio
    args: ["-y", "@modelcontextprotocol/server-everything"]
  - url: "http://127.0.0.1:27182/mcp"       # Streamable HTTP
```

On Windows the stdio transport resolves `npx`/`pnpm`-style `.cmd`/`.bat` shims via
`PATH`+`PATHEXT`, which a bare process spawn does not do.

A server that fails to connect is logged and skipped, so one bad entry cannot take
down the agent.

The headless `kessel-cli` reads stdio servers from the `MCP_SERVERS` environment
variable instead (comma-separated `command arg1 arg2`).

## App-server

`kessel-cli app-server` exposes the agent as a **whole-turn backend** over
line-delimited JSON-RPC on stdio: a driving client hands kessel an entire
conversation turn and gets back the final text, while kessel runs its own ReAct
loop, tools and MCP connections inside that turn.

It speaks a subset of the codex app-server protocol, so a client that already
drives `codex app-server` can drive kessel by swapping the binary. This is how
[klein](https://github.com/fpt/klein-cli) uses it (`llm.backend: "kessel"`).

```bash
OPENAI_API_KEY=sk-... kessel-cli app-server
```

## Claude Code Integration

*(macOS only — the Windows frontend has no watcher yet.)*

The watcher monitors Claude Code activity and speaks brief summaries when it
edits files, runs tests, or commits.

```bash
bash scripts/install-claude-hook.sh     # installs the hook into ~/.claude/settings.json
kessel --config configs/openai.yaml
```

## Skills

Skills load from `skills/` (project) and `~/.claude/plugins/`. Each is a `SKILL.md`
with YAML frontmatter:

```markdown
---
name: my-skill
description: "What this skill does"
---
Prompt body injected as system context...
```

## Commands

**macOS (`kessel`)**

| Command | Description |
|---------|-------------|
| `/listen` | Listen once, then reply |
| `/goal <condition>` | Work toward a condition across turns; `/goal` for status, `/goal clear` to stop |
| `/loop [interval] <prompt>` | Ambient mode: run a prompt periodically in the background |
| `/reset` | Clear conversation history |
| `/history` | Show conversation |
| `/voices` | List available TTS voices |
| `/stop` | Stop current TTS playback |
| `/help` | Show help |
| `/quit` | Exit |

**Windows (`kessel.exe`)**

| Command | Description |
|---------|-------------|
| **Shift+Tab** | Toggle `text` ⇄ `listen` mode |
| `/listen` | Listen once, then reply |
| `/reset` | Clear conversation history |
| `/history` | Show conversation |
| `/help` | Show help |
| `/quit` | Exit |

## Architecture

```
macOS:    Mic -> SpeechTranscriber -> Swift CLI ─┐
Windows:  Mic -> System.Speech     -> C# CLI   ──┼─> UniFFI -> Rust Agent
any:                                 Rust CLI  ──┘            -> ReAct loop
                                                                 (LLM + tools + MCP)
```

- **Rust** (`crates/lib`): agent core — LLM providers, ReAct loop, tools, skills, MCP, memory
- **Rust** (`crates/app`): `kessel-cli` — headless REPL + `app-server`
- **Swift** (`swift/`): macOS voice app, audio pipeline, TTS, watcher
- **C#** (`win/`): Windows frontend
- **UniFFI**: generates the Swift and C# bindings to the Rust core

### LLM Providers

| Provider | Backend | Tool Calling | Notes |
|----------|---------|--------------|-------|
| `LlamaLocalProvider` | llama-cpp-2 FFI | Native (per the model's chat template) | In-process; no server needed |
| `OpenAiProvider` | Responses API | Native | Supports reasoning models |

## Development

See [CLAUDE.md](CLAUDE.md) for the full developer guide.

```bash
cd crates && cargo build --release
cd crates && cargo test

# Regenerate UniFFI bindings after changing crates/lib/src/agent.udl
bash scripts/gen_uniffi.sh        # Swift (copies into the tracked tree)
bash scripts/gen_uniffi_cs.sh     # C#

cd swift && swift build
```

## License

MIT
