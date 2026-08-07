//! The wire protocol between a running `kessel mcp` and an attached
//! `kessel attach`.
//!
//! Deliberately a tiny binary protocol rather than the JSON used for MCP: this
//! carries a 60 Hz stream of full framebuffers, and base64-in-JSON would cost
//! ~5× the bytes plus an encode/decode per frame for no benefit. It is
//! request/response and strictly client-driven — **the server never advances the
//! machine on its own**, so with no player attached an agent's runs stay exactly
//! as reproducible as before.
//!
//! ```text
//! HELLO  →  [0x00]
//!        ←  [version u8][busy u8][dim u16 LE][controls_len u16 LE][controls_json …]
//!
//! TICK   →  [0x01][buttons u8]
//!        ←  [flags u8][dim u16 LE][rgba dim*dim*4]
//! ```
//!
//! Each TICK response carries its own `dim` rather than relying on HELLO's.
//! The agent can load a ROM with a different `screen { … }` mode at any moment,
//! and a length the reader only *assumed* would desynchronise the stream for
//! good — a framing bug, not a wrong picture. HELLO's `dim` is now just the
//! opening size, so a client can allocate before the first frame arrives.
//!
//! Historically both sides knew `dim` up front, so every TICK response was a fixed size and
//! needs no length prefix.

// Both halves of the codec are always compiled, even though a headless build
// only ever writes and a player only ever reads. Splitting a wire format by
// which side you are is how the two sides drift apart; the unused direction
// costs nothing and keeps the round-trip tests meaningful in every config.
#![allow(dead_code)]

use std::io::{self, Read, Write};

/// Bumped on any incompatible change. A mismatched player is refused with a
/// clear message rather than left to misparse a frame stream.
pub const PROTOCOL_VERSION: u8 = 1;

pub const MSG_HELLO: u8 = 0x00;
pub const MSG_TICK: u8 = 0x01;

// TICK response flag bits.
/// Largest screen edge a frame may claim. The console tops out at 240; this is
/// a sanity bound on a length read off a socket, not a machine limit.
pub const MAX_DIM: usize = 1024;

pub const FLAG_HAS_ROM: u8 = 0b0000_0001;
pub const FLAG_PAUSED: u8 = 0b0000_0010;
pub const FLAG_HALTED: u8 = 0b0000_0100;

/// What the server knows at handshake time.
#[derive(Debug, Clone)]
pub struct Hello {
    pub version: u8,
    /// Set when another player already holds this session. The server closes
    /// straight after, so the newcomer gets a clear refusal instead of sitting
    /// in the accept backlog waiting for a turn that may never come.
    pub busy: bool,
    /// Screen edge length in pixels (square).
    pub dim: u16,
    /// The loaded ROM's control-layout metadata, as JSON.
    pub controls_json: String,
}

impl Hello {
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        let controls = self.controls_json.as_bytes();
        let len = u16::try_from(controls.len()).unwrap_or(u16::MAX);
        w.write_all(&[self.version, self.busy as u8])?;
        w.write_all(&self.dim.to_le_bytes())?;
        w.write_all(&len.to_le_bytes())?;
        w.write_all(&controls[..len as usize])?;
        w.flush()
    }

    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        let mut head = [0u8; 6];
        r.read_exact(&mut head)?;
        let version = head[0];
        let busy = head[1] != 0;
        let dim = u16::from_le_bytes([head[2], head[3]]);
        let len = u16::from_le_bytes([head[4], head[5]]) as usize;
        let mut controls = vec![0u8; len];
        r.read_exact(&mut controls)?;
        Ok(Hello {
            version,
            busy,
            dim,
            controls_json: String::from_utf8_lossy(&controls).into_owned(),
        })
    }

    /// Bytes in one TICK response body, once `dim` is known.
    pub fn frame_bytes(&self) -> usize {
        self.dim as usize * self.dim as usize * 4
    }
}

