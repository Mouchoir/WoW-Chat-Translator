//! Checksums used by the bridge.

const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = make_table();

/// CRC-32 (zlib / ISO-HDLC: reflected polynomial 0xEDB88320, init and final xor 0xFFFFFFFF).
/// Used for whole strip frames and for whole messages (CHUNK msgCrc).
pub fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in d {
        c = (c >> 8) ^ TABLE[((c ^ b as u32) & 0xFF) as usize];
    }
    !c
}

/// CRC-5 of a row tag's 10-bit payload (`slot << 3 | row << 1 | sessBit`).
/// Polynomial 0x05, init 0x1F, payload bits fed most significant first, no reflection, no final xor.
/// Only the low 10 bits of `payload10` are used.
pub fn crc5_tag(payload10: u16) -> u8 {
    let mut c: u8 = 0x1F;
    for i in (0..10).rev() {
        let bit = ((payload10 >> i) & 1) as u8;
        let top = (c >> 4) & 1;
        c = (c << 1) & 0x1F;
        if bit != top {
            c ^= 0x05;
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn crc5_check_values() {
        // (slot, row, sessBit) -> crc5, from the design's check table
        assert_eq!(crc5_tag(0), 25);
        assert_eq!(crc5_tag(5 << 3 | 2 << 1), 17);
        assert_eq!(crc5_tag(127 << 3), 23);
    }

    #[test]
    fn crc5_detects_every_single_bit_error() {
        for p in 0u16..1024 {
            let v = (p << 5) | crc5_tag(p) as u16;
            for b in 0..15 {
                let w = v ^ (1 << b);
                assert_ne!(crc5_tag(w >> 5) as u16, w & 0x1F, "payload {p} bit {b}");
            }
        }
    }
}
