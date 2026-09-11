//! `kessel shot` — run a game headless and write what it *looks* like.
//!
//! The visual half of what [`render_audio`](crate::render_audio) does for sound,
//! and it exists for the same two readers who cannot share a channel: a person,
//! who gets a `.png` to open, and an agent, which gets the size and the frame
//! count on stdout.
//!
//! No window, no GPU, no `play` feature — this works in a
//! `--no-default-features` build and over ssh. That matters more than it looks:
//! the bugs this catches are the *plausible-but-wrong picture* class — a game
//! laid out for one screen drawn into the corner of another, a HUD off the
//! edge, a board that no longer centres — and none of them fault, fail a test,
//! or show up in an observation record. They are only visible.
//!
//! The screen size is read **after** the ROM loads, never before: it is the
//! ROM's `screen { … }` that decides, and sizing the buffer first silently
//! yields 240×240 and tears a 320×240 game across it.

use std::path::{Path, PathBuf};

use kessel_vm::VmConsole;

/// Parsed `shot` arguments.
#[derive(Debug)]
pub struct Args {
    pub file: PathBuf,
    pub frames: u64,
    pub out: PathBuf,
    /// Buttons held for the whole run, as gamepad bits.
    pub buttons: u8,
}

/// One second. Enough for a title to settle and an idle animation to move off
/// its first frame, which is usually what makes a screenshot worth looking at.
const DEFAULT_FRAMES: u64 = 60;

pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut file: Option<PathBuf> = None;
    let mut frames = DEFAULT_FRAMES;
    let mut out: Option<PathBuf> = None;
    let mut buttons = 0u8;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--frames" | "-n" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| format!("'{a}' needs a frame count"))?;
                frames = v
                    .parse()
                    .map_err(|_| format!("'{v}' is not a frame count"))?;
                i += 2;
            }
            "-o" | "--out" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| format!("'{a}' needs a path"))?;
                out = Some(PathBuf::from(v));
                i += 2;
            }
            "--buttons" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "'--buttons' needs a list, e.g. A,RIGHT".to_string())?;
                let names: Vec<String> = v.split(',').map(|s| s.trim().to_uppercase()).collect();
                buttons = kessel_vm::buttons_from_names(&names);
                if buttons == 0 {
                    return Err(format!(
                        "'{v}' named no buttons (expected LEFT, RIGHT, UP, DOWN, A, B, START, SELECT)"
                    ));
                }
                i += 2;
            }
            other if other.starts_with('-') => return Err(format!("unexpected option '{other}'")),
            other => {
                if file.is_some() {
                    return Err(format!("unexpected argument '{other}'"));
                }
                file = Some(PathBuf::from(other));
                i += 1;
            }
        }
    }

    let file = file.ok_or_else(|| {
        "`kessel shot` needs a file, e.g. `kessel shot games/tetris.lua`".to_string()
    })?;
    // Frame 0 is a legitimate request: it is the reset vector's output, before
    // any `update` has run, which is exactly what you want when a game draws
    // something wrong on its very first frame.
    let out = out.unwrap_or_else(|| default_out(&file));
    Ok(Args {
        file,
        frames,
        out,
        buttons,
    })
}

/// `games/tetris.lua` → `tetris.png` in the current directory.
fn default_out(file: &Path) -> PathBuf {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "shot".to_string());
    PathBuf::from(format!("{stem}.png"))
}

