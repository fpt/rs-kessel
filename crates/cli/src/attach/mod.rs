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

/// The "which session did you mean?" error.
///
/// Each line must be a command the user can actually run — see
/// `suggestions_are_commands_attach_accepts`, which feeds them back through the
/// real parser. This previously suggested `--root <path>`, which `attach`
/// rejects, so the printed recovery path could not work.
#[cfg(feature = "play")]
fn ambiguous_message(sessions: &[Session]) -> String {
    let list = sessions
        .iter()
        .map(|s| format!("  kessel attach {}", s.root))
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

    /// Every command the ambiguity error suggests must survive the real parser.
    ///
    /// This is the guard for the class of bug where the CLI grammar changes and
    /// an error message keeps recommending the old spelling — the message looks
    /// helpful, and following it fails.
    #[test]
    fn suggestions_are_commands_attach_accepts() {
        let msg = ambiguous_message(&[session("/work/one"), session("/work/two")]);

        let suggestions: Vec<&str> = msg
            .lines()
            .filter_map(|l| l.trim().strip_prefix("kessel attach "))
            .collect();
        assert_eq!(
            suggestions.len(),
            2,
            "both sessions should be offered: {msg}"
        );

        for s in suggestions {
            let args: Vec<String> = s.split_whitespace().map(String::from).collect();
            let parsed = crate::parse_attach(&args)
                .unwrap_or_else(|e| panic!("suggested `kessel attach {s}` is rejected: {e}"));
            assert_eq!(parsed, Some(std::path::PathBuf::from(s)));
        }
    }

    /// The roots have to appear, or the user can't tell the sessions apart.
    #[test]
    fn ambiguous_message_names_every_root() {
        let msg = ambiguous_message(&[session("/work/one"), session("/work/two")]);
        assert!(msg.contains("/work/one"), "{msg}");
        assert!(msg.contains("/work/two"), "{msg}");
    }
}
