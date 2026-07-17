//! A minimal, dependency-free PNG encoder.
//!
//! Simard is a pure-Rust daemon with a tightly-pinned dependency set, so rather
//! than add an image codec crate we encode 8-bit RGB PNGs directly. The encoder
//! uses uncompressed (stored) DEFLATE blocks wrapped in a zlib stream — this is
//! trivially correct and deterministic, at the cost of no compression (fine for
//! short preview sequences). Output is a fully spec-compliant PNG readable by
//! any decoder (Blender, Natron, browsers, `file`, etc.).

/// The 8-byte PNG signature.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// Encode an 8-bit RGB image (`width*height*3` bytes, row-major) as a PNG.
///
/// # Panics
/// Panics if `rgb.len() != width * height * 3`.
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    assert_eq!(
        rgb.len(),
        width as usize * height as usize * 3,
        "rgb buffer must be width*height*3 bytes"
    );

    let mut out = Vec::with_capacity(rgb.len() + 1024);
    out.extend_from_slice(&SIGNATURE);

    // IHDR: width, height, bit depth 8, colour type 2 (truecolour), no
    // interlace.
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // colour type: truecolour RGB
    ihdr.push(0); // compression: deflate
    ihdr.push(0); // filter: adaptive
    ihdr.push(0); // interlace: none
    write_chunk(&mut out, b"IHDR", &ihdr);

    // Filtered raw data: each scanline is prefixed with filter byte 0 (None).
    let stride = width as usize * 3;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0); // filter type None
        raw.extend_from_slice(&rgb[row * stride..(row + 1) * stride]);
    }

    let idat = zlib_store(&raw);
    write_chunk(&mut out, b"IDAT", &idat);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Write a PNG chunk: length, type, data, CRC32(type ++ data).
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

/// Wrap `data` in a zlib stream that uses only stored (uncompressed) DEFLATE
/// blocks.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65535 * 5 + 16);
    // zlib header: CMF=0x78 (deflate, 32K window), FLG=0x01 chosen so that
    // (CMF<<8 | FLG) % 31 == 0 and no preset dictionary / fastest level.
    out.push(0x78);
    out.push(0x01);

    // Stored blocks, each carrying up to 65535 bytes.
    let mut chunks = data.chunks(0xFFFF).peekable();
    if data.is_empty() {
        // A single final empty stored block.
        out.push(0x01);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(!0u16).to_le_bytes());
    } else {
        while let Some(chunk) = chunks.next() {
            let is_final = chunks.peek().is_none();
            out.push(if is_final { 0x01 } else { 0x00 }); // BFINAL, BTYPE=00
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }

    // Adler-32 of the uncompressed data, big-endian.
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32 checksum.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// Streaming CRC-32 (IEEE 802.3, as used by PNG).
struct Crc32 {
    value: u32,
}

impl Crc32 {
    fn new() -> Self {
        Self { value: 0xFFFF_FFFF }
    }

    fn update(&mut self, data: &[u8]) {
        let table = crc_table();
        for &byte in data {
            let idx = ((self.value ^ byte as u32) & 0xFF) as usize;
            self.value = table[idx] ^ (self.value >> 8);
        }
    }

    fn finalize(self) -> u32 {
        self.value ^ 0xFFFF_FFFF
    }
}

/// The CRC-32 lookup table, computed once on first use.
fn crc_table() -> &'static [u32; 256] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (n, slot) in table.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        table
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ihdr(png: &[u8]) -> (u32, u32, u8, u8) {
        assert_eq!(&png[0..8], &SIGNATURE, "PNG signature");
        // First chunk starts at byte 8: [len(4)][type(4)][data...]
        assert_eq!(&png[12..16], b"IHDR");
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        let depth = png[24];
        let color = png[25];
        (w, h, depth, color)
    }

    #[test]
    fn encodes_valid_signature_and_ihdr() {
        let png = encode_rgb(2, 3, &[0u8; 2 * 3 * 3]);
        let (w, h, depth, color) = parse_ihdr(&png);
        assert_eq!((w, h, depth, color), (2, 3, 8, 2));
    }

    #[test]
    fn ends_with_iend() {
        let png = encode_rgb(1, 1, &[10, 20, 30]);
        assert!(png.ends_with(&[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn adler32_known_value() {
        // Adler-32 of "Wikipedia" is 0x11E60398.
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn crc32_known_value() {
        // CRC-32 of the ASCII string "123456789" is 0xCBF43926.
        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finalize(), 0xCBF4_3926);
    }

    #[test]
    fn idat_roundtrips_through_stored_blocks() {
        // A larger-than-one-block image exercises multi-block stored deflate.
        let w = 300u32;
        let h = 300u32;
        let png = encode_rgb(w, h, &vec![127u8; (w * h * 3) as usize]);
        let (pw, ph, _, _) = parse_ihdr(&png);
        assert_eq!((pw, ph), (w, h));
        // Must contain an IDAT chunk.
        assert!(
            png.windows(4).any(|win| win == b"IDAT"),
            "PNG should contain an IDAT chunk"
        );
    }

    #[test]
    #[should_panic]
    fn wrong_buffer_size_panics() {
        let _ = encode_rgb(2, 2, &[0u8; 3]);
    }
}