/// One frame as the player sees it.
#[derive(Debug, Clone)]
pub struct Frame {
    pub has_rom: bool,
    pub paused: bool,
    pub halted: bool,
    /// Screen edge length this frame was rendered at. May differ from HELLO's
    /// if the agent has since loaded a ROM with another `screen` mode.
    pub dim: u16,
    /// RGBA, `dim * dim * 4` bytes. All zeroes when no ROM is loaded.
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn flags(&self) -> u8 {
        let mut f = 0;
        if self.has_rom {
            f |= FLAG_HAS_ROM;
        }
        if self.paused {
            f |= FLAG_PAUSED;
        }
        if self.halted {
            f |= FLAG_HALTED;
        }
        f
    }

    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&[self.flags()])?;
        w.write_all(&self.dim.to_le_bytes())?;
        w.write_all(&self.rgba)?;
        w.flush()
    }

    /// Read one response. The length comes off the wire, so a resolution change
    /// on the far side resizes the reader instead of tearing the stream.
    pub fn read(r: &mut impl Read) -> io::Result<Self> {
        let mut head = [0u8; 3];
        r.read_exact(&mut head)?;
        let dim = u16::from_le_bytes([head[1], head[2]]);
        // A corrupt or hostile length would otherwise be a multi-gigabyte
        // allocation on a loopback socket.
        if dim as usize > MAX_DIM {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("frame dim {dim} exceeds the {MAX_DIM} maximum"),
            ));
        }
        let mut rgba = vec![0u8; dim as usize * dim as usize * 4];
        r.read_exact(&mut rgba)?;
        Ok(Frame {
            has_rom: head[0] & FLAG_HAS_ROM != 0,
            paused: head[0] & FLAG_PAUSED != 0,
            halted: head[0] & FLAG_HALTED != 0,
            dim,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trips() {
        let h = Hello {
            version: PROTOCOL_VERSION,
            busy: false,
            dim: 128,
            controls_json: r#"{"pause":"START"}"#.to_string(),
        };
        let mut buf = Vec::new();
        h.write(&mut buf).unwrap();
        let back = Hello::read(&mut buf.as_slice()).unwrap();
        assert_eq!(back.version, PROTOCOL_VERSION);
        assert!(!back.busy);
        assert_eq!(back.dim, 128);
        assert_eq!(back.controls_json, h.controls_json);
        assert_eq!(back.frame_bytes(), 128 * 128 * 4);
    }

    #[test]
    fn frame_round_trips_every_flag() {
        for (has_rom, paused, halted) in [
            (true, false, false),
            (false, false, false),
            (true, true, false),
            (true, false, true),
            (true, true, true),
        ] {
            let f = Frame {
                has_rom,
                paused,
                halted,
                dim: 1,
                rgba: vec![1, 2, 3, 4],
            };
            let mut buf = Vec::new();
            f.write(&mut buf).unwrap();
            let back = Frame::read(&mut buf.as_slice()).unwrap();
            assert_eq!(back.dim, 1);
            assert_eq!(back.has_rom, has_rom);
            assert_eq!(back.paused, paused);
            assert_eq!(back.halted, halted);
            assert_eq!(back.rgba, vec![1, 2, 3, 4]);
        }
    }

    /// The busy flag must survive the wire, since it is the only thing telling
    /// a second player why it is being turned away.
    #[test]
    fn busy_round_trips() {
        let h = Hello {
            version: PROTOCOL_VERSION,
            busy: true,
            dim: 64,
            controls_json: String::new(),
        };
        let mut buf = Vec::new();
        h.write(&mut buf).unwrap();
        let back = Hello::read(&mut buf.as_slice()).unwrap();
        assert!(back.busy);
        assert_eq!(back.dim, 64);
    }

    /// A truncated stream must surface as an error, not a silently short frame
    /// that would render as garbage.
    #[test]
    fn a_short_frame_is_an_error() {
        // Claims dim 1 (4 bytes of pixels) but carries 3.
        let buf = vec![FLAG_HAS_ROM, 1, 0, 1, 2, 3];
        assert!(Frame::read(&mut buf.as_slice()).is_err());
    }

    /// The reader must take its length from the wire, so the agent switching
    /// to a 240×240 ROM mid-session resizes the client instead of shearing
    /// every subsequent frame by 57 KiB.
    #[test]
    fn a_frame_may_change_size_between_ticks() {
        let mut buf = Vec::new();
        Frame {
            has_rom: true,
            paused: false,
            halted: false,
            dim: 2,
            rgba: vec![7; 16],
        }
        .write(&mut buf)
        .unwrap();
        Frame {
            has_rom: true,
            paused: false,
            halted: false,
            dim: 3,
            rgba: vec![9; 36],
        }
        .write(&mut buf)
        .unwrap();

        let mut r = buf.as_slice();
        let a = Frame::read(&mut r).unwrap();
        let b = Frame::read(&mut r).unwrap();
        assert_eq!((a.dim, a.rgba.len()), (2, 16));
        assert_eq!((b.dim, b.rgba.len()), (3, 36), "stream stayed in sync");
    }

    /// A length read off a socket must be bounded, or a corrupt header is a
    /// multi-gigabyte allocation.
    #[test]
    fn an_absurd_dim_is_refused_rather_than_allocated() {
        let buf = vec![FLAG_HAS_ROM, 0xff, 0xff];
        assert!(Frame::read(&mut buf.as_slice()).is_err());
    }
}
