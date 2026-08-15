//! The attach listener that runs inside `kessel mcp`.
//!
//! Binds a loopback port, publishes a [`Session`] file, and serves one attached
//! player at a time. Every TICK locks the *same* `VmConsole` the `vm_*` tools
//! drive, so the player and the agent share one machine and one timeline.
//!
//! Two properties are load-bearing:
//!
//! - **Loopback only.** It binds `127.0.0.1`, never `0.0.0.0` — this hands
//!   arbitrary control of a VM and a view of the screen to whoever connects, and
//!   that must not be reachable off-box.
//! - **Client-driven.** The server never ticks on its own. With no player
//!   attached the machine advances only through tool calls, exactly as before.

use std::io::{BufReader, BufWriter};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use kessel_vm::Shared;

use super::protocol::{Frame, Hello, Tick, MSG_HELLO, MSG_TICK, PROTOCOL_VERSION};
use super::session::Session;

/// A published listener. Dropping it removes the session file, so a clean exit
/// leaves nothing for the next `kessel attach` to trip over.
pub struct AttachServer {
    session_path: PathBuf,
    /// Ours, so [`Session::unpublish`] can tell our advertisement from a newer
    /// server's that reused the same filename.
    port: u16,
}

impl Drop for AttachServer {
    fn drop(&mut self) {
        let _ = Session::unpublish(&self.session_path, self.port);
    }
}

/// The address the listener binds.
///
/// Loopback, and port 0 so the OS picks a free one (the session file carries the
/// result, so there is no fixed port to collide with another project's server).
/// Kept as its own function because the loopback part is a security property
/// worth a test, not just a literal.
fn bind_addr() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
}

/// Start accepting players on loopback, sharing `console`.
///
/// Returns `None` (after warning on stderr) if the port or session file can't be
/// claimed — an agent's MCP session must not die just because the optional play
/// bridge couldn't start.
pub fn start(console: Shared, root: &Path) -> Option<AttachServer> {
    start_in(&super::session::session_dir(), console, root)
}

/// As [`start`], publishing into an explicit session directory. Split out so
/// tests get their own directory instead of racing on a process-wide variable.
pub fn start_in(session_dir: &Path, console: Shared, root: &Path) -> Option<AttachServer> {
    let listener = match TcpListener::bind(bind_addr()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("kessel mcp: attach unavailable (bind: {e})");
            return None;
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            eprintln!("kessel mcp: attach unavailable (local_addr: {e})");
            return None;
        }
    };
    let session_path = match Session::publish_in(session_dir, root, port) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kessel mcp: attach unavailable (session file: {e})");
            return None;
        }
    };

    // One player at a time — two humans plus an agent on one timeline is a
    // problem this design already has enough of. Connections are still handled
    // on their own threads so a second `kessel attach` gets an immediate refusal
    // instead of sitting in the accept backlog, and so a liveness probe never
    // waits behind an active player.
    let taken = Arc::new(AtomicBool::new(false));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    let console = console.clone();
                    let taken = taken.clone();
                    std::thread::spawn(move || serve_player(&console, s, &taken));
                }
                Err(e) => {
                    eprintln!("kessel mcp: attach accept failed: {e}");
                    return;
                }
            }
        }
    });

    eprintln!("kessel mcp: attach ready on 127.0.0.1:{port} — run `kessel attach` to join");
    Some(AttachServer { session_path, port })
}

