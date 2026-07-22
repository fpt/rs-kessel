# Refactor: split kessel (VM + platform) from gallium (agent)

Direction, decided 2026-07.

## Status

**gallium side complete** (Phases 1–3), on branch `refactor/absorb-kessel-agent`
in `../rs-gallium`. Not yet committed.

- **Phase 1 done** — engine divergence upstreamed to rs-gallium (`QExperts`,
  `get_i64_array`, `lfm2moe_q.rs`, the gemma4 MoE rework). Engine crates build;
  30/31 tests pass. The 1 failure (`gemma4_gguf`, E4B dense → degenerate output)
  is **pre-existing on rs-gallium `main`** (identical output before the change),
  a latent dense-path/GGUF bug, not a regression.
- **Phase 2 done** — the agent core is ported into `gallium-agent`, replacing its
  older ancestors: `llm`, `llm_local` (llama.cpp), `llm_gallium`, `protocol`,
  `harmony`, `gemma`, `memory`, `skill`, `model_downloader`, `mcp*` (all 5),
  `github`, `react`, `tool`, `situation`, `state_updater`. `situation` travels
  with the core (the only agent-core→kessel-only coupling was
  `tool.rs`→`situation`). Builds in all three feature combos
  (`gallium` default / `--no-default-features` cloud / `--features local`);
  155 gallium-agent tests pass.
- **Phase 3 done** — `appserver/` moved in; `gallium-agent` binary gained the
  `app-server` subcommand. Verified: `gallium-agent app-server` answers the
  `initialize` / `account/read` JSON-RPC handshake. This is the ACP backend that
  **replaces `kessel-cli`**.

### Decisions made during the gallium port

- **Dropped UniFFI/Swift from `gallium-agent`.** It is now a headless lib+bin
  (no `staticlib`/`cdylib`, no `agent.udl`, no `uniffi-bindgen`), since kessel
  drives it over ACP, not in-process. Consequence: rs-gallium's own `swift/`
  voice frontend no longer builds (`make gen-uniffi` / `build-swift`) — it is
  superseded by kessel and should be removed in a cleanup pass. `cargo build
  --workspace` is unaffected (swift/ is not a cargo member).
- **`gallium` feature is default-on**, `local` (llama.cpp) is opt-in — matches
  rs-gallium's candle-first identity while keeping bare builds free of the C++
  build.
- **Left functional identity strings as "kessel"** (app-server `userAgent`
  `kessel/0.1.0`, MCP client/server names, HF User-Agent). Rename is a follow-up.

### Remaining follow-ups (gallium side)

- Runtime-validate LFM2.5 and Gemma-4 MoE inference through
  `gallium-agent`'s `create_provider` (code-complete, not yet run).
- Rename residual "kessel" identity strings.
- Remove rs-gallium's retired `swift/` frontend + its Makefile targets.

**kessel side — Phase 4 in progress** (uncommitted, on `docs/harness-direction`):

- **Phase 4a done** — `acp_client.rs`: an ACP client that spawns a backend
  (`gallium app-server` or `codex app-server`) and drives it a turn at a time,
  reusing the symmetric `appserver::rpc` transport. It sends
  `initialize`/`thread/start`/`turn/start` and handles inbound `item/tool/call`
  + approval requests, capturing the final `agentMessage`. Verified end-to-end
  against an in-process `AppServer` with a scripted provider, including a
  reentrant client-tool call. (Fixed a Drop-ordering deadlock: joining the
  reader while still holding the connection writer; now detach when there's no
  child to close the stream.)
- **Phase 4b done** — `vm_*` (and any `ToolHandler`, incl. capture) serve back
  to the backend verbatim via the `HandlerClientTool` adapter — no rewrite.
  `vm::tools::vm_tool_handlers()` exposes the set; `register_vm_tools` now builds
  on it. Verified a real `vm_reset` runs through the adapter. All 295 kessel-core
  tests pass.
- **Phase 4c done (Rust); Swift needs only a rebuild** — `Agent` is now
  **ACP-backed**, keeping the *exact same UDL surface* (`agent_new` + all `Agent`
  methods). `agent_new` spawns the backend (`gallium-agent app-server` by
  default; override via `KESSEL_ACP_BACKEND`), forwards model/API config as env,
  and serves the resident VM (`vm_*`), screen `capture`, `read_situation_messages`,
  and `suggest_next_check` back as client tools. `step`/`observe`/`evaluate_goal`
  drive backend turns (observe/eval on throwaway threads so they don't pollute
  history); goals, situation, backchannel, and the conversation mirror stay local.
  `config.mcp_servers` is forwarded via `thread/start`'s `config.mcp_servers`.
  Builds; all 295 kessel-core tests pass.
  - **Because the UDL is byte-identical, the Swift frontend needs no source
    changes** — it keeps calling `agent_new`/`agent.step`; it just spawns the
    backend at runtime. Not yet rebuilt/validated here (`swift build` +
    `gen_uniffi` needs the release dylib), and a real end-to-end run needs
    `gallium-agent` on PATH + a model/key.
  - Known degradations vs. in-process: `step` returns no keyword hints / token
    counts (backend returns text only); `step_with_allowed_tools`/`observe`
    can't restrict the backend's own tool set (advisory only).

