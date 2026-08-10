//! A minimal, dependency-free WAV writer.
//!
//! Just enough to turn a render into a file an ear (or `kessel render-audio`)
//! can check — 16-bit PCM, no chunks beyond `fmt ` and `data`. Same ethos as
//! the hand-rolled PNG encoder in `kessel-vm`: the format is 44 bytes of header
//! and a pile of samples, and a crate dependency for that is not a trade.

/// Encode interleaved f32 samples in `[-1, 1]` as a 16-bit PCM WAV.
///
/// Samples outside the range are clamped rather than wrapped: a render that
/// overshoots should sound loud, not shredded.
pub fn encode_pcm16(sample_rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
    let bits = 16u16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * block_align as u32;
    let data_len = (samples.len() * 2) as u32;

    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // format: PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_describes_the_data_that_follows() {
        let samples = vec![0.0f32; 100];
        let wav = encode_pcm16(48_000, 2, &samples);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 200);
        let riff_len = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        assert_eq!(riff_len as usize, wav.len() - 8);
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_len as usize, wav.len() - 44);
        // 2 channels × 16 bits = 4 bytes per frame, 48 kHz × 4 byte rate.
        assert_eq!(u16::from_le_bytes(wav[32..34].try_into().unwrap()), 4);
        assert_eq!(
            u32::from_le_bytes(wav[28..32].try_into().unwrap()),
            48_000 * 4
        );
    }

    #[test]
    fn out_of_range_samples_clamp_instead_of_wrapping() {
        let wav = encode_pcm16(48_000, 1, &[2.0, -2.0]);
        let a = i16::from_le_bytes(wav[44..46].try_into().unwrap());
        let b = i16::from_le_bytes(wav[46..48].try_into().unwrap());
        assert_eq!(a, i16::MAX);
        assert_eq!(b, -i16::MAX);
    }
}
