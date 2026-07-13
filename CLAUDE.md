# Kessel - Developer Guide

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
| `lib/src/appserver/` | JSON-RPC app-server: exposes the agent as a whole-turn backend (see below) |
| `lib/src/agent.udl` | UniFFI interface definition |
| `app/src/main.rs` | Standalone Rust CLI (REPL + `app-server` subcommand) |

### Swift Packages (`swift/Sources/`)

| Package | Purpose |
|---------|---------|
| `KesselCli` | Main entry point, text/voice mode, watcher integration |
| `Audio` | AudioCapture (mic -> SpeechTranscriber), VoiceProcessingIO |
| `TTS` | AVSpeechSynthesizer wrapper |
| `Watcher` | SessionJSONLWatcher, SocketReceiver, EventPipeline |
| `Util` | Config, Logger, HarmonyParser, SkillLoader, WhisperModelDownloader |
| `AgentBridge` | Generated UniFFI Swift bindings |
| `AgentBridgeFFI` | C module map for FFI |
| `LLM` | LanguageClient protocol (experimental) |

### Key Patterns

- `ChatMessage` has `#[serde(skip)]` fields for tool state; use helper methods (`ChatMessage::user()`, `ChatMessage::assistant()`, etc.) not struct literals
- ReAct loop in `react.rs` is provider-agnostic; each provider serializes to its own wire format in `chat_with_tools()`
- OpenAI provider uses Responses API (`/v1/responses`) with `function_call`/`function_call_output` input items
- Local LLM tool calling (`llm_local.rs`, llama-cpp-2 0.1.151): the GGUF's embedded jinja chat template is rendered with **minijinja** (0.1.150 removed llama-cpp-2's OAI-compat/jinja chat layer). Templates that declare tools natively (gemma 4's `<|tool>` / `<|tool_call>`) get the tools passed through the template itself, so the model sees them in the form it was trained on; otherwise tools are injected into the system prompt with a JSON output protocol. The reply is parsed leniently in multiple formats (JSON object/array, OpenAI `tool_calls`, Python/Llama `[name(arg=val)]`, and gemma's native `<|tool_call>call:…`), after stripping `<think>` blocks. Fallbacks: system-role fold (for templates that reject a system role — **not** gemma 4, which accepts one) → manual ChatML. Verified with gemma-4-E4B and LFM2.5-8B-A1B. See **[docs/GEMMA4.md](docs/GEMMA4.md)** for the wire format.
- `ToolAccess` trait abstracts `ToolRegistry` and `FilteredToolRegistry` for restricted tool access
- Built-in tools (`create_default_registry`): `read`, `glob`, `grep`, `write`, `edit`, `bash`, `tasks`, `lookup_skill`, `read_situation_messages` (+ `capture_screen`/`find_window`/`apply_ocr` from `lib.rs`, + MCP tools)
- **Mutation safety** (`ToolSession`, shared per-agent): `edit` and overwriting `write` require the file to have been `read` first (read-first, like klein-cli). `write`/`edit` and non-whitelisted `bash` commands prompt on the terminal (`1` yes / `2` yes-to-all (remembered for the session) / `3` no). `bash` runs a default allowlist (make, go, gcc, uv, cargo, ls, ps, cd, pwd, grep, …; extend via `KESSEL_BASH_ALLOW`) without prompting. Non-interactive contexts (no TTY) deny mutations unless `KESSEL_AUTO_APPROVE=1`.
- Half-duplex: `AudioCapture.mute()`/`unmute()` drops audio buffers during TTS playback

## Configuration

YAML configs in `configs/`. System prompt supports `{language}` template variable.

```yaml
llm:
  modelPath: "hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf"  # Local provider; auto-downloads
  # modelPath: "../models/Qwen3.5-9B-Q4_K_M.gguf"  # ...or a plain path to an existing GGUF
  baseURL: "https://api.openai.com/v1"          # For OpenAI provider
  model: "gpt-5.6-luna"
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
cp vendor/uniffi-swift/kessel_core.swift swift/Sources/AgentBridge/

# Swift
cd swift && swift build

# Run
cd swift && swift run kessel-cli --config ../configs/openai.yaml
cd swift && swift run kessel-cli --config ../configs/qwen3.yaml

# Local model standalone (Rust only, no Swift)
MODEL_PATH=hf:unsloth/Qwen3.5-9B-GGUF/Qwen3.5-9B-Q4_K_M.gguf cargo run -p kessel-cli

# As a whole-turn backend for another agent (see "App-server mode")
OPENAI_API_KEY=sk-... cargo run -p kessel-cli -- app-server
```

