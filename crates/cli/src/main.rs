//! `kessel` — the fantasy console, two ways.
//!
//! ```text
//! kessel mcp [--root <dir>]     # serve the vm_* tools to an agent over MCP
//! kessel play <file.lua|.asm>   # open a window and play a game yourself
//! ```
//!
//! Both drive the same [`kessel_vm`] console; they differ only in who is at the
//! controls. The VM itself is host-free — it rasterizes into an indexed
//! framebuffer and records sound as events — so neither mode can change what the
//! machine computes, which is what keeps agent runs reproducible.

mod mcp;
#[cfg(feature = "play")]
mod play;

use std::path::PathBuf;

pub const NAME: &str = "kessel";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
kessel — a tiny fantasy console for agents and humans

USAGE:
    kessel mcp [--root <dir>]      Serve the VM to an agent as an MCP stdio server
    kessel play <file.lua|.asm>    Open a window and play a game

MCP OPTIONS:
    --root <dir>    Working directory holding the game sources the VM compiles
                    (default: the current directory). Sources are read from and
                    written to real files here, so an agent's own file-editing
                    tools and the VM see the same game.

PLAY CONTROLS:
    Arrows / WASD   D-pad        Z or J   A        X or K   B
    Enter           START        Shift    SELECT
    R               reload from disk      Esc      quit

Register the MCP server with any MCP-capable agent, e.g.:

    {\"command\": \"kessel\", \"args\": [\"mcp\", \"--root\", \"/path/to/project\"]}
";

fn main() {
    if let Err(e) = run() {
        eprintln!("kessel: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        None | Some("-h") | Some("--help") | Some("help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("-V") | Some("--version") | Some("version") => {
            println!("{NAME} {VERSION}");
            Ok(())
        }
        Some("mcp") => {
            let root = parse_root(&args[1..])?;
            mcp::run(root);
            Ok(())
        }
        Some("play") => {
            let path = args
                .get(1)
                .ok_or("`kessel play` needs a file, e.g. `kessel play games/bounce.lua`")?;
            if args.len() > 2 {
                return Err(format!("unexpected argument '{}'", args[2]));
            }
            run_play(PathBuf::from(path))
        }
        // A bare path is almost certainly a play attempt; say so rather than
        // dumping the whole usage block on someone who nearly had it right.
        Some(other) => Err(format!(
            "unknown command '{other}' — expected `mcp` or `play`\n\n\
             Did you mean: kessel play {other}"
        )),
    }
}

#[cfg(feature = "play")]
fn run_play(path: PathBuf) -> Result<(), String> {
    play::run(path)
}

#[cfg(not(feature = "play"))]
fn run_play(_path: PathBuf) -> Result<(), String> {
    Err(
        "this build has no player (compiled with --no-default-features); \
         `kessel mcp` is available"
            .to_string(),
    )
}

/// Parse `mcp`'s options. The root defaults to the cwd and is created if absent,
/// so pointing an agent at a fresh project directory just works.
fn parse_root(args: &[String]) -> Result<PathBuf, String> {
    let mut root: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--root" => {
                let dir = it.next().ok_or("--root needs a directory")?;
                root = Some(PathBuf::from(dir));
            }
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }

    let root = match root {
        Some(r) => r,
        None => std::env::current_dir().map_err(|e| format!("no --root and no cwd: {e}"))?,
    };
    std::fs::create_dir_all(&root).map_err(|e| format!("create root '{}': {e}", root.display()))?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_defaults_to_the_cwd() {
        let root = parse_root(&[]).unwrap();
        assert_eq!(root, std::env::current_dir().unwrap());
    }

    #[test]
    fn root_is_created_when_missing() {
        let dir = std::env::temp_dir().join(format!("kessel-root-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let out = parse_root(&["--root".into(), dir.display().to_string()]).unwrap();
        assert!(out.is_dir(), "root should have been created");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn root_without_a_value_is_an_error() {
        assert!(parse_root(&["--root".into()]).is_err());
    }

    #[test]
    fn unknown_mcp_flag_is_rejected() {
        let err = parse_root(&["--frames".into()]).unwrap_err();
        assert!(err.contains("--frames"), "{err}");
    }
}
