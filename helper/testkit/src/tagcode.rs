//! Row tag colours (bridge v2, docs\BRIDGE.md): a magenta/green sync pair, then 6 base-6 digits of
//! `v = payload10 << 5 | crc5` with `payload10 = slot << 3 | row << 1 | sessBit`. fakewow decodes the
//! tags it lays out (to write the truth) and the tags it reads back from its own window (self-check).

pub const SYNC: [[u8; 3]; 2] = [[255, 0, 255], [0, 255, 0]];
/// 0 black, 1 red, 2 blue, 3 yellow, 4 cyan, 5 white.
pub const DIGITS: [[u8; 3]; 6] = [[0, 0, 0], [255, 0, 0], [0, 0, 255], [255, 255, 0], [0, 255, 255], [255, 255, 255]];
pub const PLATE_RGB: [u8; 3] = [32, 40, 64];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TagId {
    pub slot: u8,
    pub row: u8,
    pub sess_bit: u8,
}

/// CRC-5 of a 10-bit payload (design C.4).
pub fn crc5(payload10: u16) -> u8 {
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

pub fn encode(id: TagId) -> [[u8; 3]; 8] {
    let payload = (u16::from(id.slot) << 3) | (u16::from(id.row) << 1) | u16::from(id.sess_bit & 1);
    let mut v = (u32::from(payload) << 5) | u32::from(crc5(payload));
    let mut digits = [0usize; 6];
    for d in digits.iter_mut().rev() {
        *d = (v % 6) as usize;
        v /= 6;
    }
    let mut out = [[0u8; 3]; 8];
    out[0] = SYNC[0];
    out[1] = SYNC[1];
    for (i, d) in digits.iter().enumerate() {
        out[i + 2] = DIGITS[*d];
    }
    out
}

/// Decodes 8 exact tag colours. None when the sync pair, a digit or the CRC is wrong.
pub fn decode(colors: &[[u8; 3]]) -> Option<TagId> {
    if colors.len() != 8 || colors[0] != SYNC[0] || colors[1] != SYNC[1] {
        return None;
    }
    let mut v: u32 = 0;
    for c in &colors[2..] {
        let d = DIGITS.iter().position(|x| x == c)?;
        v = v * 6 + d as u32;
    }
    from_value(v)
}

fn from_value(v: u32) -> Option<TagId> {
    if v >= 32768 {
        return None;
    }
    let payload = (v >> 5) as u16;
    if crc5(payload) != (v & 31) as u8 {
        return None;
    }
    Some(TagId { slot: (payload >> 3) as u8, row: ((payload >> 1) & 3) as u8, sess_bit: (payload & 1) as u8 })
}

/// Decodes 8 observed colours by nearest reference (`refs` = the 8 colours of SYNC then DIGITS as
/// they look after the display transform). Used to read tags back from a captured image.
pub fn decode_observed(obs: &[[f32; 3]; 8], refs: &[[f32; 3]; 8]) -> Option<TagId> {
    let nearest = |p: [f32; 3]| -> usize {
        let mut best = (f32::MAX, 0);
        for (i, r) in refs.iter().enumerate() {
            let d = (0..3).map(|c| (p[c] - r[c]).abs()).fold(0.0, f32::max);
            if d < best.0 {
                best = (d, i);
            }
        }
        best.1
    };
    if nearest(obs[0]) != 0 || nearest(obs[1]) != 1 {
        return None;
    }
    let mut v: u32 = 0;
    for p in &obs[2..] {
        let k = nearest(*p);
        if k < 2 {
            return None;
        }
        v = v * 6 + (k - 2) as u32;
    }
    from_value(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_check_values() {
        // (slot, row, sessBit) -> crc5, from the design's check table
        assert_eq!(crc5(0), 25);
        assert_eq!(crc5(44), 17);
        assert_eq!(crc5(1016), 23);
        let c = encode(TagId { slot: 0, row: 0, sess_bit: 0 });
        assert_eq!(&c[2..], &[DIGITS[0], DIGITS[0], DIGITS[0], DIGITS[0], DIGITS[4], DIGITS[1]]);
        let c = encode(TagId { slot: 127, row: 0, sess_bit: 0 });
        assert_eq!(&c[2..], &[DIGITS[4], DIGITS[1], DIGITS[0], DIGITS[3], DIGITS[4], DIGITS[3]]);
    }

    #[test]
    fn round_trip_every_tag() {
        for slot in 0..=127u8 {
            for row in 0..4u8 {
                for sess_bit in 0..2u8 {
                    let id = TagId { slot, row, sess_bit };
                    let c = encode(id);
                    assert_eq!(decode(&c), Some(id));
                    let mut obs = [[0f32; 3]; 8];
                    for (o, x) in obs.iter_mut().zip(c.iter()) {
                        *o = [f32::from(x[0]) * 0.9 + 10.0, f32::from(x[1]) * 0.9 + 10.0, f32::from(x[2]) * 0.9 + 10.0];
                    }
                    let mut refs = [[0f32; 3]; 8];
                    for (i, r) in SYNC.iter().chain(DIGITS.iter()).enumerate() {
                        refs[i] = [f32::from(r[0]) * 0.9 + 10.0, f32::from(r[1]) * 0.9 + 10.0, f32::from(r[2]) * 0.9 + 10.0];
                    }
                    assert_eq!(decode_observed(&obs, &refs), Some(id));
                }
            }
        }
    }

    #[test]
    fn rejects_bad_crc() {
        let mut c = encode(TagId { slot: 5, row: 2, sess_bit: 0 });
        c[7] = if c[7] == DIGITS[0] { DIGITS[1] } else { DIGITS[0] };
        assert_eq!(decode(&c), None);
    }
}
