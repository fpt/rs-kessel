//! `kessel render-audio` — run a game headless and write what it sounds like.
//!
//! The offline half of the audio loop. It exists for two readers who cannot
//! share a channel: a person, who gets a `.wav` to play, and an agent, which
//! gets the report on stdout because it has no ears.
//!
//! No window, no audio device, no `play` feature — this works in a
//! `--no-default-features` build and over ssh, which is most of why it is the
//! step that lands before cpal.

use std::path::{Path, PathBuf};

use kessel_vm::VmConsole;

/// Parsed `render-audio` arguments.
#[derive(Debug)]
pub struct Args {
    pub file: PathBuf,
    pub frames: u64,
    pub out: PathBuf,
    /// Buttons held for the whole run, as gamepad bits.
    pub buttons: u8,
}

const DEFAULT_FRAMES: u64 = 180;

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
        "`kessel render-audio` needs a file, e.g. `kessel render-audio games/shooter.lua`"
            .to_string()
    })?;
    if frames == 0 {
        return Err("--frames must be at least 1".to_string());
    }
    // Default output beside the source, named after it: rendering `shooter.lua`
    // twice should overwrite one file rather than scatter them.
    let out = out.unwrap_or_else(|| default_out(&file));
    Ok(Args {
        file,
        frames,
        out,
        buttons,
    })
}

/// `games/shooter.lua` → `shooter.wav` in the current directory.
fn default_out(file: &Path) -> PathBuf {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "render".to_string());
    PathBuf::from(format!("{stem}.wav"))
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
    console.write_source(&name, &source)?;
    let built = console.assemble(&name)?;
    if !built.ok() {
        // Same call as `kessel run`: a game that does not compile has no sound
        // to render, and the diagnostics are the useful output.
        let mut msg = format!("{} did not compile:\n", args.file.display());
        for d in &built.diagnostics {
            msg.push_str(&format!("  {d:?}\n"));
        }
        return Err(msg);
    }
    console.load_rom(&name)?;

    let render = console.render_audio(&[(args.buttons.into(), args.frames)])?;
    print!("{}", render.summary.report());

    let bytes = render.to_wav();
    let len = bytes.len();
    std::fs::write(&args.out, bytes)
        .map_err(|e| format!("could not write {}: {e}", args.out.display()))?;
    println!("wrote {} ({len} bytes)", args.out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_are_a_three_second_render_beside_the_source() {
        let a = parse(&strs(&["games/shooter.lua"])).unwrap();
        assert_eq!(a.file, PathBuf::from("games/shooter.lua"));
        assert_eq!(a.frames, DEFAULT_FRAMES);
        assert_eq!(a.out, PathBuf::from("shooter.wav"));
        assert_eq!(a.buttons, 0);
    }

    #[test]
    fn options_parse_in_any_order() {
        let a = parse(&strs(&["-n", "60", "g.lua", "-o", "/tmp/x.wav"])).unwrap();
        assert_eq!(a.frames, 60);
        assert_eq!(a.file, PathBuf::from("g.lua"));
        assert_eq!(a.out, PathBuf::from("/tmp/x.wav"));
    }

    #[test]
    fn buttons_are_named_not_numbered() {
        let a = parse(&strs(&["g.lua", "--buttons", "a,right"])).unwrap();
        assert_eq!(
            a.buttons,
            kessel_vm::device::BTN_A | kessel_vm::device::BTN_RIGHT
        );
    }

    #[test]
    fn bad_arguments_say_what_was_wrong() {
        for (args, expect) in [
            (vec![], "needs a file"),
            (vec!["a.lua", "b.lua"], "unexpected argument 'b.lua'"),
            (vec!["--frames"], "needs a frame count"),
            (vec!["g.lua", "--frames", "lots"], "not a frame count"),
            (vec!["g.lua", "--frames", "0"], "at least 1"),
            (vec!["g.lua", "-o"], "needs a path"),
            (vec!["g.lua", "--buttons", "banana"], "named no buttons"),
            (vec!["g.lua", "--wat"], "unexpected option '--wat'"),
        ] {
            let err = parse(&strs(&args)).unwrap_err();
            assert!(err.contains(expect), "{args:?} said {err:?}");
        }
    }

    #[test]
    fn renders_a_game_to_a_wav_file() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("beep.lua");
        std::fs::write(
            &game,
            r#"
instrument blip { wave = square  attack = 0  decay = 60  sustain = 0 }
sfx ping { inst = blip  notes = "72" }
local t: word
function update() t = t + 1  if t == 2 then sfx(ping) end end
function draw() cls(0) end
"#,
        )
        .unwrap();
        let out = dir.path().join("beep.wav");
        run(Args {
            file: game,
            frames: 30,
            out: out.clone(),
            buttons: 0,
        })
        .unwrap();

        let wav = std::fs::read(&out).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert!(wav.len() > 44, "no samples were written");
    }

    #[test]
    fn a_game_that_does_not_compile_reports_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("bad.lua");
        std::fs::write(
            &game,
            "instrument i { wave = trumpet }\nfunction update() end",
        )
        .unwrap();
        let err = run(Args {
            file: game,
            frames: 10,
            out: dir.path().join("bad.wav"),
            buttons: 0,
        })
        .unwrap_err();
        assert!(err.contains("did not compile"), "{err}");
        assert!(err.contains("trumpet"), "{err}");
    }
}
