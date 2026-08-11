//! The audio device for `kessel run`: a cpal output stream fed by the game
//! thread through a lock-free queue.
//!
//! ```text
//! game thread              audio callback thread
//! ───────────              ─────────────────────
//! tick() → events ──► Ring ──► AudioEngine::render() ──► device
//! ```
//!
//! Three rules hold this together, and all three are about the callback thread:
//!
//! - **It never blocks.** Not on a mutex, not on the game thread. A stalled
//!   game keeps envelopes decaying and the last notes ringing; silence would be
//!   a worse failure than lateness.
//! - **It never allocates.** [`kessel_audio`] guarantees that for `render`; the
//!   queue is a fixed array and the stereo path writes straight into the
//!   device's buffer.
//! - **A full queue drops events**, counted, rather than making the game thread
//!   wait for a speaker.
//!
//! The queue itself is [`kessel_audio::EventQueue`] — every host that plays live
//! needs one, and one implementation is enough.
//!
//! Events are applied at the start of the callback block that finds them, not
//! at a sample computed from the game's frame counter. The two clocks drift —
//! a game running slightly slow would otherwise accumulate a growing offset —
//! so realtime trades sample-accurate placement for a fixed latency of one
//! device buffer. `kessel render-audio` keeps the sample-accurate placement,
//! which is what makes *it* reproducible.

use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use kessel_audio::{AudioEngine, AudioEvent, EventQueue, SoundBank, SynthConfig};

/// A live output stream. Dropping it stops the audio.
pub struct AudioHost {
    // Held for its lifetime: dropping a cpal stream closes the device.
    _stream: cpal::Stream,
    ring: Arc<EventQueue>,
    sample_rate: u32,
}

impl AudioHost {
    /// Hand an event to the synth. Never blocks; a full queue drops it.
    pub fn send(&self, ev: AudioEvent) {
        self.ring.push(ev);
    }

    /// Sounds lost because the queue was full — worth printing on exit rather
    /// than losing silently. Exact here because [`send`](Self::send) never
    /// retries.
    pub fn dropped(&self) -> u64 {
        self.ring.rejected()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

/// The body of the audio callback: drain the queue, render, spread to the
/// device's channel count.
///
/// A free function rather than a closure body so it can be tested without a
/// sound card. The channel spreading is the reason that matters — like the
/// window's `blit`, a wrong stride here produces a plausible-sounding mistake
/// (one channel silent, or a mono device at half level) rather than a crash.
///
/// Allocation-free by construction: `scratch` is sized by the caller and long
/// blocks are rendered in chunks that fit it.
fn render_block(
    engine: &mut AudioEngine,
    ring: &EventQueue,
    out: &mut [f32],
    channels: usize,
    scratch: &mut [f32],
) {
    while let Some(ev) = ring.pop() {
        engine.handle(ev);
    }
    if channels == 2 {
        // The common path: the device buffer *is* an interleaved stereo block,
        // so render straight into it.
        engine.render(out);
        return;
    }
    if channels == 0 {
        return;
    }
    let per_chunk = (scratch.len() / 2) * channels;
    for block in out.chunks_mut(per_chunk) {
        let frames = block.len() / channels;
        let stereo = &mut scratch[..frames * 2];
        engine.render(stereo);
        for (i, frame) in block.chunks_mut(channels).enumerate() {
            let (l, r) = (stereo[i * 2], stereo[i * 2 + 1]);
            if frame.len() == 1 {
                // Mono: average, don't pick a side — a hard-panned sound would
                // otherwise vanish.
                frame[0] = (l + r) * 0.5;
            } else {
                frame[0] = l;
                frame[1] = r;
                // Surround: leave the rest silent rather than inventing a
                // centre channel from a stereo mix.
                for s in &mut frame[2..] {
                    *s = 0.0;
                }
            }
        }
    }
}

/// Open the default output device and start rendering `bank`.
///
/// Errors are returned rather than fatal: `kessel run` prints them and plays on
/// in silence. A machine with no sound card should still show the game.
pub fn start(bank: SoundBank) -> Result<AudioHost, String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no output audio device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("no output config: {e}"))?;

    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();
    if format != cpal::SampleFormat::F32 {
        // Every platform this ships on offers f32 by default. Converting from
        // i16/u16 is easy but untested surface, so say so instead of guessing.
        return Err(format!(
            "output format {format:?} is not supported (need f32)"
        ));
    }

    // The engine runs at the *device's* rate, not a preferred one: resampling
    // would be a second signal path to get right for no gain.
    let mut engine = AudioEngine::new(SynthConfig {
        sample_rate,
        ..SynthConfig::default()
    });
    engine.set_bank(bank);

