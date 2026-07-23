//! Minimal, dependency-free PNG + base64 encoders.
//!
//! Just enough to turn the VM framebuffer into a base64 PNG for vision tool
//! output. The PNG uses truecolour-with-alpha (8-bit) and a *stored* (level-0)
//! zlib stream — no compression, but tiny and correct — matching this repo's
//! hand-rolled ethos (see `model_downloader`).

/// Encode raw RGBA (`width*height*4` bytes) as a PNG byte stream.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        (width * height * 4) as usize,
        "rgba size mismatch"
    );

    // Build the raw (filtered) image data: each scanline prefixed with filter 0.
    let stride = (width * 4) as usize;
    let mut raw = Vec::with_capacity((height as usize) * (1 + stride));
    for row in 0..height as usize {
        raw.push(0); // filter type: None
        raw.extend_from_slice(&rgba[row * stride..(row + 1) * stride]);
    }

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

    // IHDR
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth 8, colour type 6 (RGBA)
    write_chunk(&mut out, b"IHDR", &ihdr);

    // IDAT (zlib stored)
    let idat = zlib_store(&raw);
    write_chunk(&mut out, b"IDAT", &idat);

    // IEND
    write_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Wrap `data` in a zlib stream using only stored (uncompressed) deflate blocks.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0x78, 0x01]); // zlib header: 32K window, no dict

    // Deflate stored blocks, max 0xFFFF bytes each.
    if data.is_empty() {
        // A single empty final stored block.
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    } else {
        let mut i = 0;
        while i < data.len() {
            let chunk = &data[i..(i + 0xffff).min(data.len())];
            let is_last = i + chunk.len() >= data.len();
            out.push(if is_last { 0x01 } else { 0x00 }); // BFINAL, BTYPE=00
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
            i += chunk.len();
        }
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// Standard base64 (with padding).
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn crc32_known_vector() {
        // CRC-32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn adler32_known_vector() {
        // Adler-32 of "Wikipedia" is 0x11E60398.
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn png_has_signature_and_chunks() {
        let w = 2;
        let h = 2;
        let rgba = vec![255u8; (w * h * 4) as usize];
        let png = encode_rgba(w, h, &rgba);
        assert_eq!(
            &png[0..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        // Contains IHDR, IDAT, IEND chunk names.
        let s = png.windows(4).any(|w| w == b"IHDR");
        assert!(s);
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert!(png.windows(4).any(|w| w == b"IEND"));
    }

    #[test]
    fn png_large_image_multiblock() {
        // 128x128 RGBA filtered data exceeds one 0xFFFF stored block; must still
        // encode without panic and be non-trivial in size.
        let (w, h) = (128u32, 128u32);
        let rgba = vec![0x40u8; (w * h * 4) as usize];
        let png = encode_rgba(w, h, &rgba);
        assert!(png.len() > 60_000);
    }
}
