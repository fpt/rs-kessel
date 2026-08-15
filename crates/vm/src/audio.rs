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

use crate::VmConsole;

/// One sound event as JSON, for the observation record and `vm_run_frames`.
///
/// Defined once: two spellings of "what the game asked for" is how an agent
/// ends up debugging the difference between two reports of the same frame.
pub fn event_json(ev: &AudioEvent) -> serde_json::Value {
    match *ev {
        AudioEvent::PlaySfx { id } => serde_json::json!({"kind": "sfx", "id": id}),
        AudioEvent::PlayMusic { id } => serde_json::json!({"kind": "music", "id": id}),
        AudioEvent::StopMusic => serde_json::json!({"kind": "music_stop"}),
        AudioEvent::Play {
            inst,
            note,
            vel,
            frames,
        } => serde_json::json!({
            "kind": "play", "inst": inst, "note": note, "vel": vel, "frames": frames,
        }),
        AudioEvent::NoteOn {
            chan,
            inst,
            note,
            vel,
        } => serde_json::json!({
            "kind": "note_on", "chan": chan, "inst": inst, "note": note, "vel": vel,
        }),
        AudioEvent::NoteOff { chan } => serde_json::json!({"kind": "note_off", "chan": chan}),
        AudioEvent::Panic => serde_json::json!({"kind": "panic"}),
    }
}