/// Serve one attached player until it disconnects.
fn serve_player(console: &Shared, stream: TcpStream, taken: &AtomicBool) {
    // Nagle would batch our tiny TICK requests and add latency to a 60 Hz loop.
    let _ = stream.set_nodelay(true);
    let peer = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());
    // Discovery probes the port to test liveness, so a bare connection is not a
    // player. Only announce one once it has actually said HELLO, or the log
    // reports an attach every time someone runs `kessel attach` elsewhere.
    let mut announced = false;
    // Claimed on HELLO, released when this connection ends.
    let mut holds_slot = false;

    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("kessel mcp: attach clone failed: {e}");
            return;
        }
    });
    let mut writer = BufWriter::new(stream);

    loop {
        use std::io::Read;
        let mut op = [0u8; 1];
        if reader.read_exact(&mut op).is_err() {
            break; // player went away
        }

        match op[0] {
            MSG_HELLO => {
                // Claim the single player slot, unless someone already has it.
                let busy = !holds_slot
                    && taken
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_err();
                if !busy {
                    holds_slot = true;
                }

                let (dim, controls) = {
                    let c = console.lock();
                    (c.screen_dim() as u16, c.controls().to_json().to_string())
                };
                let hello = Hello {
                    version: PROTOCOL_VERSION,
                    busy,
                    dim,
                    controls_json: controls,
                };
                let _ = hello.write(&mut writer);
                if busy {
                    eprintln!("kessel mcp: refused a second player ({peer}) — one at a time");
                    break;
                }
                if !announced {
                    eprintln!("kessel mcp: player attached ({peer})");
                    announced = true;
                }
            }
            MSG_TICK => {
                // A TICK advances the shared machine, so it is only honoured
                // from a connection that completed a non-busy HELLO. Without
                // this the one-player admission check is advisory: a refused
                // player (or anything else on loopback) could skip the
                // handshake and drive the agent's console anyway.
                if !holds_slot {
                    eprintln!("kessel mcp: attach TICK before HELLO from {peer}, dropping");
                    break;
                }
                let Ok(request) = Tick::read(&mut reader) else {
                    break;
                };
                let frame = tick(console, request.input);
                if frame.write(&mut writer).is_err() {
                    break;
                }
            }
            other => {
                eprintln!("kessel mcp: attach got unknown opcode {other:#x}, dropping player");
                break;
            }
        }
    }
    if holds_slot {
        taken.store(false, Ordering::Release);
    }
    if announced {
        eprintln!("kessel mcp: player detached ({peer})");
    }
}

