//! Offline audio rendering: run the game, render what it asked for, and report
//! what happened.
//!
//! This is the audio half of the agent loop, and it is built the way it is
//! because **the agent cannot listen**. A WAV alone would be an artefact nobody
//! in the loop can read, so every render also returns numbers that distinguish
//! the failures a person would otherwise diagnose by ear:
//!
//! | what you hear | what it reports |
//! |---|---|
//! | nothing | `events` is empty — the game never called `sfx()` |
//! | nothing, but events fired | `unknown_sfx`, or `notes_dropped` |
//! | a mess | `limited` samples, `peak` |
//! | only the first sound | `queue_overflow` |
//!
//! The renderer advances the machine exactly as `run_frame` does, one frame at
//! a time, submitting each frame's sound events at that frame's sample and
//! rendering exactly [`samples_per_frame`] samples. That is what makes an
//! offline render reproducible: same ROM, same inputs, same WAV.

use kessel_audio::{
    samples_per_frame, wav, AudioEngine, AudioEvent, SynthConfig, DEFAULT_SAMPLE_RATE,
};

use crate::device::SoundKind;
use crate::VmConsole;

/// One sound the game triggered, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrace {
    /// The console's frame counter, **the same number `vm_run_frames` reports**
    /// for the same trigger. Not the render's own offset: an agent comparing a
    /// run trace with an audio trace has to see one event, not two.
    pub frame: u64,
    /// `"sfx"`, `"music"`, or `"music_stop"`.
    pub kind: &'static str,
    pub id: u16,
    /// The declaration's name, when the bank has one. An id with no name is
    /// how `sfx(7)` in a game with three effects shows up.
    pub name: Option<String>,
}

/// What a render did, in numbers an agent can act on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioSummary {
    pub frames: u64,
    pub sample_rate: u32,
    pub duration_secs: f32,
    pub peak: f32,
    pub rms: f32,
    /// Stereo frames the master limiter turned down. Non-zero means the mix
    /// was too loud, which `peak` cannot show — the limiter holds peak at its
    /// ceiling either way.
    pub limited: u64,
    pub voices_started: u64,
    pub voices_stolen: u64,
    /// Notes that named an instrument the bank doesn't have.
    pub notes_dropped: u64,
    /// `sfx(id)` for an id the bank doesn't have.
    pub unknown_sfx: u64,
    /// Notes that never got scheduled because too much was in flight.
    pub queue_overflow: u64,
    /// Every trigger the game emitted, in order.
    pub events: Vec<AudioTrace>,
    /// Triggers that were recorded but cannot be rendered yet (`music`).
    pub unsupported: u64,
    /// Why the run stopped before the requested frame count, if it did.
    pub stopped_early: Option<String>,
}

impl AudioSummary {
    /// A short human/agent-readable report.
    ///
    /// Deliberately prose rather than JSON: this is read by a model deciding
    /// what to fix next, and "no sound events — the game never called sfx()"
    /// is a more useful sentence than `{"events": []}`.
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "rendered {} frames ({:.2}s at {} Hz)\n",
            self.frames, self.duration_secs, self.sample_rate
        ));
        if let Some(why) = &self.stopped_early {
            out.push_str(&format!("stopped early: {why}\n"));
        }
        out.push_str(&format!(
            "level: peak {:.3}, rms {:.3}{}\n",
            self.peak,
            self.rms,
            if self.limited > 0 {
                format!(", limiter engaged on {} samples", self.limited)
            } else {
                String::new()
            }
        ));
        out.push_str(&format!(
            "voices: {} started, {} stolen\n",
            self.voices_started, self.voices_stolen
        ));

        if self.events.is_empty() {
            out.push_str(
                "no sound events — the game never called sfx()/music(). \
                 Nothing was going to play.\n",
            );
        } else {
            out.push_str(&format!("{} triggers:\n", self.events.len()));
            for e in &self.events {
                match &e.name {
                    Some(n) => out.push_str(&format!("  frame {:<5} {} {}\n", e.frame, e.kind, n)),
                    None => out.push_str(&format!(
                        "  frame {:<5} {} {} (no such declaration)\n",
                        e.frame, e.kind, e.id
                    )),
                }
            }
        }

        // Each of these is a specific, fixable cause of "it didn't sound right".
        if self.unknown_sfx > 0 {
            out.push_str(&format!(
                "WARNING: {} trigger(s) named an sfx the bank doesn't have — \
                 nothing played for them.\n",
                self.unknown_sfx
            ));
        }
        if self.notes_dropped > 0 {
            out.push_str(&format!(
                "WARNING: {} note(s) named an instrument the bank doesn't have.\n",
                self.notes_dropped
            ));
        }
        if self.queue_overflow > 0 {
            out.push_str(&format!(
                "WARNING: {} note(s) dropped — too many sounds in flight at once.\n",
                self.queue_overflow
            ));
        }
        if self.unsupported > 0 {
            out.push_str(&format!(
                "NOTE: {} music trigger(s) were recorded but the sequencer is not \
                 built yet, so they are silent in this render.\n",
                self.unsupported
            ));
        }
        if self.peak == 0.0 && !self.events.is_empty() {
            out.push_str(
                "WARNING: triggers fired but the render is silent — check the \
                 instrument's volume and envelope.\n",
            );
        }
        out
    }
}