### `make install` — two binaries

`make install` installs **both** builds into `$PREFIX/bin` (default `~/bin`).
They are not interchangeable:

| Installed as | Built from | What it is |
|--------------|-----------|------------|
| `kessel-cli` | `crates/` (Rust) | Text REPL **plus `app-server`** — the JSON-RPC whole-turn backend. Statically linked, so it runs from anywhere. |
| `kessel` | `swift/` | The voice app: TTS/STT and the Claude Code watcher. Links `libkessel_core.dylib` by **absolute path** into this repo, so the repo must stay put. |

Only `kessel-cli` understands `app-server`, and [klein](../klein-cli) spawns
`kessel-cli app-server` by default (`kessel_path` in its settings). The Swift
binary silently ignores an `app-server` argument and boots the voice agent
instead, so installing it under that name breaks klein's kessel backend.

## Windows CLI (`win/`)

A C# console frontend (text REPL) that consumes the same Rust `kessel_core`
library through **UniFFI C# bindings** — the Windows analogue of the Swift CLI.
Mirrors the `../rs-gallium` approach (cdylib + `uniffi-bindgen-cs` + .NET).

Windows produces **two binaries**, the same split as macOS (see `make install`):

| Binary | Built from | What it is |
|--------|-----------|------------|
| `kessel-cli.exe` | `crates/` (Rust) | REPL **plus `app-server`** — the JSON-RPC backend klein spawns. Statically links `kessel_core`, so it needs no DLL beside it. |
| `kessel.exe` | `win/KesselCli/` (C#) | The frontend: text/listen REPL, TTS/STT. Needs `uniffi_kessel_core.dll` beside it (the csproj copies the cdylib under that name). |

Only `kessel-cli.exe` understands `app-server`. The build scripts build the
cdylib **and** `kessel-cli.exe` in a single cargo invocation with the same
feature — building them separately would resolve `kessel-core` differently for
each and overwrite the GPU `kessel_core.dll` with a CPU one.

```bash
# 1a. Cloud-only (no llama.cpp; uses whatever cargo is on PATH):
cd crates && cargo build --release --no-default-features -p kessel-core -p kessel-cli
#     -> crates/target/release/{kessel_core.dll, kessel-cli.exe}

# 1b. With in-process llama.cpp (local GGUF models): use the helper script.
#     It enters the MSVC dev env (vcvars64), forces cmake's Ninja generator, and
#     uses rustup's MSVC toolchain so the C++ libs (common.lib/llama.lib, built by
#     cl.exe) match what rustc links. Building with the GNU toolchain fails with
#     "could not find native static library `common`".
#     Requires: VS Build Tools w/ "Desktop development with C++" (cl.exe + Windows
#     SDK + bundled Ninja), and an up-to-date rustup MSVC toolchain (rustup update stable).
scripts/build-win-local.bat

# 1c. GPU-accelerated llama.cpp. Cargo features: cuda / metal / vulkan (each implies
#     local). On Windows with NVIDIA:
scripts/build-win-cuda.bat   # CUDA build; pins a Pascal-capable toolkit + sm_61
#     NOTE: CUDA 13 dropped Pascal (GTX 10xx). The script defaults to CUDA_VER=v12.9,
#     CUDA_ARCH=61 (GTX 1060); override for other GPUs, e.g.:
#       set CUDA_VER=v12.9 & set CUDA_ARCH=86 & scripts\build-win-cuda.bat
#     The provider offloads all layers by default; cap it for small VRAM via
#     KESSEL_GPU_LAYERS=N (e.g. 20 on a 6 GB card with a big model).

# 2. Generate C# bindings into win/vendor/ (install once:
#    cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.9.0+v0.28.3)
bash scripts/gen_uniffi_cs.sh

# 3. Build & run the C# frontend (net8.0, x64). The build copies kessel_core.dll
#    next to the exe as uniffi_kessel_core.dll (the DllImport name the bindings
#    expect). Emits kessel.exe.
dotnet build win/KesselCli/KesselCli.csproj -c Release
win/KesselCli/bin/Release/net8.0-windows/kessel.exe --config configs/default.yaml

# 4. The Rust CLI came out of step 1 and needs no dotnet build. Point klein's
#    `kessel_path` at it (or put it on PATH as kessel-cli.exe).
crates\target\release\kessel-cli.exe app-server
```

- `win/KesselCli/Program.cs` — REPL with two modes toggled by **Shift+Tab** (like Claude Code's plan/auto cycle): `text` (type → printed reply) ⇄ `listen` (speak → reply printed **and** spoken). Interactive terminals use a key-level loop (`ReadLineOrToggle`/`TogglePressed`); piped stdin (the testsuite) uses a plain line loop with no mode switching. Commands: `/listen` (one-shot), `/reset`, `/history`, `/help`, `/quit`.
- `win/KesselCli/SpeechInput.cs` — STT via `System.Speech` (Windows desktop recognizer). `RecognizeOnce()` for the `/listen` command; `Listen(toggleRequested)` for continuous listen mode (async recognition + key polling so Shift+Tab stays responsive; mic released during TTS = half-duplex). Requires `net8.0-windows`.
- `win/KesselCli/VoiceOutput.cs` — TTS via `System.Speech.Synthesis`; strips `<think>` blocks and markdown before speaking.
- `win/KesselCli/DotEnv.cs` — loads a local `.env` at startup (nearest from cwd, nearest from the exe dir, then `~/.cache/kessel/.env`) so keys like `OPENAI_API_KEY` can live in a file. Parses `KEY=VALUE` (tolerates `export`, `#` comments, quotes); existing environment variables are not overridden. Runs before config load, so `.env` may also set `KESSEL_CONFIG`.
- `win/KesselCli/AppConfig.cs` — YAML loader (YamlDotNet) for the same `configs/*.yaml` schema; API key falls back to `OPENAI_API_KEY` (which `.env` can supply). Config resolution order: `--config` → `KESSEL_CONFIG` env → **`~/.cache/kessel/config.yml`** (the user default; `.yaml` also accepted) → repo `configs/default.yaml` (cwd or walked up from the exe). If nothing is found, a commented starter config (LLM + TTS/STT + `mcpServers` example) is scaffolded at `~/.cache/kessel/config.yml`.
- MCP client works on Windows. Config `mcpServers:` entries are either **stdio** (`command` + `args`) or **Streamable HTTP** (`url:`); `agent_new` picks the transport per entry. stdio (`mcp_client.rs`) resolves `npx`/`pnpm`/etc. `.cmd`/`.bat` shims via PATH+PATHEXT (`Command::new` doesn't), and its JSON-RPC read loop skips notifications/log lines while waiting for the matching id. HTTP (`mcp_client_http.rs`) speaks Streamable HTTP with `Mcp-Session-Id` and JSON/SSE responses. Verified against `npx -y @modelcontextprotocol/server-everything` (stdio) and the Autodesk Fusion MCP server at `http://127.0.0.1:27182/mcp` (HTTP — discovered & called `fusion_mcp_read`).
- `win/vendor/kessel_core.cs` — generated bindings (gitignored; namespace `uniffi.kessel_core`, `DllImport("uniffi_kessel_core")`)
- No watcher integration yet.

## App-server mode (`lib/src/appserver/`)

`kessel-cli app-server` exposes the agent as a **whole-turn backend** over
line-delimited JSON-RPC 2.0 on stdio: a driving client hands kessel an entire
conversation turn and takes back the final text, while kessel runs its own ReAct
loop, tools, and MCP connections inside that turn.

It speaks a **subset of the codex app-server protocol**, so a client that already
drives `codex app-server` can drive kessel by swapping the binary. This is how
[klein](../klein-cli) consumes it: `llm.backend: "kessel"` in klein's
`settings.json`, served by `internal/agentserver` — the same runner it uses for
codex.

```bash
OPENAI_API_KEY=sk-... kessel-cli app-server   # or MODEL_PATH=/path/to.gguf
```

| Method | Direction | Purpose |
|--------|-----------|---------|
| `initialize` | in | capability negotiation (`experimentalApi`) |
| `account/read` | in | readiness probe; kessel reports no auth requirement |
| `thread/start` | in | create a thread (an LLM provider + tool registry + history) |
| `turn/start` | in | run one turn, block until it completes |
| `item/tool/call` | **out** | invoke a client-provided `dynamicTools` tool |
| `item/{commandExecution,fileChange}/requestApproval` | **out** | ask the client to permit a mutation |
| `item/completed`, `turn/completed`, `turn/failed` | **out** | progress notifications |

Key points:

- **The transport is bidirectional** (`rpc.rs`), unlike `mcp_server.rs`'s strict
  request→response loop. Inbound requests are dispatched on their own threads so
  a long `turn/start` can originate tool-call requests while the reader keeps
  running — both sides block on each other otherwise. `serve()` joins in-flight
  handlers before returning, and cancels pending outbound requests first so a
  handler awaiting a hung-up client cannot deadlock the join.
- **Client tools** (`thread/start`'s `dynamicTools`) become `ToolHandler`s that
  call back over the connection — the mirror image of `McpRemoteTool`, which
  wraps a tool living in a subprocess *we* spawned.
- **Approvals**: there is no TTY, so `ToolSession`'s terminal prompt would fail
  closed on every `write`/`edit`/`bash`. `ApprovalSink` redirects the question to
  the client. `approvalPolicy: "never"` installs `AutoApproveSink` instead.
  Kessel has **no sandbox**, so this is the only gate on mutations.
- **Logs go to stderr** in this mode — stdout carries the JSON-RPC stream.
- `react::run_observed` reports each tool call/result so progress notifications
  can be emitted mid-turn rather than going silent for minutes.

## Claude Code Watcher

Monitors Claude Code via hooks (PostToolUse, Stop events) sent over a Unix domain socket.

- **Hook script**: `scripts/claude-hook.sh` forwards stdin JSON to `/tmp/kessel-cli-<uid>.sock`
- **Install**: `bash scripts/install-claude-hook.sh` copies hook and updates `~/.claude/settings.json`
- **SocketReceiver** (`swift/Sources/Watcher/`): listens on the socket, parses ndjson
- **EventPipeline**: debounces events, summarizes via `EventSummarizer`, calls `agent.chatOnce()` with the `claude-activity-report` skill
- **SessionJSONLWatcher**: also watches Claude Code's session JSONL file for events

## Project Structure

```
kessel-cli/
├── configs/                    # YAML configurations
│   ├── default.yaml            # Default (OpenAI, English)
│   ├── openai.yaml             # OpenAI with watcher
│   ├── openai-ja.yaml          # OpenAI, Japanese
│   ├── qwen3.yaml              # Local Qwen3.5-9B
│   ├── gemma4.yaml             # Local Gemma 4 26B-A4B (QAT)
│   └── system-prompt.md        # System prompt template ({language})
├── skills/                     # Project-local skills
│   └── claude-activity-report/SKILL.md
├── crates/                     # Rust workspace
│   ├── lib/src/                # Agent core library (kessel_core)
│   └── app/src/                # Standalone Rust CLI
├── swift/                      # Swift package
│   └── Sources/
│       ├── KesselCli/      # Main entry point
│       ├── Audio/              # SpeechTranscriber, AudioCapture
│       ├── TTS/                # AVSpeechSynthesizer
│       ├── Watcher/            # Claude Code monitoring
│       ├── Util/               # Config, Logger, SkillLoader
│       ├── AgentBridge/        # UniFFI Swift bindings
│       └── AgentBridgeFFI/     # C module map
├── scripts/
│   ├── gen_uniffi.sh           # Generate UniFFI bindings
│   ├── install-claude-hook.sh  # Install Claude Code hook
│   ├── claude-hook.sh          # Hook script (stdin -> socket)
│   └── ...
├── vendor/uniffi-swift/        # Generated UniFFI outputs
└── models/                     # GGUF models (gitignored)
```

## Troubleshooting

**"library 'kessel_core' not found"**: `cd crates && cargo build --release`

**"no such module 'kessel_coreFFI'"**: `bash scripts/gen_uniffi.sh`

**UniFFI checksum mismatch**: Regenerate bindings and copy: `bash scripts/gen_uniffi.sh && cp vendor/uniffi-swift/kessel_core.swift swift/Sources/AgentBridge/`

**Local model OOM**: Use a smaller quantization or model. Rough weights-on-disk for the shipped configs:

| config | model | size |
|--------|-------|------|
| `gemma4.yaml` | gemma-4-26B-A4B-it-qat (UD-Q4_K_XL) | 14.3GB |
| `qwen3.yaml` | Qwen3.5-9B (Q4_K_M) | 5.7GB |
| `lfm2.yaml` | LFM2.5-8B-A1B (Q4_K_M) | 5.2GB |

`gemma4.yaml` is the big one — it is a 26B mixture-of-experts (only ~4B active per token, but **all** weights must be resident). If it will not fit, swap `modelPath` for `hf:unsloth/gemma-4-E4B-it-GGUF/gemma-4-E4B-it-Q4_K_M.gguf` (~5.0GB, identical prompt format), or pick a smaller quant from the model's repo.
