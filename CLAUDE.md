# Kessel - Developer Guide

## Overview

A tiny fantasy console, shipped two ways from one binary:

- `kessel mcp` — an **MCP stdio server** serving the `vm_*` tools, so any
  MCP-capable agent can drive the write → assemble → run → observe → debug loop.
- `kessel play <file>` — a **window** (winit + softbuffer) for a human to play
  the result.

Kessel does no LLM inference and hosts no agent. It used to be a macOS/Windows
voice assistant that embedded the VM; that frontend and its ACP client were
removed, leaving only the console.

- **Rust**: workspace in `crates/`, two members — `vm` (`kessel-vm`) and `cli` (`kessel`)
- **Platforms**: macOS, Windows, Linux. `kessel mcp` is headless-safe.

## Architecture

```
Agent (Claude Code, codex, …)
  │  MCP over stdio (line-delimited JSON-RPC 2.0)
  ▼
kessel mcp ── crates/cli/src/mcp/ ── VmToolSet ──┐
                                                 ├─► VmConsole (crates/vm)
kessel play ── crates/cli/src/play.rs ── VmPlayer ┘      │
                  winit window, 60 Hz tick               ▼
                  softbuffer CPU blit          indexed framebuffer
                                               + sound event log
```

The load-bearing rule: **`kessel-vm` is host-free.** It does no I/O beyond the
files under its working directory, synthesizes no audio, and touches no GPU.
Drawing is a software rasterizer into an indexed framebuffer; sound is an event
log. Every backend (a window, an audio device, a future GPU upscaler) lives in
`crates/cli` or further out. This is what keeps the machine deterministic and
snapshotable — which is the entire point of the agent loop, since
`vm_snapshot`/`vm_restore` and frame-exact replay depend on it.

Corollary: if you are tempted to put wgpu, cpal, or any device backend into
`crates/vm`, don't. Put it in the player.

### `crates/vm` — `kessel-vm`

| File | Purpose |
|------|---------|
| `src/lib.rs` | `VmConsole` — the machine plus the authoring workspace (sources, ROMs, snapshots) and the `Observation` record. Disk-backed when a root is set. |
| `src/isa.rs` | The 34-opcode instruction set. |
| `src/vm.rs` | The stack machine: memory, stacks, fetch/execute, frame runner. |
| `src/device.rs` | Varvara-lite device layer — screen, gamepad, rng, storage, debug, console, sound (recorded only). |
| `src/assembler.rs` | Two-pass textual assembler → ROM + diagnostics. |
| `src/luax.rs` | The statically-typed Lua-ish front-end that compiles to assembly. |
| `src/png.rs` | Dependency-free PNG + base64 for framebuffer output. |
| `src/player.rs` | `VmPlayer` — a standalone handle for human play (load, tick, framebuffer_rgba). |
| `src/tool.rs` | The crate's own tool surface: `VmTool`, `ToolResult`, `ImageContent`, `VmToolError`. Deliberately not borrowed from any host framework. |
| `src/tools.rs` | The `vm_*` tools and `VmToolSet` (name-dispatch over the set). |

### `crates/cli` — `kessel`

| File | Purpose |
|------|---------|
| `src/main.rs` | Subcommand dispatch (`mcp`, `play`, help/version) and `--root` parsing. |
| `src/mcp/mod.rs` | The stdio read → dispatch → write loop. |
| `src/mcp/server.rs` | Method dispatch: `initialize`, `tools/list`, `tools/call`, `ping`. Pure function of request + VM state, so it tests without a process. |
| `src/mcp/wire.rs` | MCP / JSON-RPC wire types, including the `image` content block. |
| `src/play.rs` | winit window, 60 Hz tick, key→gamepad mapping, and `blit` (nearest-neighbour upscale to a `0RGB` CPU surface). |

### Key patterns

- **Tools are dynamically dispatched.** `Vec<Box<dyn VmTool>>` with hand-written
  JSON schemas, wrapped by `VmToolSet`. This is why the MCP layer is hand-rolled
  rather than using `rmcp`: that SDK's value is its `#[tool]` macros over typed
  Rust fns, which buys nothing here, and it would drag in tokio.
- **Tool failure ≠ protocol error.** A program that won't compile, faults, or
  halts is a *successful* `tools/call` whose text reports the failure — the model
  is meant to read it and debug. `VmToolError` (unknown tool, bad argument) comes
  back as `isError: true`. JSON-RPC errors are reserved for protocol faults.
- **The filesystem is the source of truth** when a root is set. `vm_write_source`
  writes through to disk and `vm_assemble` re-reads on every call, so the agent's
  own file-editing tools and the VM never diverge. In-memory sources exist only
  for `VmPlayer` and tests.
- **stdout is the MCP channel.** Every diagnostic in `kessel mcp` goes to stderr.
- **The `play` feature is default-on but removable.** `--no-default-features`
  drops winit/softbuffer for a headless `kessel mcp`.
- Prefer `vm_run_frames` over looping `vm_run_frame`: an MCP round trip per frame
  is pure overhead. It stops at the first fault/halt and caps at 1800 frames.

## Build & Run

```bash
cd crates && cargo build --release
cd crates && cargo test
cd crates && cargo build --release --no-default-features   # headless

./crates/target/release/kessel mcp --root /path/to/project
./crates/target/release/kessel play games/tetris.lua
```

`make install` builds release and copies `kessel` into `$PREFIX/bin` (default
`~/bin`). The binary is self-contained — no dylib, no separate backend process.

## Project Structure

```
kessel/
├── crates/vm/          kessel-vm: the console (host-free)
├── crates/cli/         kessel: `mcp` + `play`
├── games/              sample games / luax reference corpus
└── docs/VM.md          machine, ISA, devices, luax, agent loop
```

## Testing notes

- `crates/vm/tests/games_compile.rs` guards every file in `games/`: each must
  compile with no diagnostics and survive 300 frames under both idle and rotating
  button input without faulting. Sources are `include_str!`'d, so renaming a game
  breaks the build rather than silently skipping it.
- `crates/cli/src/mcp/server.rs` has a full write → assemble → load → run test
  over the MCP surface — the thing that actually has to work for a real host.
- `blit` in `play.rs` is tested separately (channel order, integer upscale,
  letterboxing, undersized window) because a wrong stride there produces a
  plausible-but-wrong picture rather than a crash.

## Troubleshooting

**`kessel play` reports diagnostics and exits**: the game didn't compile. That's
deliberate — a blank window would be worse. Fix the source and re-run, or keep
the window open and press `R` to reload.

**An agent can't see the VM tools**: check the agent spawned `kessel mcp` (not
bare `kessel`) and that stdout isn't being polluted — the protocol lives there.