    // The device's default buffer, not a smaller one we ask for. At the sizes
    // hosts choose (256–512 frames, 5–11 ms) the latency is already under one
    // 60 Hz frame, so forcing it lower would trade a real underrun risk on a
    // loaded machine for a delay nobody can hear against a 16.7 ms frame.
    let ring = Arc::new(EventQueue::new());
    let consumer = Arc::clone(&ring);

    // Scratch for devices that are not stereo. Sized once, here, so the
    // callback never allocates; oversized blocks are rendered in chunks.
    const SCRATCH_FRAMES: usize = 4096;
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * 2];

    let stream = device
        .build_output_stream(
            &supported.config(),
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                render_block(&mut engine, &consumer, out, channels, &mut scratch);
            },
            |e| eprintln!("kessel: audio stream error: {e}"),
            None,
        )
        .map_err(|e| format!("could not open the audio stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("could not start audio: {e}"))?;

    Ok(AudioHost {
        _stream: stream,
        ring,
        sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An engine holding one loud, instant, sustaining instrument.
    fn test_engine() -> AudioEngine {
        let (bank, errors) = kessel_audio::bank::parse(
            r#"
            instrument tone { wave = square  attack = 0  decay = 0  sustain = 255  volume = 255 }
            sfx beep { inst = tone  speed = 30  notes = "69" }
            "#,
        );
        assert!(errors.is_empty(), "{errors:?}");
        let mut e = AudioEngine::new(SynthConfig::default());
        e.set_bank(bank);
        e
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }

    #[test]
    fn the_callback_turns_queued_events_into_sound() {
        // Everything the audio thread does, minus the device: an event pushed
        // from the game side comes back as samples.
        let ring = EventQueue::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 512 * 2];
        let mut out = vec![0.0f32; 1024 * 2];

        render_block(&mut engine, &ring, &mut out, 2, &mut scratch);
        assert_eq!(peak(&out), 0.0, "silent until something is queued");

        ring.push(AudioEvent::PlaySfx { id: 0 });
        render_block(&mut engine, &ring, &mut out, 2, &mut scratch);
        assert!(
            peak(&out) > 0.1,
            "queued event never sounded: {}",
            peak(&out)
        );
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn panic_from_the_queue_silences_the_stream() {
        let ring = EventQueue::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 512 * 2];
        let mut out = vec![0.0f32; 1024 * 2];
        ring.push(AudioEvent::PlaySfx { id: 0 });
        render_block(&mut engine, &ring, &mut out, 2, &mut scratch);
        assert!(peak(&out) > 0.1);

        ring.push(AudioEvent::Panic);
        render_block(&mut engine, &ring, &mut out, 2, &mut scratch);
        assert_eq!(peak(&out), 0.0, "a reload left the old game ringing");
    }

    #[test]
    fn a_mono_device_gets_both_channels_averaged() {
        // Not one side: a hard-panned sound would vanish on a mono device.
        let ring = EventQueue::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 512 * 2];
        ring.push(AudioEvent::PlaySfx { id: 0 });
        let mut mono = vec![0.0f32; 900];
        render_block(&mut engine, &ring, &mut mono, 1, &mut scratch);
        assert!(peak(&mono) > 0.1, "mono device was silent");
        assert!(mono.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_surround_device_gets_the_mix_on_the_first_two_channels() {
        let ring = EventQueue::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 512 * 2];
        ring.push(AudioEvent::PlaySfx { id: 0 });
        const CH: usize = 6;
        let mut out = vec![0.0f32; 600 * CH];
        render_block(&mut engine, &ring, &mut out, CH, &mut scratch);

        let front: f32 = out
            .chunks(CH)
            .fold(0.0f32, |a, f| a.max(f[0].abs()).max(f[1].abs()));
        let rest: f32 = out
            .chunks(CH)
            .fold(0.0f32, |a, f| f[2..].iter().fold(a, |m, s| m.max(s.abs())));
        assert!(front > 0.1, "front channels were silent");
        assert_eq!(rest, 0.0, "the mix leaked into the surround channels");
    }

    #[test]
    fn a_block_longer_than_the_scratch_still_renders_whole() {
        // The chunking path: an oversized callback buffer must be filled, not
        // truncated, and without allocating a bigger scratch.
        let ring = EventQueue::new();
        let mut engine = test_engine();
        let mut scratch = vec![0.0f32; 64 * 2]; // deliberately tiny
        ring.push(AudioEvent::PlaySfx { id: 0 });
        let mut out = vec![0.0f32; 1000];
        render_block(&mut engine, &ring, &mut out, 1, &mut scratch);
        // The tail of the buffer is as alive as the head.
        assert!(peak(&out[..100]) > 0.1);
        assert!(peak(&out[900..]) > 0.1, "the block was left half-rendered");
    }
}