/// A finished render: the audio, and what happened while making it.
pub struct AudioRender {
    /// Interleaved stereo f32, `frames * samples_per_frame` long.
    pub samples: Vec<f32>,
    pub summary: AudioSummary,
}

impl AudioRender {
    /// The render as a 16-bit stereo WAV.
    pub fn to_wav(&self) -> Vec<u8> {
        wav::encode_pcm16(self.summary.sample_rate, 2, &self.samples)
    }
}

/// Ceiling on one render, matching `vm_run_frames`: 30 seconds at 60 fps.
pub const MAX_RENDER_FRAMES: u64 = 1800;

impl VmConsole {
    /// Run the loaded ROM forward and render its audio.
    ///
    /// **This advances the machine**, exactly like `run_frame` — it is the same
    /// call, with a synth listening. Snapshot first if you want the state back.
    ///
    /// `segments` is `(buttons, frames)` pairs, so a render can follow an input
    /// script: a sound that only fires when you press A needs A pressed.
    pub fn render_audio(&mut self, segments: &[(u8, u64)]) -> Result<AudioRender, String> {
        if !self.rom_loaded {
            return Err("no ROM loaded — call load_rom first".to_string());
        }
        let sample_rate = DEFAULT_SAMPLE_RATE;
        let spf = samples_per_frame(sample_rate) as usize;

        let mut engine = AudioEngine::new(SynthConfig {
            sample_rate,
            ..SynthConfig::default()
        });
        engine.set_bank(self.sound_bank().clone());

        let mut summary = AudioSummary {
            sample_rate,
            ..AudioSummary::default()
        };
        let mut samples: Vec<f32> = Vec::new();
        let mut block = vec![0.0f32; spf * 2];

        let mut frame = 0u64;
        'outer: for (bits, count) in segments {
            for _ in 0..*count {
                if frame >= MAX_RENDER_FRAMES {
                    summary.stopped_early =
                        Some(format!("frame cap ({MAX_RENDER_FRAMES}) reached"));
                    break 'outer;
                }
                let obs = self.run_frame(*bits);
                // Timestamps are relative to the render, which always starts at
                // sample zero; the *trace* uses the console's own counter.
                let at = engine.frame_at(frame);
                for s in &obs.sound {
                    let (kind, event) = match s.kind {
                        SoundKind::Sfx => ("sfx", Some(AudioEvent::PlaySfx { id: s.id })),
                        // Recorded and traced, but the sequencer that would
                        // play them is a later step. Reporting them as
                        // unsupported beats rendering silence and saying
                        // nothing.
                        SoundKind::Music => ("music", None),
                        SoundKind::MusicStop => ("music_stop", None),
                    };
                    let name = match s.kind {
                        SoundKind::Sfx => self.sound_bank().sfx_names.get(s.id as usize).cloned(),
                        _ => None,
                    };
                    summary.events.push(AudioTrace {
                        frame: obs.frame,
                        kind,
                        id: s.id,
                        name,
                    });
                    match event {
                        Some(ev) => engine.submit(ev, at),
                        None => summary.unsupported += 1,
                    }
                }
                engine.render(&mut block);
                samples.extend_from_slice(&block);
                frame += 1;

                if obs.halted || obs.fault.is_some() {
                    summary.stopped_early = Some(match &obs.fault {
                        Some(f) => format!("faulted: {f}"),
                        None => "halted".to_string(),
                    });
                    break 'outer;
                }
            }
        }

        // Let the tails finish. A render that cuts off the last note's release
        // sounds like a bug in the synth rather than the end of the buffer.
        let tail = (sample_rate / 2) as usize / spf; // half a second, in frames
        for _ in 0..tail {
            engine.render(&mut block);
            samples.extend_from_slice(&block);
        }