/// Name an event for the trace: what kind it is, and the id it names.
///
/// A note has no single id, so its instrument and pitch are packed into one —
/// enough for a machine to compare, while `trace` supplies the readable name.
fn describe(ev: AudioEvent) -> (&'static str, u16) {
    match ev {
        AudioEvent::PlaySfx { id } => ("sfx", id),
        AudioEvent::PlayMusic { id } => ("music", id),
        AudioEvent::StopMusic => ("music_stop", 0),
        AudioEvent::Play { inst, note, .. } => ("play", ((inst as u16) << 8) | note as u16),
        AudioEvent::NoteOn { inst, note, .. } => ("note_on", ((inst as u16) << 8) | note as u16),
        AudioEvent::NoteOff { chan } => ("note_off", chan as u16),
        AudioEvent::Panic => ("panic", 0),
    }
}

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
    /// `music(id)` for an id the bank doesn't have.
    pub unknown_track: u64,
    /// Notes the machine refused because an argument was out of range.
    pub bad_note_args: u64,
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
        if self.bad_note_args > 0 {
            out.push_str(&format!(
                "WARNING: {} note(s) were ignored because an argument was out of range \
                 (channel and instrument 0-255, note 0-127, velocity 0-255).\n",
                self.bad_note_args
            ));
        }
        if self.unknown_track > 0 {
            out.push_str(&format!(
                "WARNING: {} music trigger(s) named a track the bank doesn't have.\n",
                self.unknown_track
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
    /// Describe one event for the trace, naming the declaration behind an id
    /// where there is one.
    fn trace(&self, ev: AudioEvent, frame: u64) -> AudioTrace {
        let (kind, id) = describe(ev);
        let name = match ev {
            AudioEvent::PlaySfx { id } => self.sound_bank().sfx_names.get(id as usize).cloned(),
            AudioEvent::PlayMusic { id } => self.sound_bank().track_names.get(id as usize).cloned(),
            // A note reads better as `piano 67` than as the packed id the
            // trace carries for machine comparison.
            AudioEvent::Play { inst, note, .. } | AudioEvent::NoteOn { inst, note, .. } => self
                .sound_bank()
                .instrument_names
                .get(inst as usize)
                .map(|n| format!("{n} {note}")),
            _ => None,
        };
        AudioTrace {
            frame,
            kind,
            id,
            name,
        }
    }

    /// Run the loaded ROM forward and render its audio.
    ///
    /// **This advances the machine**, exactly like `run_frame` — it is the same
    /// call, with a synth listening. Snapshot first if you want the state back.
    ///
    /// `segments` is `(input, frames)` pairs, so a render can follow an input
    /// script: a sound that only fires when you press A needs A pressed — and a
    /// sound that only fires when a finger lands needs the finger.
    pub fn render_audio(
        &mut self,
        segments: &[(crate::device::Input, u64)],
    ) -> Result<AudioRender, String> {
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
        // Counted cumulatively by the device, so take the difference over this
        // run rather than reporting a total from before it started.
        let dropped_before = self.vm.devices.sound_dropped;
        let mut samples: Vec<f32> = Vec::new();
        let mut block = vec![0.0f32; spf * 2];

        // Whatever `init()` asked for happens before frame 0's samples.
        for ev in self.take_reset_sound() {
            summary.events.push(self.trace(ev, 0));
            engine.submit(ev, 0);
        }

        let mut frame = 0u64;
        'outer: for (input, count) in segments {
            for _ in 0..*count {
                if frame >= MAX_RENDER_FRAMES {
                    summary.stopped_early =
                        Some(format!("frame cap ({MAX_RENDER_FRAMES}) reached"));
                    break 'outer;
                }
                let obs = self.run_frame(*input);
                // Timestamps are relative to the render, which always starts at
                // sample zero; the *trace* uses the console's own counter.
                let at = engine.frame_at(frame);
                // Cloned so the trace can look names up on `self` while the
                // borrow of the observation is done with.
                for ev in obs.sound.clone() {
                    summary.events.push(self.trace(ev, obs.frame));
                    engine.submit(ev, at);
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
        summary.bad_note_args = self.vm.devices.sound_dropped - dropped_before;
        let eng = engine.stats();
        summary.unknown_sfx = eng.unknown_sfx;
        summary.unknown_track = eng.unknown_track;
        summary.queue_overflow = eng.queue_overflow;

        Ok(AudioRender { samples, summary })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Input;

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
        let r = c.render_audio(&[(Input::default(), 30)]).unwrap();
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
        let r = c.render_audio(&[(Input::default(), 10)]).unwrap();
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
        let r = c.render_audio(&[(Input::default(), 10)]).unwrap();
        assert_eq!(r.summary.unknown_sfx, 1);
        assert_eq!(r.summary.events[0].name, None);
        let text = r.summary.report();
        assert!(text.contains("no such declaration"), "{text}");
        assert!(text.contains("WARNING"), "{text}");
    }

    #[test]
    fn a_track_renders_and_is_named_in_the_trace() {
        let mut c = console(
            r#"
            instrument bass { wave = triangle  attack = 0  decay = 0  sustain = 200  release = 20 }
            track theme { tempo = 4  bass = "36 - 43 -" }
            local t: word
            function update() t = t + 1  if t == 2 then music(theme) end end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(Input::default(), 40)]).unwrap();
        assert_eq!(r.summary.events.len(), 1);
        assert_eq!(r.summary.events[0].kind, "music");
        assert_eq!(r.summary.events[0].name.as_deref(), Some("theme"));
        assert!(r.summary.peak > 0.1, "the track was silent");
        // It loops by default, so a 40-frame render is more than one pass.
        assert!(r.summary.voices_started > 2);
    }

    #[test]
    fn music_stop_shows_up_in_the_trace() {
        let mut c = console(
            r#"
            instrument bass { wave = triangle  sustain = 200 }
            track theme { tempo = 4  bass = "36 - 43 -" }
            local t: word
            function update()
              t = t + 1
              if t == 2 then music(theme) end
              if t == 10 then music_stop() end
            end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(Input::default(), 40)]).unwrap();
        let kinds: Vec<&str> = r.summary.events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["music", "music_stop"]);
    }

    #[test]
    fn an_unknown_track_is_a_warning_not_silence() {
        let mut c = console(
            r#"
            local t: word
            function update() t = t + 1  if t == 2 then music(3) end end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(Input::default(), 10)]).unwrap();
        assert_eq!(r.summary.unknown_track, 1);
        let text = r.summary.report();
        assert!(
            text.contains("named a track the bank doesn't have"),
            "{text}"
        );
    }

    #[test]
    fn sound_asked_for_in_init_is_not_lost() {
        // `init()` runs at load, outside any frame, and the device's log is
        // cleared at the start of the next one. Starting the music in `init()`
        // is the obvious way to write a game, and without the reset-vector
        // hand-off it is silent everywhere.
        let mut c = console(
            r#"
            instrument bass { wave = triangle  attack = 0  decay = 0  sustain = 200  release = 20 }
            track theme { tempo = 4  bass = "36 - 43 -" }
            function init() music(theme) end
            function update() end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(Input::default(), 30)]).unwrap();
        assert_eq!(r.summary.events.len(), 1, "the init() trigger was dropped");
        assert_eq!(r.summary.events[0].kind, "music");
        assert_eq!(r.summary.events[0].frame, 0);
        assert!(r.summary.peak > 0.1, "the track never played");
    }

    #[test]
    fn play_reaches_the_synth_with_every_argument_intact() {
        // The note ports latch three values and commit on the fourth. A wrong
        // register or a wrong commit order does not fail — it plays a
        // different note, which is why every field is checked.
        let mut c = console(
            r#"
            instrument piano { wave = triangle  attack = 0  decay = 200  sustain = 0 }
            local t: word
            function update()
              t = t + 1
              if t == 2 then play(piano, 67, 200, 15) end
            end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(Input::default(), 30)]).unwrap();
        assert_eq!(r.summary.events.len(), 1);
        assert_eq!(r.summary.events[0].kind, "play");
        assert_eq!(r.summary.events[0].name.as_deref(), Some("piano 67"));
        assert_eq!(r.summary.voices_started, 1);
        assert!(r.summary.peak > 0.1, "the note was silent");
    }

    #[test]
    fn a_held_note_lasts_until_note_off() {
        // `play` is fire-and-forget; `note_on` holds until the game says so.
        // The difference is the whole reason both exist.
        let src = |release: &str| {
            format!(
                r#"
                instrument organ {{ wave = square  attack = 0  decay = 0  sustain = 255  release = 10 }}
                local t: word
                function update()
                  t = t + 1
                  if t == 2 then note_on(0, organ, 60, 200) end
                  {release}
                end
                function draw() cls(0) end
                "#
            )
        };
        // Held: still sounding at the end of a long render.
        let mut held = console(&src(""));
        let r = held.render_audio(&[(Input::default(), 90)]).unwrap();
        let spf = kessel_audio::samples_per_frame(r.summary.sample_rate) as usize;
        let last = &r.samples[r.samples.len() - spf * 2..];
        assert!(
            last.iter().fold(0.0f32, |a, s| a.max(s.abs())) > 0.05,
            "a held note stopped on its own"
        );

        // Released: silent well before the end.
        let mut released = console(&src("if t == 20 then note_off(0) end"));
        let r = released.render_audio(&[(Input::default(), 90)]).unwrap();
        let kinds: Vec<&str> = r.summary.events.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["note_on", "note_off"]);
        let last = &r.samples[r.samples.len() - spf * 2..];
        assert_eq!(
            last.iter().fold(0.0f32, |a, s| a.max(s.abs())),
            0.0,
            "note_off did not release the note"
        );
    }

    #[test]
    fn an_out_of_range_note_is_reported_rather_than_played_wrong() {
        // Every channel is one some game may be holding, so an invalid one
        // cannot be mapped onto the valid range without stealing a note. The
        // machine emits nothing — and says so, or the silence is unexplainable.
        let mut c = console(
            r#"
            instrument piano { wave = triangle  attack = 0  decay = 200  sustain = 0 }
            local t: word
            function update()
              t = t + 1
              if t == 2 then play(piano, 300, 200, 15) end
            end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(Input::default(), 20)]).unwrap();
        assert_eq!(r.summary.bad_note_args, 1);
        assert!(r.summary.events.is_empty(), "a bad note reached the synth");
        assert_eq!(r.summary.peak, 0.0);
        assert!(
            r.summary.report().contains("out of range"),
            "{}",
            r.summary.report()
        );
    }

    #[test]
    fn a_note_naming_no_instrument_is_counted_not_guessed() {
        let mut c = console(
            r#"
            local t: word
            function update() t = t + 1  if t == 2 then play(9, 60, 200, 10) end end
            function draw() cls(0) end
            "#,
        );
        let r = c.render_audio(&[(Input::default(), 10)]).unwrap();
        assert_eq!(r.summary.notes_dropped, 1);
        assert_eq!(r.summary.peak, 0.0);
        assert!(r
            .summary
            .report()
            .contains("instrument the bank doesn't have"));
    }

    #[test]
    fn a_render_is_reproducible() {
        let one = console(GAME)
            .render_audio(&[(Input::default(), 30)])
            .unwrap();
        let two = console(GAME)
            .render_audio(&[(Input::default(), 30)])
            .unwrap();
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
        let idle = console(src)
            .render_audio(&[(Input::default(), 20)])
            .unwrap();
        assert!(idle.summary.events.is_empty());
        assert_eq!(idle.summary.peak, 0.0);

        const A: u8 = crate::device::BTN_A;
        let pressed = console(src)
            .render_audio(&[
                (Input::default(), 5),
                (Input::from(A), 5),
                (Input::default(), 10),
            ])
            .unwrap();
        assert_eq!(pressed.summary.events.len(), 1);
        assert!(pressed.summary.peak > 0.1);
    }

    #[test]
    fn rendering_needs_a_rom() {
        let mut c = VmConsole::new();
        assert!(c.render_audio(&[(Input::default(), 10)]).is_err());
    }

    #[test]
    fn the_wav_is_well_formed() {
        let mut c = console(GAME);
        let r = c.render_audio(&[(Input::default(), 6)]).unwrap();
        let wav = r.to_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        // 6 frames plus the half-second tail, stereo, 16-bit.
        assert_eq!(wav.len(), 44 + r.samples.len() * 2);
    }
}
