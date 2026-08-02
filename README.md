# Kessel

A tiny **fantasy console** for AI agents and humans.

Kessel gives a model a real machine to write games for: a 16-bit stack VM with a
128×128 screen, a gamepad, and a statically-typed Lua-ish language that compiles
to it. The model writes a game, assembles it, runs frames, looks at the screen,
and debugs — then you play the result in a window.

```bash
kessel mcp                      # serve the console to an agent over MCP
kessel play games/tetris.lua    # play a game yourself
```

## Why

Most "let an LLM make a game" setups hand the model a browser and hope. Kessel
hands it a machine small enough to reason about completely — 34 opcodes, a flat
64 KiB address space, a fixed palette — and a feedback loop that actually closes:
every frame returns an observation (screen hash, changed-pixel bbox, stack state,
faults, game-reported entities) and the screen itself comes back as a PNG.

The VM is **deterministic and snapshotable**. Drawing is a software rasterizer
into an indexed framebuffer; sound is an event log, not audio. That is what makes
`vm_snapshot`/`vm_restore` and frame-exact replays possible, and it means the
console runs anywhere — CI, a container, a headless box — with no GPU.

## Install

```bash
cargo install --path crates/cli     # installs `kessel`
```

Or build in place:

```bash
make build          # cargo build --release
make install        # -> ~/bin/kessel (override with PREFIX=/usr/local)
make test
```

For a server or container with no window system, drop the player:

```bash
cargo build --release --no-default-features   # `kessel mcp` only
```

## Use it with an agent

`kessel mcp` is an MCP stdio server. Register it with any MCP-capable agent:

```json
{
  "mcpServers": {
    "kessel": { "command": "kessel", "args": ["mcp", "--root", "/path/to/project"] }
  }
}
```

With Claude Code, that's:

```bash
claude mcp add kessel -- kessel mcp --root .
```

The console is rooted at `--root` (default: the current directory) and **the
filesystem is the source of truth**. `vm_write_source` writes a real `game.lua`
there and `vm_assemble` re-reads it on every call — so the agent can equally well
edit the file with its own editing tools and just call `vm_assemble`. No stale
in-memory copy to get out of sync.

### The tools

| Tool | What it does |
|------|--------------|
| `vm_write_source` | Write a `.lua` (luax) or `.asm` source file |
| `vm_assemble` | Compile + assemble it to a ROM, with line-numbered diagnostics |
| `vm_load_rom` | Load a ROM and run its reset vector |
| `vm_run_frames` | Play an input script for many frames — **the one to reach for** |
| `vm_run_frame` | Advance a single frame with buttons held |
| `vm_run_cycles` | Step N instructions (fine-grained debugging) |
| `vm_get_framebuffer` | The screen as a PNG, so the model can *see* it |
| `vm_inspect_memory` / `vm_inspect_stacks` | Hex dump / stack + pc + fault state |
| `vm_snapshot` / `vm_restore` | Save and rewind machine state |
| `vm_reset` | Reset the machine |

Prefer `vm_run_frames`: it plays a whole scenario in one call and reports the
frames run, any fault that stopped it early, every sound trigger, and how often
the screen changed — an MCP round trip per frame is pure overhead.

## Play

```bash
kessel play games/2048.lua      # arrows slide tiles, A starts a new game
kessel play games/tetris.lua    # L/R move, A rotates, Down soft-drops
kessel play games/platform.lua  # arrows move, A jumps and wall-jumps
kessel play games/outrun.lua    # pseudo-3D road racer
```

Arrows or WASD for the d-pad, `Z`/`X` for A/B, Return for START, Shift for
SELECT. `R` reloads the file from disk so you can edit and re-run without
leaving the window; Esc quits.

The `games/` directory doubles as worked luax examples covering the whole
language — sprite declarations, tilemaps and `solid()` collision, entity pools,
edge-triggered input, fixed-point trig. Point a model at them.

## Write a game

```lua
sprite hero {
  ..7777..
  .777777.
  77777777
  77.77.77
  77777777
  .777777.
  ..7777..
  .77..77.
}

local x = 60

function update()
  if btn(LEFT)  then x = x - 1 end
  if btn(RIGHT) then x = x + 1 end
end

function draw()
  cls(0)
  spr(hero, x, 60, 0)
  entity(x, 60, 1)   -- reported back in the observation
end
```

luax is Lua-*flavored*, not Lua: statically typed, no tables, closures, or
recursion. It exists because models have strong Lua priors from PICO-8 and
TIC-80, and reusing those priors beats teaching a new syntax. See
**[docs/VM.md](docs/VM.md)** for the full language, instruction set, and device
map.

## Layout

```
crates/vm/     kessel-vm — the console: ISA, VM, assembler, luax, PNG, vm_* tools.
               Host-free: no I/O beyond the working directory, no audio, no GPU.
crates/cli/    kessel — the binary. `mcp` (stdio server) and `play` (winit window).
games/         Sample games, and the luax reference corpus.
docs/VM.md     Machine, instruction set, devices, luax, and the agent loop.
```

## License

MIT OR Apache-2.0. See [LICENSE.txt](LICENSE.txt).