- **Phase 5 done (kessel side)** — on branch
  `refactor/phase5-remove-inprocess-agent`. Deleted the vendored engine
  (`crates/gallium-core`, `crates/gallium-models`), the `crates/app` `kessel-cli`
  binary, and every now-dead in-process agent module from `crates/lib`:
  `react.rs`, `llm_local.rs`, `llm_gallium.rs`, `protocol.rs`, `harmony.rs`,
  `gemma.rs`, `model_downloader.rs`, `github.rs`, all four `mcp_client*/mcp_server*`,
  `appserver/server.rs` + `tools.rs` + `e2e_tests.rs`, and the empty
  `state_capsule.rs`. `llm.rs` shrank to the shared data types (`ChatMessage`,
  `ChatRole`, `TokenUsage`, `ImageContent`, `ToolDefinition`, `ToolCallInfo`) —
  the `LlmProvider`/`OpenAiProvider`/`create_provider` layer is gone. What stayed:
  `appserver/rpc.rs` (the symmetric transport `acp_client` reuses), `mcp.rs` (its
  JSON-RPC constants), `tool.rs` (the `ToolHandler`/`ToolRegistry` surface the VM,
  capture, and situation client tools build on), and the voice-orchestration
  layer (`goal`, `situation`, `state_updater`, `event_router`, `capture`). The
  lib `Cargo.toml` lost the `local`/`cuda`/`metal`/`vulkan`/`gallium` features and
  the llama.cpp/candle/hf-hub deps; the workspace is a single member (`lib`). The
  `acp_client` e2e test was rewritten to drive a hand-rolled JSON-RPC backend stub
  (the old fixture used the now-deleted in-process `AppServer`). `cargo
  build`/`--release` green; 206 lib + 21 game tests pass. Also fixed the stale
  default backend name (`gallium-agent` → `gallium`, matching rs-gallium's renamed
  binary).

- **Phase 6 done (kessel side)** — docs + scripts brought in line with the
  single-cdylib + external-`gallium`-backend world. `Makefile` rewritten (single
  `kessel` install, `run`/`run-codex`, no `kessel-cli`); `build-win-local.bat`
  reduced to a plain cdylib build and `build-win-cuda.bat` deleted (GPU inference
  lives in the backend); the agent-capability `testsuite/` and its driver scripts
  removed (they belong with the agent, in rs-gallium); the in-process agent test
  scripts and dead local-model/whisper helpers deleted; `configs/` trimmed to
  `gallium.yaml` (default) + `codex.yaml`; the Claude Code **watcher** removed
  end-to-end (Rust `event_router`/`feed_watcher_event`, the Swift `Watcher`
  package + wiring, hook scripts, the `claude-activity-report` skill, and
  `watcher:` config); CLAUDE.md/README rewritten. kessel and any downstream ACP
  driver are **independent** — no cross-repo coupling is documented or assumed.

---

## Original plan

## Goal

Stop maintaining two parallel agent stacks and one drifting vendored inference
engine. After the split:

- **kessel** = the luax **VM** + platform/frontend (voice TTS/STT, `PlayWindow`,
  Swift/C# apps). It is a **backend-agnostic ACP client** — it drives whatever
  app-server it's pointed at (`gallium` or `codex`). It has no agent loop and no
  local inference of its own.
- **gallium** = the agent: ReAct loop, tools, MCP, the llama.cpp **and** native
  candle local backends, and the **app-server (ACP)** kessel spawns. gallium's
  binary is that ACP server.
- **`kessel-cli` is deleted.** Its `app-server` role moved to gallium.

## Target topology

```
Swift / C# frontend  (kessel: VM, TTS/STT, watcher, PlayWindow)
   |  ACP / JSON-RPC over stdio           spawns + drives
   v
gallium app-server   (agent: react, tools, mcp, llama.cpp + candle)
   |  item/tool/call  (outbound, dynamicTools)
   v
kessel client tools:  vm_*, capture_screen/apply_ocr   (executed in the client)
```

Backend is swappable: `gallium` and `codex` both speak the same
codex-app-server subset kessel drives via `appserver/rpc`.

## Current state (why this is a refactor, not a rewrite)

Two axes of duplication exist today:

1. **The engine is vendored and has drifted.** `crates/gallium-core` +
   `crates/gallium-models` are a manual copy of rs-gallium's (no submodule, no
   sync). The copy is *ahead* of upstream:
   - `gallium-core`: kessel added `QExperts` (generic per-expert dequant for any
     GGML block quant; upstream only has MXFP4 `Tq2Tensor`).
   - `gallium-models`: kessel added `lfm2moe_q.rs` (LFM2.5 MoE) and reworked
     `gemma4_q.rs` (+366 lines — the 26B-A4B MoE variant). Upstream still has
     `gemma4_vision.rs`, which kessel dropped.

