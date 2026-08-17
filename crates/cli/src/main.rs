//! `kessel` — the fantasy console, two ways.
//!
//! ```text
//! kessel mcp [--root <dir>]     # serve the vm_* tools to an agent over MCP
//! kessel run <file.lua|.asm>    # open a window and play a game yourself
//! kessel attach [workdir]        # join a running mcp session and play its VM
//! kessel render-audio <file>    # render a game's sound to a .wav, headless
//! ```
//!
//! Both drive the same [`kessel_vm`] console; they differ only in who is at the
//! controls. The VM itself is host-free — it rasterizes into an indexed
//! framebuffer and records sound as events — so neither mode can change what the
//! machine computes, which is what keeps agent runs reproducible.

mod attach;
#[cfg(feature = "audio")]
mod audio;
mod mcp;
#[cfg(feature = "play")]
mod play;
mod render_audio;

use std::path::{Path, PathBuf};

pub const NAME: &str = "kessel";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
kessel — a tiny fantasy console for agents and humans

USAGE:
    kessel mcp [--root <dir>]     Serve the VM to an agent as an MCP stdio server
    kessel run <file.lua|.asm>    Open a window and play a game on its own VM
    kessel attach [workdir]       Join a running `kessel mcp` and play ITS VM
    kessel render-audio <file>    Render a game's sound to a .wav (no window,
                                  no audio device — works headless and over ssh)

OPTIONS:
    --root <dir>    For `mcp`: the working directory holding the game sources the
                    VM compiles. Sources are read from and written to real files
                    here, so an agent's own file-editing tools and the VM see the
                    same game. Also settable as $KESSEL_ROOT, for hosts whose
                    config can set env but not a working directory.

                    Optional. Without it the current directory is used when it
                    looks like a project someone chose, and ~/Documents/Kessel
                    when it doesn't — a desktop app launched from Finder starts
                    in `/` or its own bundle, which is no place to save a game.

                    An agent can also just name an ABSOLUTE path in vm_write_source
                    and the workspace moves there, so one registered server serves
                    a different project each session. Adopted directories are
                    confined to the root above and your home.

    For `render-audio`:
    --frames, -n <n>   How many frames to run (default 180 = 3 seconds)
    --out, -o <file>   Where to write the .wav (default: <name>.wav here)
    --buttons <list>   Buttons held for the whole run, e.g. A,RIGHT — a sound
                       behind a button needs the button pressed

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

`--root` can be left out — useful for a desktop host with one static config, where
each session's workspace is whichever directory the agent writes into.
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
            let adoptable = adoptable_roots(&root);
            mcp::run(root, adoptable);
            Ok(())
        }
        Some("run") => run_play(parse_run(&args[1..])?),
        Some("attach") => run_attach(parse_attach(&args[1..])?),
        Some("render-audio") => render_audio::run(render_audio::parse(&args[1..])?),
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
            "unknown command '{other}' — expected `mcp`, `run`, `attach`, or \
             `render-audio`\n\n\
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

/// Parse `mcp`'s options. The root is created if absent, so pointing an agent at
/// a fresh project directory just works.
///
/// Three sources, in order: `--root`, `$KESSEL_ROOT`, then [`default_root`]. The
/// environment variable is there for hosts whose config can set `env` but whose
/// working directory you don't control — a desktop app launched from Finder.
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

    let root = match root.or_else(|| env_dir("KESSEL_ROOT")) {
        Some(r) => r,
        None => default_root(std::env::current_dir().ok(), home()),
    };
    std::fs::create_dir_all(&root).map_err(|e| format!("create root '{}': {e}", root.display()))?;
    // Canonical from here on: the session file is keyed off this path, and the
    // console compares it against directories the model names absolutely.
    Ok(root.canonicalize().unwrap_or(root))
}

