//! Attaching a play window to a running `kessel mcp`.
//!
//! `kessel attach` joins a live MCP session and drives **the agent's
//! own `VmConsole`** — one machine, one timeline, two drivers. That is the
//! literal shared session, and it has consequences worth stating plainly:
//!
//! - The agent's `vm_snapshot`/`vm_restore`/`vm_reset` rewind the game under
//!   the player.
//! - `vm_run_frames` advances the machine in bursts the player didn't ask for.
//! - The player's button presses land in the agent's observations, so a run
//!   with someone attached is not reproducible.
//!
//! None of that is a bug to be fixed here — it is what sharing one machine
//! means. `kessel run <file>` remains fully independent for when you want your
//! own timeline.
//!
//! The transport is loopback TCP rather than a Unix socket so the same code path
//! works on Windows, and it carries a small binary protocol rather than JSON
//! because it is a 60 Hz stream of framebuffers.

// The server half is always built: `kessel mcp` publishes it even in a headless
// build, so a player elsewhere can still join. The client half is the window's,
// and compiles only with it.
pub mod protocol;
pub mod server;
pub mod session;

#[cfg(feature = "play")]
pub mod client;
#[cfg(feature = "play")]
pub use client::AttachClient;
#[cfg(feature = "play")]
pub use session::{discover, Discovery, Session};

#[cfg(feature = "play")]
/// Whether a session is actually reachable. Used as the liveness probe for
/// [`discover`] — connecting is the only honest test, since a pid can be reused
/// and a crashed server leaves its file behind.
pub fn is_live(session: &Session) -> bool {
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::time::Duration;

    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, session.port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
}

#[cfg(feature = "play")]
/// Find a session to attach to and connect, turning every failure into
/// something a person can act on.
pub fn attach(root: Option<&std::path::Path>) -> Result<AttachClient, String> {
    match discover(root, is_live) {
        Discovery::Found(s) => {
            let client = AttachClient::connect(&s)?;
            eprintln!("kessel attach: joined the session at {}", s.root);
            Ok(client)
        }
        Discovery::None => Err("no running `kessel mcp` to attach to.\n\n\
             Start one (or let your agent start it), then run `kessel attach` again — \
             or play a file on its own timeline with `kessel run <file.lua>`."
            .to_string()),
        Discovery::Ambiguous(sessions) => Err(ambiguous_message(&sessions)),
    }
}

/// Quote a path for pasting into a shell.
///
/// Unconditional double quotes rather than quote-only-when-needed: they are
/// understood identically by sh/bash/zsh, cmd.exe and PowerShell, so there is
/// one rule and no per-platform list of metacharacters to get subtly wrong. A
/// workdir with a space in it is ordinary on macOS and near-universal on
/// Windows (`C:\Users\Me\My Games`), and unquoted it would reach `attach` as
/// two arguments.
///
/// Backslashes are left alone — they are the Windows path separator and are
/// literal inside double quotes there. Only an embedded `"` needs escaping, and
/// that character is illegal in Windows paths and pathological elsewhere.
#[cfg(feature = "play")]
fn shell_quote(path: &str) -> String {
    format!("\"{}\"", path.replace('"', "\\\""))
}

/// The "which session did you mean?" error.
///
/// Each line must be a command the user can actually run, which has now been
/// wrong twice: first suggesting the `--root` flag that `attach` rejects, then
/// printing paths unquoted so any workdir with a space became two arguments.
/// `suggestions_survive_a_shell_and_the_parser` guards both by pasting the
/// suggestions through a shell-like splitter and into the real parser.
#[cfg(feature = "play")]
fn ambiguous_message(sessions: &[Session]) -> String {
    let list = sessions
        .iter()
        .map(|s| format!("  kessel attach {}", shell_quote(&s.root)))
        .collect::<Vec<_>>()
        .join("\n");
    format!("several `kessel mcp` sessions are running; say which one:\n{list}")
}

#[cfg(all(test, feature = "play"))]
mod tests {
    use super::*;

    fn session(root: &str) -> Session {
        Session {
            port: 1234,
            root: root.to_string(),
            pid: 1,
            version: "0.1.0".into(),
        }
    }

    /// Split a command line the way a shell would, honouring double quotes.
    /// Enough to model someone copy-pasting our suggestion into a terminal.
    fn shell_split(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut started = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    quoted = !quoted;
                    started = true;
                }
                '\\' if quoted && chars.peek() == Some(&'"') => {
                    chars.next();
                    cur.push('"');
                }
                c if c.is_whitespace() && !quoted => {
                    if started || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                c => cur.push(c),
            }
        }
        if started || !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// Every command the ambiguity error suggests must survive a shell *and*
    /// the real parser, and arrive as the workdir we meant.
    ///
    /// This has been wrong twice — once recommending a removed flag, once
    /// printing paths unquoted so a workdir with a space split into two
    /// arguments. Both are the same class: an error message that documents a
    /// grammar it is not checked against. Hence a root with a space here.
    #[test]
    fn suggestions_survive_a_shell_and_the_parser() {
        let roots = ["/work/one", "/work/my game", "/work/two words/deep"];
        let sessions: Vec<Session> = roots.iter().map(|r| session(r)).collect();
        let msg = ambiguous_message(&sessions);

        let lines: Vec<&str> = msg
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("kessel attach"))
            .collect();
        assert_eq!(
            lines.len(),
            roots.len(),
            "every session should be offered: {msg}"
        );

        for (line, expected) in lines.iter().zip(roots) {
            let argv = shell_split(line);
            assert_eq!(&argv[..2], ["kessel", "attach"], "from {line:?}");
            let parsed = crate::parse_attach(&argv[2..])
                .unwrap_or_else(|e| panic!("suggested {line:?} is rejected: {e}"));
            assert_eq!(
                parsed,
                Some(std::path::PathBuf::from(expected)),
                "suggested {line:?} did not round-trip to its workdir"
            );
        }
    }

    /// The splitter above is only trustworthy if it actually splits on spaces —
    /// otherwise the test could pass by never exercising the quoting.
    #[test]
    fn shell_split_models_word_splitting() {
        assert_eq!(
            shell_split("kessel attach /a b"),
            ["kessel", "attach", "/a", "b"]
        );
        assert_eq!(
            shell_split("kessel attach \"/a b\""),
            ["kessel", "attach", "/a b"]
        );
    }

    /// The roots have to appear, or the user can't tell the sessions apart.
    #[test]
    fn ambiguous_message_names_every_root() {
        let msg = ambiguous_message(&[session("/work/one"), session("/work/two")]);
        assert!(msg.contains("/work/one"), "{msg}");
        assert!(msg.contains("/work/two"), "{msg}");
    }
}