        summary.frames = frame;
        summary.duration_secs = samples.len() as f32 / 2.0 / sample_rate as f32;
        summary.peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        summary.rms = if samples.is_empty() {
            0.0
        } else {
            (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
        };

        let synth = engine.synth_stats();
        summary.voices_started = synth.started;
        summary.voices_stolen = synth.stolen;
        summary.notes_dropped = synth.dropped;
        summary.limited = synth.limited;
        let eng = engine.stats();
        summary.unknown_sfx = eng.unknown_sfx;
        summary.queue_overflow = eng.queue_overflow;

        Ok(AudioRender { samples, summary })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &str = r#"
instrument blip {
  wave = square
  attack = 0  decay = 60  sustain = 0
  volume = 220
}
sfx ping { inst = blip  speed = 2  notes = "72 79" }

local t: word
function update()
  t = t + 1
  if t == 3 then sfx(ping) end
end
function draw() cls(0) end
"#;

    fn console(src: &str) -> VmConsole {
        let mut c = VmConsole::new();
        c.write_source("game.lua", src).unwrap();
        let built = c.assemble("game.lua").unwrap();
        assert!(built.ok(), "{:?}", built.diagnostics);
        c.load_rom("game.lua").unwrap();
        c
    }

    #[test]
    fn a_render_reports_what_the_game_triggered() {
        let mut c = console(GAME);
        let r = c.render_audio(&[(0, 30)]).unwrap();
        assert_eq!(r.summary.frames, 30);
        assert_eq!(r.summary.events.len(), 1);
        // The console counts frames from 1, and `vm_run_frames` reports the
        // same number for this trigger.
        assert_eq!(r.summary.events[0].frame, 3);
        assert_eq!(r.summary.events[0].name.as_deref(), Some("ping"));
        assert_eq!(r.summary.voices_started, 2); // two rows, two notes
        assert!(r.summary.peak > 0.1);
        assert!(r.summary.rms > 0.0);
        assert!(r.samples.iter().all(|s| s.is_finite()));

        let text = r.summary.report();
        assert!(text.contains("frame 3"), "{text}");
        assert!(text.contains("ping"), "{text}");
    }

    #[test]
    fn a_silent_game_says_why_it_is_silent() {
        let mut c = console("function update() end\nfunction draw() cls(0) end");
        let r = c.render_audio(&[(0, 10)]).unwrap();
        assert_eq!(r.summary.peak, 0.0);
        assert!(r.summary.events.is_empty());
        let text = r.summary.report();
        assert!(text.contains("never called sfx()"), "{text}");
    }

    #[test]
    fn an_id_with_no_declaration_is_named_as_such() {
        // `sfx(7)` in a game with one effect: the trace has to say the id
        // missed, or the agent sees "a trigger fired" and silence.
        let mut c = console(
            r#"
            instrument i { wave = sine }
            sfx only { inst = i  notes = "60" }
            local t: word
            function update() t = t + 1  if t == 2 then sfx(7) end end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(0, 10)]).unwrap();
        assert_eq!(r.summary.unknown_sfx, 1);
        assert_eq!(r.summary.events[0].name, None);
        let text = r.summary.report();
        assert!(text.contains("no such declaration"), "{text}");
        assert!(text.contains("WARNING"), "{text}");
    }

    #[test]
    fn music_is_traced_and_reported_as_not_yet_playable() {
        let mut c = console(
            r#"
            local t: word
            function update() t = t + 1  if t == 2 then music(0) end end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(0, 10)]).unwrap();
        assert_eq!(r.summary.unsupported, 1);
        assert_eq!(r.summary.events[0].kind, "music");
        let text = r.summary.report();
        assert!(text.contains("sequencer is not built yet"), "{text}");
    }

    #[test]
    fn a_render_is_reproducible() {
        let one = console(GAME).render_audio(&[(0, 30)]).unwrap();
        let two = console(GAME).render_audio(&[(0, 30)]).unwrap();
        assert_eq!(one.samples, two.samples);
        assert_eq!(one.summary, two.summary);
    }

    #[test]
    fn the_input_script_is_followed() {
        // A sound behind a button only renders if the button is pressed.
        let src = r#"
            instrument i { wave = square  attack = 0  decay = 40  sustain = 0 }
            sfx shoot { inst = i  notes = "72" }
            function update() if btnp(A) then sfx(shoot) end end
            function draw() cls(0) end
        "#;
        let idle = console(src).render_audio(&[(0, 20)]).unwrap();
        assert!(idle.summary.events.is_empty());
        assert_eq!(idle.summary.peak, 0.0);

        const A: u8 = crate::device::BTN_A;
        let pressed = console(src)
            .render_audio(&[(0, 5), (A, 5), (0, 10)])
            .unwrap();
        assert_eq!(pressed.summary.events.len(), 1);
        assert!(pressed.summary.peak > 0.1);
    }

    #[test]
    fn rendering_needs_a_rom() {
        let mut c = VmConsole::new();
        assert!(c.render_audio(&[(0, 10)]).is_err());
    }

    #[test]
    fn the_wav_is_well_formed() {
        let mut c = console(GAME);
        let r = c.render_audio(&[(0, 6)]).unwrap();
        let wav = r.to_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        // 6 frames plus the half-second tail, stereo, 16-bit.
        assert_eq!(wav.len(), 44 + r.samples.len() * 2);
    }
}