/// A non-empty environment variable as a path. Empty means unset — an
/// exported-but-empty variable would otherwise resolve to the relative path "".
fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The user's home, from the environment — the same shape `attach::session` uses.
fn home() -> Option<PathBuf> {
    env_dir(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
}

/// Where `kessel mcp` works when nothing says otherwise.
///
/// The cwd is right for an agent launched inside a project (`claude`, `codex`)
/// and wrong for a desktop app launched from Finder, whose cwd is `/` or its own
/// bundle: unwritable, invisible to the user, and not a place a game should be
/// saved. Fall back to a real directory under the user's home in that case.
fn default_root(cwd: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    match cwd {
        Some(dir) if is_plausible_workspace(&dir) => dir,
        _ => base_dir(home),
    }
}

/// A cwd worth working in: a directory someone chose, not the filesystem root
/// and not inside a macOS app bundle.
fn is_plausible_workspace(dir: &Path) -> bool {
    dir.parent().is_some()
        && !dir.components().any(|c| {
            std::path::Path::new(c.as_os_str())
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("app"))
        })
}

/// The fallback workspace: `~/Documents/Kessel` when a `Documents` exists (the
/// desktop case this is for — somewhere the user can actually find the game),
/// else `~/Kessel`, else a temp directory on a system with no home at all.
fn base_dir(home: Option<PathBuf>) -> PathBuf {
    match home {
        Some(h) if h.join("Documents").is_dir() => h.join("Documents").join("Kessel"),
        Some(h) => h.join("Kessel"),
        None => std::env::temp_dir().join("kessel"),
    }
}

/// The directories the model may move the workspace into by naming an absolute
/// path (see `VmConsole::set_adoptable_roots`): the configured root — the user's
/// explicit choice — and the user's home.
///
/// Deliberately not `/`. A per-session workspace is worth having, but a host
/// approves `vm_write_source` once by name, and that approval must not become a
/// licence to write anywhere on the disk.
fn adoptable_roots(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];
    if let Some(h) = home() {
        dirs.push(h);
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_defaults_to_the_cwd() {
        let root = parse_root(&[]).unwrap();
        // Canonical on both sides: `parse_root` resolves the path it returns, and
        // a test run under a symlinked checkout would otherwise fail here.
        assert_eq!(
            root,
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
    }

    /// The Claude Desktop case: launched from Finder, the cwd is `/` or the app's
    /// own bundle. Working there writes a game somewhere unwritable and invisible,
    /// so the fallback has to take over.
    #[test]
    fn a_desktop_apps_cwd_is_not_used_as_a_workspace() {
        let home = PathBuf::from("/Users/someone");
        for cwd in [
            PathBuf::from("/"),
            PathBuf::from("/Applications/Claude.app/Contents/MacOS"),
        ] {
            assert_eq!(
                default_root(Some(cwd.clone()), Some(home.clone())),
                base_dir(Some(home.clone())),
                "cwd {} should not be a workspace",
                cwd.display()
            );
        }
    }

    /// ...while a real project directory is still exactly where an agent launched
    /// inside it should work, with no flag at all.
    #[test]
    fn a_project_cwd_is_used_as_the_workspace() {
        let cwd = PathBuf::from("/Users/someone/code/my-game");
        assert_eq!(
            default_root(Some(cwd.clone()), Some(PathBuf::from("/Users/someone"))),
            cwd
        );
    }

    /// The fallback should land somewhere a person can find, and must not depend
    /// on a `Documents` that isn't there.
    #[test]
    fn the_fallback_workspace_follows_the_home_it_is_given() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            base_dir(Some(home.path().to_path_buf())),
            home.path().join("Kessel")
        );

        std::fs::create_dir(home.path().join("Documents")).unwrap();
        assert_eq!(
            base_dir(Some(home.path().to_path_buf())),
            home.path().join("Documents").join("Kessel")
        );

        // No home at all (a service account, a stripped container) still resolves.
        assert!(base_dir(None).is_absolute());
    }

    /// The model chooses the workspace now, so this list is the whole boundary:
    /// it must contain the places a game plausibly lives and nothing wider.
    #[test]
    fn adoptable_roots_cover_the_root_and_the_home_only() {
        let root = PathBuf::from("/Users/someone/code/my-game");
        let dirs = adoptable_roots(&root);
        assert!(dirs.contains(&root));
        assert!(
            !dirs.iter().any(|d| d == Path::new("/")),
            "adoption must not be allowed to reach the whole filesystem"
        );
        if let Some(h) = home() {
            assert!(dirs.contains(&h), "the user's home should be adoptable");
        }
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