/// Advance the shared machine one frame and snapshot the screen.
///
/// The lock is held for exactly this and released before the frame goes out on
/// the wire, so a slow socket never blocks a tool call.
fn tick(console: &Shared, input: kessel_vm::device::Input) -> Frame {
    let mut c = console.lock();
    if c.rom_loaded {
        c.play_tick(input);
        Frame {
            has_rom: true,
            paused: c.is_paused(),
            halted: c.vm.halted,
            dim: c.screen_dim() as u16,
            rgba: c.framebuffer_rgba(),
        }
    } else {
        // The agent may not have loaded a ROM yet, or just reset. Report an
        // empty screen rather than dropping the connection — it will load one.
        let dim = c.screen_dim() as usize;
        Frame {
            has_rom: false,
            paused: false,
            halted: false,
            dim: dim as u16,
            rgba: vec![0; dim * dim * 4],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kessel_vm::device::Input;
    use kessel_vm::VmConsole;
    use parking_lot::Mutex;
    use std::sync::Arc;

    fn console() -> Shared {
        Arc::new(Mutex::new(VmConsole::new()))
    }

    /// A trivial ROM, so `play_tick` actually advances the frame counter.
    fn load_a_rom(shared: &Shared) {
        let tools = kessel_vm::VmToolSet::with_console(shared.clone());
        let src = "local x = 60\n\
                   function update() x = x + 1 end\n\
                   function draw() cls(0) pset(x % 128, 60, 7) end\n";
        tools
            .call(
                "vm_write_source",
                serde_json::json!({"path": "g.lua", "source": src}),
            )
            .unwrap();
        tools
            .call("vm_assemble", serde_json::json!({"path": "g.lua"}))
            .unwrap();
        tools
            .call("vm_load_rom", serde_json::json!({"path": "g.lua"}))
            .unwrap();
        assert!(shared.lock().rom_loaded, "test ROM should be loaded");
    }

    /// With no ROM the player still gets a well-formed, blank frame — it must
    /// not error or disconnect while the agent is still writing the game.
    #[test]
    fn tick_without_a_rom_yields_a_blank_frame() {
        let c = console();
        let f = tick(&c, Input::default());
        assert!(!f.has_rom);
        assert_eq!(f.rgba.len(), 128 * 128 * 4);
        assert!(f.rgba.iter().all(|b| *b == 0));
    }

    /// The point of the whole feature: a tick through the attach path advances
    /// the very console the tools hold, so the agent sees the player's frames.
    #[test]
    fn tick_advances_the_shared_console() {
        let shared = console();
        let tools = kessel_vm::VmToolSet::with_console(shared.clone());

        let src = "local x = 60\n\
                   function update() x = x + 1 end\n\
                   function draw() cls(0) pset(x % 128, 60, 7) entity(x % 128, 60, 1) end\n";
        tools
            .call(
                "vm_write_source",
                serde_json::json!({"path": "g.lua", "source": src}),
            )
            .unwrap();
        tools
            .call("vm_assemble", serde_json::json!({"path": "g.lua"}))
            .unwrap();
        tools
            .call("vm_load_rom", serde_json::json!({"path": "g.lua"}))
            .unwrap();

        let before = shared.lock().frame;
        let f = tick(&shared, Input::default());
        assert!(f.has_rom);
        let after = shared.lock().frame;
        assert_eq!(after, before + 1, "attach tick must advance the shared VM");

        // ...and a tool call sees the advanced state, which is the shared
        // timeline the design accepts.
        let obs = tools
            .call("vm_inspect_stacks", serde_json::json!({}))
            .unwrap();
        assert!(!obs.text.is_empty());
    }

    /// This endpoint grants full control of a VM and a view of the screen to
    /// whoever connects, so it must never be bound to a routable interface.
    #[test]
    fn binds_loopback_only() {
        let addr = bind_addr();
        assert!(
            addr.ip().is_loopback(),
            "attach must not be reachable off-box"
        );
        assert_eq!(addr.port(), 0, "let the OS choose the port");
    }

    #[test]
    fn publishes_a_reachable_session() {
        let dir = std::env::temp_dir().join(format!("kessel-attach-bind-{}", std::process::id()));
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&dir).unwrap();

        let server = start_in(&sessions, console(), &dir).expect("listener should start");
        let s = Session::list_in(&sessions)
            .into_iter()
            .map(|(_, s)| s)
            .next()
            .expect("session published");

        // Connecting on loopback works.
        assert!(TcpStream::connect(("127.0.0.1", s.port)).is_ok());

        drop(server);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A TICK without a handshake must not advance the machine. Otherwise the
    /// one-player check is advisory only: a refused player could simply skip
    /// HELLO and drive the agent's console anyway.
    #[test]
    fn tick_before_hello_is_refused() {
        use std::io::{Read, Write};

        let dir =
            std::env::temp_dir().join(format!("kessel-attach-nohello-{}", std::process::id()));
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&dir).unwrap();

        // Load a ROM first, or the frame counter can't move and the assertion
        // below would pass whether or not the handshake is enforced.
        let shared = console();
        load_a_rom(&shared);

        let server = start_in(&sessions, shared.clone(), &dir).expect("listener should start");
        let port = Session::list_in(&sessions)[0].1.port;

        let before = shared.lock().frame;
        let mut sock = TcpStream::connect(("127.0.0.1", port)).unwrap();
        sock.write_all(&[MSG_TICK, 0]).unwrap();
        sock.flush().unwrap();

        // The server should close on us rather than answer with a frame.
        let mut buf = [0u8; 1];
        assert!(
            matches!(sock.read(&mut buf), Ok(0) | Err(_)),
            "server answered a TICK that skipped the handshake"
        );
        assert_eq!(
            shared.lock().frame,
            before,
            "an un-handshaken TICK advanced the shared VM"
        );

        drop(server);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two servers rooted at the same directory share a filename. The first to
    /// exit must not delete the second's live advertisement.
    #[test]
    fn exiting_does_not_delete_a_newer_servers_session() {
        let dir = std::env::temp_dir().join(format!("kessel-attach-race-{}", std::process::id()));
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&dir).unwrap();

        let first = start_in(&sessions, console(), &dir).expect("first server");
        let first_port = Session::list_in(&sessions)[0].1.port;

        // A second server for the same root overwrites the advertisement.
        let second = start_in(&sessions, console(), &dir).expect("second server");
        let second_port = Session::list_in(&sessions)[0].1.port;
        assert_ne!(first_port, second_port, "servers should get distinct ports");

        // The older one exits: the newer, still-live entry must survive.
        drop(first);
        let after = Session::list_in(&sessions);
        assert_eq!(after.len(), 1, "newer session was deleted by the older one");
        assert_eq!(after[0].1.port, second_port);

        drop(second);
        assert!(
            Session::list_in(&sessions).is_empty(),
            "the owning server should clean up on exit"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A clean shutdown must not leave a session file that shadows the next server.
    #[test]
    fn dropping_the_server_removes_its_session_file() {
        let dir = std::env::temp_dir().join(format!("kessel-attach-drop-{}", std::process::id()));
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&dir).unwrap();

        let server = start_in(&sessions, console(), &dir).expect("listener should start");
        let path = server.session_path.clone();
        assert!(path.exists());
        drop(server);
        assert!(!path.exists(), "session file should be cleaned up on drop");
        std::fs::remove_dir_all(&dir).ok();
    }
}
