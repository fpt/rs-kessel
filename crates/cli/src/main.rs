//! `kessel` — the fantasy console, two ways.
//!
//! ```text
//! kessel mcp [--root <dir>]     # serve the vm_* tools to an agent over MCP
//! kessel run <file.lua|.asm>    # open a window and play a game yourself
//! kessel attach [workdir]        # join a running mcp session and play its VM
//! ```
//!
//! Both drive the same [`kessel_vm`] console; they differ only in who is at the
//! controls. The VM itself is host-free — it rasterizes into an indexed
//! framebuffer and records sound as events — so neither mode can change what the
//! machine computes, which is what keeps agent runs reproducible.

mod attach;
mod mcp;
#[cfg(feature = "play")]
mod play;

use std::path::PathBuf;

pub const NAME: &str = "kessel";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
kessel — a tiny fantasy console for agents and humans

USAGE:
    kessel mcp [--root <dir>]     Serve the VM to an agent as an MCP stdio server
    kessel run <file.lua|.asm>    Open a window and play a game on its own VM
    kessel attach [workdir]       Join a running `kessel mcp` and play ITS VM

OPTIONS:
    --root <dir>    For `mcp`: the working directory holding the game sources the
                    VM compiles (default: the current directory). Sources are read
                    from and written to real files here, so an agent's own
                    file-editing tools and the VM see the same game.

RUN vs ATTACH:
    `kessel run` plays a file on a VM of its own — nothing else can see or
    disturb it.

    `kessel attach` joins a running `kessel mcp` and drives the agent's own
    machine, so you can play the work in progress. You share one timeline with
    the agent: its vm_snapshot/vm_restore/vm_reset will rewind the game under
    you, vm_run_frames advances it in bursts, and your button presses show up in
    its observations. Give it a workdir to pick between several live sessions;
    with none it prefers the session rooted here.

CONTROLS:
    Arrows / WASD   D-pad        Z or J   A        X or K   B
    Enter           START        Shift    SELECT
    R               reload from disk      Esc      quit
                    (`run` only — when attached, the agent owns what's loaded)

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
        Some("run") => run_play(parse_run(&args[1..])?),
        Some("attach") => run_attach(parse_attach(&args[1..])?),
        // `play` was split into `run` and `attach`; name both rather than
        // leaving someone with muscle memory at a bare "unknown command".
        Some("play") => Err(
            "`kessel play` was split in two, so it is obvious which VM you get:\n\
             \n    kessel run <file.lua>   play a file on its own VM\
             \n    kessel attach [workdir] join a running `kessel mcp` and play its VM"
                .to_string(),
        ),
        // A bare path is almost certainly a run attempt; say so rather than
        // dumping the whole usage block on someone who nearly had it right.
        Some(other) => Err(format!(
            "unknown command '{other}' — expected `mcp`, `run`, or `attach`\n\n\
             Did you mean: kessel run {other}"
        )),
    }
}

/// Reject any option before looking at arity, so `attach --root /tmp` complains
/// about `--root` rather than about the path that follows it.
fn reject_options(args: &[String]) -> Result<(), String> {
    match args.iter().find(|a| a.starts_with('-')) {
        Some(opt) => Err(format!("unexpected option '{opt}'")),
        None => Ok(()),
    }
}

/// Parse `run`'s arguments: exactly one file, no options.
fn parse_run(args: &[String]) -> Result<PathBuf, String> {
    reject_options(args)?;
    match args {
        [] => Err(
            "`kessel run` needs a file, e.g. `kessel run games/bounce.lua`\n\n\
                   To play a game an agent is building, use `kessel attach` instead."
                .to_string(),
        ),
        [one] => Ok(PathBuf::from(one)),
        [_, extra, ..] => Err(format!("unexpected argument '{extra}'")),
    }
}

/// Parse `attach`'s arguments: an optional workdir naming which session to join.
///
/// It is positional rather than a `--root` flag so the two commands read the
/// same way — `kessel run <file>` / `kessel attach <workdir>` — and so it is
/// obvious at a glance which VM a given invocation is driving.
fn parse_attach(args: &[String]) -> Result<Option<PathBuf>, String> {
    reject_options(args)?;
    match args {
        [] => Ok(None),
        [one] => Ok(Some(PathBuf::from(one))),
        [_, extra, ..] => Err(format!("unexpected argument '{extra}'")),
    }
}

#[cfg(feature = "play")]
fn run_play(path: PathBuf) -> Result<(), String> {
    play::run(path)
}

#[cfg(feature = "play")]
fn run_attach(root: Option<PathBuf>) -> Result<(), String> {
    play::run_attached(root.as_deref())
}

#[cfg(not(feature = "play"))]
fn run_play(_path: PathBuf) -> Result<(), String> {
    Err(NO_PLAYER.to_string())
}

#[cfg(not(feature = "play"))]
fn run_attach(_root: Option<PathBuf>) -> Result<(), String> {
    Err(NO_PLAYER.to_string())
}

#[cfg(not(feature = "play"))]
const NO_PLAYER: &str = "this build has no player (compiled with --no-default-features); \
                         `kessel mcp` is available";

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

    #[test]
    fn run_takes_exactly_one_file() {
        assert_eq!(
            parse_run(&["games/bounce.lua".into()]).unwrap(),
            PathBuf::from("games/bounce.lua")
        );
        assert!(parse_run(&["a.lua".into(), "b.lua".into()]).is_err());
        assert!(parse_run(&["--root".into()]).is_err());
    }

    /// `kessel run` with no file used to mean "attach". Now that attaching has
    /// its own command, the bare form is a mistake — and the message should say
    /// which command the user probably wanted.
    #[test]
    fn run_without_a_file_points_at_attach() {
        let err = parse_run(&[]).unwrap_err();
        assert!(err.contains("kessel attach"), "{err}");
    }

    #[test]
    fn attach_workdir_is_optional_and_positional() {
        assert_eq!(parse_attach(&[]).unwrap(), None);
        assert_eq!(
            parse_attach(&["./my-game".into()]).unwrap(),
            Some(PathBuf::from("./my-game"))
        );
        assert!(parse_attach(&["a".into(), "b".into()]).is_err());
    }

    /// The workdir is positional now; a stray `--root` should say so rather than
    /// being silently swallowed as a path.
    #[test]
    fn attach_rejects_a_root_flag() {
        let err = parse_attach(&["--root".into(), "/tmp".into()]).unwrap_err();
        assert!(err.contains("--root"), "{err}");
    }
}