2. **The agent layer exists twice, sharing ancestry.** kessel's `lib` is the
   evolved superset of rs-gallium's `gallium-agent`:

   | module | gallium-agent | kessel lib | identical lines |
   |---|---|---|---|
   | `protocol.rs` | 1916 | 2122 | 1674 |
   | `tool.rs` | 741 | 2203 | 356 |
   | `react.rs` | 172 | 411 | 114 |
   | `provider`/`llm_gallium` | 181 | 565 | 163 |

   kessel's versions win — they carry appserver, the llama.cpp backend, MCP-HTTP,
   github, etc. that gallium-agent never had.

## Module disposition (kessel `crates/lib`, ~13.7k lines)

**MOVE → gallium-agent** (replaces the older ancestors):
`react.rs`, `tool.rs`, `llm.rs`, `llm_local.rs` (llama.cpp), `llm_gallium.rs`,
`protocol.rs`, `harmony.rs`, `gemma.rs`, `memory.rs`, `skill.rs`,
`model_downloader.rs`, `mcp*.rs` (all 5), `github.rs`, and all of `appserver/`
(**the ACP server that replaces kessel-cli**).

**STAY in kessel** (VM + platform):
`vm/` (~7.2k), `VmPlayer`, plus new client-tool executors.

**DECIDE — the voice-assistant orchestration layer**
(`goal.rs`, `situation.rs`, `state_updater.rs`, `event_router.rs`, `capture.rs`,
`process_backchannel`): not "agent core," not "VM." Default rec:
- `capture` → ACP **client tool** (tool-def registered by kessel; screen
  capture/OCR executor stays kessel/macOS-side).
- `goal` / `situation` / ambient loop / backchannel → **stay kessel-side** as
  client orchestration that drives the backend via ACP turns.
- `state_capsule.rs` is empty (0 lines) → delete.

## Phased plan

Ordering constraints: upstream the engine **before** deleting the vendored copy;
stand up the gallium backend **before** repointing the kessel client at it.

### Phase 1 — Upstream the engine divergence to rs-gallium
Port into rs-gallium so it is again the single source of truth:
- `QExperts` (`gallium-core/quantized.rs` + `lib.rs` export).
- `lfm2moe_q.rs` + its `pub mod` in `gallium-models/lib.rs`.
- the `gemma4_q.rs` MoE rework.
Verify rs-gallium builds and runs LFM2.5 and Gemma-4 MoE. Keep `gemma4_vision.rs`.

### Phase 2 — Port the agent core into gallium-agent
Bring kessel's agent modules into `gallium-agent`, replacing the ancestors.
Fast-forward `protocol.rs` (1674 shared lines). Add kessel-core's feature flags
to gallium-agent's Cargo (`local`, `cuda`, `metal`, `vulkan`, `gallium`). This is
the largest chunk.

### Phase 3 — Move appserver + wire the `app-server` subcommand
Move `appserver/` into gallium-agent; add the `app-server` dispatch to gallium's
`main.rs` (it already has a REPL main). gallium's binary now serves ACP.

### Phase 4 — Build the kessel ACP client
Swift/C# spawns the gallium (or codex) `app-server` and speaks JSON-RPC.
kessel registers `vm_*` + `capture` as `dynamicTools`; rewrite `vm/tools.rs`'s
registration surface as client-tool executors (VM logic untouched). Port
goals/situation/ambient/backchannel to client-side orchestration over ACP.

### Phase 5 — Delete from kessel
Remove `crates/app` (kessel-cli), `crates/gallium-core`, `crates/gallium-models`
(vendored), and every moved `lib` module. `kessel-core` shrinks to VM + VmPlayer
+ ACP client + executors; its UDL drops the agent-y `Agent` methods.

### Phase 6 — Docs + scripts
Update CLAUDE.md/README/Makefile and the `win/` build scripts for the
single-cdylib + external-backend world.

## Open questions / risks

- **Swift rewrite is the biggest user-facing change** — the full decouple
  (chosen) means every in-process `agent.*` call becomes ACP.
- **gallium's own UniFFI/Swift frontend** becomes optional once kessel talks to
  it over ACP instead of linking it — decide whether to keep it for standalone
  gallium use or drop it.
- **candle rev pin** must stay identical across the moved crates.
```