pub fn run(args: Args) -> Result<(), String> {
    let source = std::fs::read_to_string(&args.file)
        .map_err(|e| format!("could not read {}: {e}", args.file.display()))?;
    let name = args
        .file
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "game.lua".to_string());

    let mut console = VmConsole::new();
    // The file's own directory is the include root, the same way `kessel run`
    // and `render-audio` treat it — otherwise a game that spans files shoots
    // nothing but a "cannot find include".
    console.set_root(Some(
        args.file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
    ));
    console.write_source(&name, &source)?;
    let built = console.assemble(&name)?;
    if !built.ok() {
        // Same call as `kessel run`: a game that does not compile has no
        // picture to take, and the diagnostics are the useful output.
        let mut msg = format!("{} did not compile:\n", args.file.display());
        for d in &built.diagnostics {
            msg.push_str(&format!("  {d:?}\n"));
        }
        return Err(msg);
    }
    console.load_rom(&name)?;

    // Stop at the first fault or halt rather than running on to the requested
    // frame: the picture at the moment it broke is the one worth having, and
    // the frames after a fault are the same frame over and over.
    let mut ran = 0u64;
    let mut stopped: Option<String> = None;
    for _ in 0..args.frames {
        let obs = console.run_frame(args.buttons);
        ran += 1;
        if let Some(fault) = obs.fault {
            stopped = Some(format!("faulted at frame {ran}: {fault:?}"));
            break;
        }
        if obs.halted {
            stopped = Some(format!("halted at frame {ran}"));
            break;
        }
    }

    // Read the screen from the console, after the ROM has had its say.
    let (w, h) = console.screen_size();
    let rgba = console.framebuffer_rgba();
    let bytes = kessel_vm::png::encode_rgba(w, h, &rgba);
    let len = bytes.len();
    std::fs::write(&args.out, bytes)
        .map_err(|e| format!("could not write {}: {e}", args.out.display()))?;

    if let Some(why) = &stopped {
        println!("{why}");
    }
    println!(
        "wrote {} — {w}x{h}, frame {ran} ({len} bytes)",
        args.out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_are_one_second_and_a_png_named_after_the_source() {
        let a = parse(&strs(&["games/tetris.lua"])).unwrap();
        assert_eq!(a.file, PathBuf::from("games/tetris.lua"));
        assert_eq!(a.frames, DEFAULT_FRAMES);
        assert_eq!(a.out, PathBuf::from("tetris.png"));
        assert_eq!(a.buttons, 0);
    }

    #[test]
    fn options_parse_in_any_order() {
        let a = parse(&strs(&["-n", "5", "g.lua", "-o", "/tmp/x.png"])).unwrap();
        assert_eq!(a.frames, 5);
        assert_eq!(a.file, PathBuf::from("g.lua"));
        assert_eq!(a.out, PathBuf::from("/tmp/x.png"));
    }

    #[test]
    fn buttons_are_named_not_numbered() {
        let a = parse(&strs(&["g.lua", "--buttons", "a,right"])).unwrap();
        assert_eq!(
            a.buttons,
            kessel_vm::device::BTN_A | kessel_vm::device::BTN_RIGHT
        );
        assert!(parse(&strs(&["g.lua", "--buttons", "nonsense"])).is_err());
    }

    /// Frame 0 is the reset vector's output and a legitimate thing to ask for,
    /// so it must not be rejected the way `render-audio` rejects a zero-length
    /// render (which really would be an empty file).
    #[test]
    fn frame_zero_is_allowed() {
        assert_eq!(parse(&strs(&["g.lua", "-n", "0"])).unwrap().frames, 0);
    }

    #[test]
    fn a_file_is_required_and_only_one() {
        assert!(parse(&[]).is_err());
        assert!(parse(&strs(&["a.lua", "b.lua"])).is_err());
    }

    /// The whole point of the command: the size comes from the ROM's `screen`
    /// block, read after it loads. A rectangular game must not come back square.
    #[test]
    fn the_png_is_the_size_the_rom_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wide.lua");
        std::fs::write(
            &file,
            "screen { mode = Landscape320 }\nfunction draw() cls(3) end\n",
        )
        .unwrap();
        let out = dir.path().join("wide.png");
        run(Args {
            file,
            frames: 2,
            out: out.clone(),
            buttons: 0,
        })
        .unwrap();

        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
        // IHDR's width and height are big-endian u32 at byte 16.
        let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        assert_eq!((width, height), (320, 240));
    }
}
