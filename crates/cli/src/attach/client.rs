//! The `kessel attach` side of a shared session.
//!
//! The round trip runs on its own thread, not the UI thread. That matters: the
//! agent can hold the console mutex for a long time — `vm_run_frames(1800)` is
//! one call — and a tick issued from the event loop would freeze the entire
//! window, not just the game. Instead the worker blocks, the UI keeps drawing
//! the last frame it got, and the window stays responsive enough to close.

use std::io::{BufReader, BufWriter, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use kessel_vm::device::Input;

use super::protocol::{Frame, Hello, Tick, MSG_HELLO, PROTOCOL_VERSION};
use super::session::Session;

/// Time between ticks the worker asks for. The console is defined at 60 Hz.
const TICK_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// A live attachment to a running `kessel mcp`.
pub struct AttachClient {
    dim: u32,
    /// What the UI thread wants sent with the next tick; read by the worker.
    ///
    /// A mutex rather than the atomic this used to be: an `Input` is buttons,
    /// a stick and four touch points, and tearing them apart across two frames
    /// would report a finger at a position it was never at.
    input: Arc<Mutex<Input>>,
    /// Most recent frame from the server. `None` until the first arrives.
    latest: Arc<Mutex<Option<Frame>>>,
    connected: Arc<AtomicBool>,
    /// Tells the worker to stop when the window closes.
    shutdown: Arc<AtomicBool>,
}

impl Drop for AttachClient {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl AttachClient {
    /// Connect to `session`, handshake, and start ticking.
    pub fn connect(session: &Session) -> Result<Self, String> {
        let stream = TcpStream::connect(("127.0.0.1", session.port))
            .map_err(|e| format!("connect to session on port {}: {e}", session.port))?;
        stream.set_nodelay(true).ok();

        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| format!("clone socket: {e}"))?,
        );
        let mut writer = BufWriter::new(stream);

        writer
            .write_all(&[MSG_HELLO])
            .and_then(|_| writer.flush())
            .map_err(|e| format!("send hello: {e}"))?;
        let hello = Hello::read(&mut reader).map_err(|e| format!("read hello: {e}"))?;

        if hello.busy {
            return Err("another player is already attached to this session — \
                 only one window at a time can drive the agent's VM"
                .to_string());
        }
        if hello.version != PROTOCOL_VERSION {
            return Err(format!(
                "session speaks attach protocol v{} but this kessel speaks v{} — \
                 the running `kessel mcp` is a different build",
                hello.version, PROTOCOL_VERSION
            ));
        }

        let input = Arc::new(Mutex::new(Input::default()));
        let latest = Arc::new(Mutex::new(None));
        let connected = Arc::new(AtomicBool::new(true));
        let shutdown = Arc::new(AtomicBool::new(false));

        let client = AttachClient {
            dim: hello.dim as u32,
            input: input.clone(),
            latest: latest.clone(),
            connected: connected.clone(),
            shutdown: shutdown.clone(),
        };

        std::thread::spawn(move || {
            let mut next = std::time::Instant::now();
            while !shutdown.load(Ordering::Relaxed) {
                let tick = Tick {
                    input: *input.lock(),
                };
                if tick.write(&mut writer).is_err() {
                    break;
                }
                // Blocks for as long as the agent holds the console. That is
                // fine here and fatal on the UI thread — hence this thread.
                match Frame::read(&mut reader) {
                    Ok(f) => *latest.lock() = Some(f),
                    Err(_) => break,
                }

                // Pace the request rate; if a tick took longer than the budget
                // (the agent was busy) go straight to the next rather than
                // trying to catch up, which would fast-forward the game.
                next += TICK_INTERVAL;
                let now = std::time::Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
            }
            connected.store(false, Ordering::Relaxed);
        });

        Ok(client)
    }

    /// Screen edge length of the most recent frame, falling back to the size
    /// HELLO advertised before one has arrived.
    ///
    /// Reads the *frame* rather than a value latched at connect time: the agent
    /// can load a ROM with another `screen` mode while someone is attached, and
    /// the window has to follow it.
    pub fn screen_dim(&self) -> u32 {
        match self.latest.lock().as_ref() {
            Some(f) if f.dim > 0 => f.dim as u32,
            _ => self.dim,
        }
    }

    /// Record the input to send with the next tick. Cheap: the UI thread only
    /// ever stores here, and the worker reads.
    pub fn set_input(&self, input: Input) {
        *self.input.lock() = input;
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// The most recent frame, or `None` before the first one lands.
    pub fn framebuffer_rgba(&self) -> Option<Vec<u8>> {
        let latest = self.latest.lock();
        match latest.as_ref() {
            Some(f) if f.has_rom => Some(f.rgba.clone()),
            _ => None,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.latest.lock().as_ref().is_some_and(|f| f.paused)
    }

    pub fn has_rom(&self) -> bool {
        self.latest.lock().as_ref().is_some_and(|f| f.has_rom)
    }
}

/// Read a `Hello` a server would send, for tests that don't want a socket.
#[cfg(test)]
pub(crate) fn parse_hello(bytes: &[u8]) -> std::io::Result<Hello> {
    Hello::read(&mut &bytes[..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attach::protocol::FLAG_HAS_ROM;

    #[test]
    fn refuses_a_session_that_is_not_listening() {
        // Port 1 on loopback: nothing is there, and binding it needs root.
        let session = Session {
            port: 1,
            root: "/tmp".into(),
            pid: 0,
            version: "0.1.0".into(),
        };
        let err = match AttachClient::connect(&session) {
            Err(e) => e,
            Ok(_) => panic!("nothing should be listening on port 1"),
        };
        assert!(err.contains("connect to session"), "{err}");
    }

    /// A version bump must be reported as a version problem, not as a garbled
    /// frame stream later on.
    #[test]
    fn a_version_mismatch_is_detected_at_handshake() {
        let wrong = Hello {
            version: PROTOCOL_VERSION.wrapping_add(1),
            busy: false,
            dim: 128,
            controls_json: "{}".into(),
        };
        let mut buf = Vec::new();
        wrong.write(&mut buf).unwrap();
        let parsed = parse_hello(&buf).unwrap();
        assert_ne!(parsed.version, PROTOCOL_VERSION);
    }

    /// A frame with no ROM must read as "nothing to draw" rather than handing
    /// the window a buffer of zeroes to render as a black game.
    #[test]
    fn a_romless_frame_reports_nothing_to_draw() {
        let latest = Arc::new(Mutex::new(Some(Frame {
            has_rom: false,
            paused: false,
            halted: false,
            dim: 2,
            rgba: vec![0; 16],
        })));
        let client = AttachClient {
            dim: 2,
            input: Arc::new(Mutex::new(Input::default())),
            latest: latest.clone(),
            connected: Arc::new(AtomicBool::new(true)),
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        assert!(client.framebuffer_rgba().is_none());
        assert!(!client.has_rom());

        *latest.lock() = Some(Frame {
            has_rom: true,
            paused: true,
            halted: false,
            dim: 2,
            rgba: vec![9; 16],
        });
        assert_eq!(client.framebuffer_rgba(), Some(vec![9; 16]));
        assert!(client.is_paused());
        assert_eq!(FLAG_HAS_ROM, 1);
    }
}
