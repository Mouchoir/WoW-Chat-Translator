//! Strip decoder (bridge v2).
//!
//! The strip is 4 physical px tall at the client's top-left: block k is 2x2 px at
//! `x = 2 * (k / 2)`, `y = 2 * (k % 2)`. Blocks 0-7 are calibration greys (level index
//! 0,3,3,0,1,2,2,1 of the levels 0/104/146/255), then the frame bytes, most significant bit first:
//! dense 6 bits per block (magic 0xD5), robust 3 bits (0x2A), mono 2 bits, grey only (0x5A).
//!
//! Frame (big-endian): magic u8, version u8 = 2, seq u8, flags u8 = 0 (ignored), session u16,
//! body length u16, records, CRC-32 of everything before it. Records are `type u8, len u16, data`:
//! 0x01 STATUS (physW u16, physH u16, addonBuild u16), 0x03 CHUNK (msgId u16, idx u8, count u8,
//! total u16, msgCrc u32, data); unknown types (0x02 = the old LAYOUT) are skipped.
//!
//! Decoding (normative in docs/BRIDGE.md): pick the sub-pixel phases whose calibration contrast is
//! at least 0.8 of the best (absent below 16), average each block over them, check the
//! calibration copies agree (torn above 20), then try dense (only when the smallest level gap is
//! at least 8), robust and mono: read the header, check magic, version and length, classify the
//! data blocks (dense and mono re-estimate the four levels from the frame's own blocks first),
//! check the CRC-32 and parse the records strictly.

use crate::{crc, image};

/// Symbol alphabet of a frame, given by its magic byte.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Dense,
    Robust,
    Mono,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Dense, Mode::Robust, Mode::Mono];

    pub fn magic(self) -> u8 {
        match self {
            Mode::Dense => 0xD5,
            Mode::Robust => 0x2A,
            Mode::Mono => 0x5A,
        }
    }

    /// Data bits per block.
    pub fn bits(self) -> usize {
        match self {
            Mode::Dense => 6,
            Mode::Robust => 3,
            Mode::Mono => 2,
        }
    }

    /// Lower-case name ("dense", "robust", "mono") for logs.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Dense => "dense",
            Mode::Robust => "robust",
            Mode::Mono => "mono",
        }
    }

    /// The addon's mode number (0 dense, 1 robust, 2 mono).
    pub fn index(self) -> u8 {
        match self {
            Mode::Dense => 0,
            Mode::Robust => 1,
            Mode::Mono => 2,
        }
    }
}

/// Per-channel references measured from the calibration blocks of one frame.
#[derive(Clone, Debug)]
pub struct Calib {
    pub black: [f32; 3],
    pub white: [f32; 3],
    /// Smallest gap between two adjacent levels on any channel (signal quality).
    pub min_gap: f32,
    /// The four measured reference levels per channel (drawn 0, 104, 146, 255): the display curve
    /// between the game and the capture, for `tone::Tone`.
    pub levels: [[f32; 3]; 4],
}

/// One CHUNK record.
pub struct Chunk {
    pub msg_id: u16,
    pub idx: u8,
    pub count: u8,
    pub total: u16,
    pub msg_crc: u32,
    pub data: Vec<u8>,
}

/// A frame that passed the CRC and the strict record parse.
pub struct Frame {
    pub mode: Mode,
    pub seq: u8,
    pub session: u16,
    pub phys_w: u16,
    pub phys_h: u16,
    pub addon_build: u16,
    pub chunks: Vec<Chunk>,
    pub calib: Calib,
}

/// Result of reading the strip from one capture.
pub enum StripRead {
    /// No calibration contrast: the strip is not on screen (idle, Alt-Z, loading screen).
    Absent,
    /// Calibration copies disagree (tearing or a partial update).
    Torn,
    /// Calibration fine but the magic, version, length, CRC or records are wrong.
    Rejected,
    Frame(Frame),
}

/// Why a capture did not give a frame (counters and diagnostics; never any payload).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    None,
    Absent,
    Torn,
    /// No mode's magic byte matched.
    NoMagic,
    BadVersion,
    /// The header's length runs past the captured blocks.
    TooLong,
    Crc,
    /// CRC fine but the header or the records are malformed.
    Malformed,
}

/// Details of one decode: the calibration (when the strip was present) and the reject reason.
#[derive(Clone, Debug)]
pub struct DecodeDetail {
    pub calib: Option<Calib>,
    pub reason: Reject,
}

pub const VERSION: u8 = 2;
pub const LEVELS: [u8; 4] = [0, 104, 146, 255];
pub const CALIB_LEVELS: [usize; 8] = [0, 3, 3, 0, 1, 2, 2, 1];
pub const NCAL: usize = 8;
pub const HDR_LEN: usize = 8;
pub const CRC_LEN: usize = 4;
pub const REC_HDR: usize = 3;
pub const REC_STATUS: u8 = 0x01;
pub const REC_CHUNK: u8 = 0x03;
pub const STATUS_DATA: usize = 6;
pub const CHUNK_FIXED: usize = 10;
/// Calibration contrast (white - black, worst channel) below which the strip is absent.
pub const PRESENCE_MIN: f32 = 16.0;
/// Largest disagreement between the two copies of a calibration level.
pub const CONSIST_MAX: f32 = 20.0;
/// Smallest adjacent-level gap at which dense mode is tried.
pub const GAP_MIN_DENSE: f32 = 8.0;

/// Bytes (header + body + CRC) that fit in a strip of `ncols` block columns.
pub fn frame_capacity(ncols: usize, mode: Mode) -> usize {
    (2 * ncols).saturating_sub(NCAL) * mode.bits() / 8
}

/// Chunk data bytes per frame (one STATUS and one CHUNK record).
pub fn max_chunk_data(ncols: usize, mode: Mode) -> usize {
    frame_capacity(ncols, mode).saturating_sub(HDR_LEN + CRC_LEN + REC_HDR + STATUS_DATA + REC_HDR + CHUNK_FIXED)
}

/// Decodes the strip. `img` covers client rows 0..8 from x = 0 (at least as wide as the strip).
pub fn decode_strip(img: &image::Bgra) -> StripRead {
    decode_strip_detail(img).0
}

/// [`decode_strip`] plus the calibration and the reject reason.
pub fn decode_strip_detail(img: &image::Bgra) -> (StripRead, DecodeDetail) {
    let absent = |r: StripRead, reason| (r, DecodeDetail { calib: None, reason });
    if img.origin_x > 0 || img.origin_y > 0 {
        return absent(StripRead::Absent, Reject::Absent);
    }
    // pixel columns 0..2*ncols and rows 0..4 must be inside the capture
    let right = i64::from(img.origin_x) + i64::from(img.width);
    let bottom = i64::from(img.origin_y) + i64::from(img.height);
    if bottom < 4 || right < 16 {
        return absent(StripRead::Absent, Reject::Absent);
    }
    let ncols = (right / 2) as usize;
    let nblocks = 2 * ncols;
    // client rows 0..4 as byte rows (the capture starts at x = 0, y = 0 or above)
    let stride = img.stride as usize;
    let rows: [&[u8]; 4] = std::array::from_fn(|y| {
        let o = (y as i64 - i64::from(img.origin_y)) as usize * stride + (-img.origin_x) as usize * 4;
        &img.px[o.min(img.px.len())..]
    });
    let px = |k: usize, phase: usize| -> [f32; 3] {
        let x = 2 * (k / 2) + (phase & 1);
        let y = 2 * (k % 2) + (phase >> 1);
        match rows[y].get(4 * x..4 * x + 3) {
            Some(p) => [f32::from(p[2]), f32::from(p[1]), f32::from(p[0])],
            None => [0.0; 3],
        }
    };
    // 1. sub-pixel phases: calibration contrast per phase (worst channel)
    let mut contrast = [0f32; 4];
    for (phase, out) in contrast.iter_mut().enumerate() {
        let c: Vec<[f32; 3]> = (0..4).map(|k| px(k, phase)).collect();
        let mut worst = f32::MAX;
        for ch in 0..3 {
            let v = (c[1][ch] - c[0][ch]).min(c[2][ch] - c[0][ch]).min(c[1][ch] - c[3][ch]).min(c[2][ch] - c[3][ch]);
            worst = worst.min(v);
        }
        *out = worst;
    }
    let best = contrast.iter().cloned().fold(f32::MIN, f32::max);
    if best < PRESENCE_MIN {
        return absent(StripRead::Absent, Reject::Absent);
    }
    let phases: Vec<usize> = (0..4).filter(|&p| contrast[p] >= 0.8 * best).collect();
    // 2. block means over the kept phases, computed as far as the frame needs them
    let inv = 1.0 / phases.len() as f32;
    let block = |k: usize| -> [f32; 3] {
        let mut s = [0f32; 3];
        for &p in &phases {
            let v = px(k, p);
            for ch in 0..3 {
                s[ch] += v[ch];
            }
        }
        [s[0] * inv, s[1] * inv, s[2] * inv]
    };
    let mut means: Vec<[f32; 3]> = Vec::with_capacity(nblocks);
    let upto = |means: &mut Vec<[f32; 3]>, n: usize| {
        while means.len() < n.min(nblocks) {
            means.push(block(means.len()));
        }
    };
    upto(&mut means, NCAL);
    let c: [[f32; 3]; NCAL] = std::array::from_fn(|k| means[k]);
    let c = &c[..];
    // 3. calibration copies must agree
    let mut cons = 0f32;
    for (a, b) in [(0, 3), (1, 2), (4, 7), (5, 6)] {
        for ch in 0..3 {
            cons = cons.max((c[a][ch] - c[b][ch]).abs());
        }
    }
    if cons > CONSIST_MAX {
        return absent(StripRead::Torn, Reject::Torn);
    }
    let mut refs = [[0f32; 3]; 4];
    for ch in 0..3 {
        refs[0][ch] = (c[0][ch] + c[3][ch]) / 2.0;
        refs[1][ch] = (c[4][ch] + c[7][ch]) / 2.0;
        refs[2][ch] = (c[5][ch] + c[6][ch]) / 2.0;
        refs[3][ch] = (c[1][ch] + c[2][ch]) / 2.0;
    }
    let mut min_gap = f32::MAX;
    for l in 0..3 {
        for ch in 0..3 {
            min_gap = min_gap.min(refs[l + 1][ch] - refs[l][ch]);
        }
    }
    let calib = Calib { black: refs[0], white: refs[3], min_gap, levels: refs };
    let ndata = nblocks - NCAL;
    let detail = |reason| DecodeDetail { calib: Some(calib.clone()), reason };
    // 4. modes
    let mut reason = Reject::NoMagic;
    for mode in Mode::ALL {
        if mode == Mode::Dense && min_gap < GAP_MIN_DENSE {
            continue;
        }
        let bps = mode.bits();
        let hdr_syms = (HDR_LEN * 8).div_ceil(bps);
        if ndata < hdr_syms {
            continue;
        }
        upto(&mut means, NCAL + hdr_syms);
        let head = from_symbols(&classify(&means[NCAL..NCAL + hdr_syms], &refs, mode), bps, HDR_LEN);
        let Some(head) = head else { continue };
        if head[0] != mode.magic() {
            continue;
        }
        if head[1] != VERSION {
            return (StripRead::Rejected, detail(Reject::BadVersion));
        }
        let total = HDR_LEN + usize::from(u16::from_be_bytes([head[6], head[7]])) + CRC_LEN;
        let nsym = (total * 8).div_ceil(bps);
        if nsym > ndata {
            return (StripRead::Rejected, detail(Reject::TooLong));
        }
        upto(&mut means, NCAL + nsym);
        let data = &means[NCAL..];
        let first = classify(&data[..nsym], &refs, mode);
        let mut tries = Vec::with_capacity(2);
        if mode != Mode::Robust {
            if let Some(r2) = refine(c, &data[..nsym], &refs, &first, mode) {
                tries.push(classify(&data[..nsym], &r2, mode));
            }
        }
        tries.push(first);
        reason = Reject::Crc;
        for sy in &tries {
            let Some(fb) = from_symbols(sy, bps, total) else { continue };
            let want = u32::from_be_bytes([fb[total - 4], fb[total - 3], fb[total - 2], fb[total - 1]]);
            if crc::crc32(&fb[..total - 4]) != want {
                continue;
            }
            return match parse_frame(&fb, mode, calib.clone()) {
                Some(f) => (StripRead::Frame(f), detail(Reject::None)),
                None => (StripRead::Rejected, detail(Reject::Malformed)),
            };
        }
        break;
    }
    (StripRead::Rejected, detail(reason))
}

/// Symbols of the data blocks from the observed reference levels (4 levels x 3 channels).
fn classify(data: &[[f32; 3]], refs: &[[f32; 3]; 4], mode: Mode) -> Vec<u8> {
    match mode {
        Mode::Dense => {
            let mut th = [[0f32; 3]; 3];
            for l in 0..3 {
                for ch in 0..3 {
                    th[l][ch] = (refs[l][ch] + refs[l + 1][ch]) / 2.0;
                }
            }
            data.iter()
                .map(|v| {
                    let mut s = 0u8;
                    for ch in 0..3 {
                        let lv = th.iter().filter(|t| v[ch] > t[ch]).count() as u8;
                        s = (s << 2) | lv;
                    }
                    s
                })
                .collect()
        }
        Mode::Robust => {
            let th: Vec<f32> = (0..3).map(|ch| (refs[0][ch] + refs[3][ch]) / 2.0).collect();
            data.iter()
                .map(|v| {
                    let mut s = 0u8;
                    for ch in 0..3 {
                        s = (s << 1) | u8::from(v[ch] > th[ch]);
                    }
                    s
                })
                .collect()
        }
        Mode::Mono => {
            let rg: Vec<f32> = refs.iter().map(|r| (r[0] + r[1] + r[2]) / 3.0).collect();
            let th = [(rg[0] + rg[1]) / 2.0, (rg[1] + rg[2]) / 2.0, (rg[2] + rg[3]) / 2.0];
            data.iter()
                .map(|v| {
                    let g = (v[0] + v[1] + v[2]) / 3.0;
                    th.iter().filter(|&&t| g > t).count() as u8
                })
                .collect()
        }
    }
}

/// Decision-directed refinement: each level re-estimated from every block of this frame
/// (calibration + data) per channel; None when the refined levels are not increasing.
fn refine(calib: &[[f32; 3]], data: &[[f32; 3]], refs: &[[f32; 3]; 4], syms: &[u8], mode: Mode) -> Option<[[f32; 3]; 4]> {
    let mut sum = [[0f64; 3]; 4];
    let mut n = [[0u32; 3]; 4];
    let mut add = |lv: [usize; 3], v: &[f32; 3]| {
        for ch in 0..3 {
            sum[lv[ch]][ch] += f64::from(v[ch]);
            n[lv[ch]][ch] += 1;
        }
    };
    for (k, v) in calib.iter().enumerate() {
        let l = CALIB_LEVELS[k];
        add([l, l, l], v);
    }
    for (s, v) in syms.iter().zip(data) {
        let s = usize::from(*s);
        let lv = if mode == Mode::Dense { [(s >> 4) & 3, (s >> 2) & 3, s & 3] } else { [s, s, s] };
        add(lv, v);
    }
    let mut r2 = *refs;
    for l in 0..4 {
        for ch in 0..3 {
            if n[l][ch] > 0 {
                r2[l][ch] = (sum[l][ch] / f64::from(n[l][ch])) as f32;
            }
        }
    }
    let ok = if mode == Mode::Mono {
        let g: Vec<f32> = r2.iter().map(|r| r[0] + r[1] + r[2]).collect();
        g.windows(2).all(|w| w[1] > w[0])
    } else {
        (0..3).all(|l| (0..3).all(|ch| r2[l + 1][ch] > r2[l][ch]))
    };
    ok.then_some(r2)
}

/// Packs symbols of `bps` bits (most significant first) into exactly `nbytes` bytes.
fn from_symbols(syms: &[u8], bps: usize, nbytes: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(nbytes);
    let mut acc: u32 = 0;
    let mut nacc = 0usize;
    for &s in syms {
        acc = (acc << bps) | u32::from(s);
        nacc += bps;
        if nacc >= 8 {
            nacc -= 8;
            out.push((acc >> nacc) as u8);
            acc &= (1 << nacc) - 1;
            if out.len() == nbytes {
                return Some(out);
            }
        }
    }
    None
}

/// Strict parse of a CRC-valid frame (bytes including the CRC).
fn parse_frame(fb: &[u8], mode: Mode, calib: Calib) -> Option<Frame> {
    if fb.len() < HDR_LEN + CRC_LEN || fb[0] != mode.magic() || fb[1] != VERSION {
        return None;
    }
    let session = u16::from_be_bytes([fb[4], fb[5]]);
    let body = usize::from(u16::from_be_bytes([fb[6], fb[7]]));
    if session == 0 || HDR_LEN + body + CRC_LEN != fb.len() {
        return None;
    }
    let end = HDR_LEN + body;
    let mut p = HDR_LEN;
    let mut status = None;
    let mut chunks = Vec::new();
    let mut first = true;
    while p < end {
        if p + REC_HDR > end {
            return None;
        }
        let t = fb[p];
        let len = usize::from(u16::from_be_bytes([fb[p + 1], fb[p + 2]]));
        if p + REC_HDR + len > end {
            return None;
        }
        let d = &fb[p + REC_HDR..p + REC_HDR + len];
        p += REC_HDR + len;
        match t {
            REC_STATUS => {
                if len != STATUS_DATA || !first {
                    return None;
                }
                let r = |i: usize| u16::from_be_bytes([d[i], d[i + 1]]);
                status = Some((r(0), r(2), r(4)));
            }
            REC_CHUNK => {
                if len < CHUNK_FIXED {
                    return None;
                }
                let (count, idx) = (d[3], d[2]);
                let total = u16::from_be_bytes([d[4], d[5]]);
                let data = d[CHUNK_FIXED..].to_vec();
                if count == 0 || idx >= count || data.len() > usize::from(total) {
                    return None;
                }
                chunks.push(Chunk {
                    msg_id: u16::from_be_bytes([d[0], d[1]]),
                    idx,
                    count,
                    total,
                    msg_crc: u32::from_be_bytes([d[6], d[7], d[8], d[9]]),
                    data,
                });
            }
            _ => {} // unknown record types (0x02 = the v1 LAYOUT) are skipped
        }
        first = false;
    }
    let (phys_w, phys_h, addon_build) = status?;
    Some(Frame { mode, seq: fb[2], session, phys_w, phys_h, addon_build, chunks, calib })
}

/// Parses frame bytes (header, records, CRC) without pixels: for tests and tools. The CRC is
/// checked; the calibration is set to the nominal levels.
pub fn parse_frame_bytes(fb: &[u8]) -> Option<Frame> {
    if fb.len() < HDR_LEN + CRC_LEN {
        return None;
    }
    let n = fb.len();
    if crc::crc32(&fb[..n - 4]) != u32::from_be_bytes([fb[n - 4], fb[n - 3], fb[n - 2], fb[n - 1]]) {
        return None;
    }
    let mode = Mode::ALL.into_iter().find(|m| m.magic() == fb[0])?;
    let calib = Calib { black: [0.0; 3], white: [255.0; 3], min_gap: 42.0, levels: [[0.0; 3], [104.0; 3], [146.0; 3], [255.0; 3]] };
    parse_frame(fb, mode, calib)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_table() {
        // design C.4: 2560 px / 1920 px / 1366 px screens
        assert_eq!(max_chunk_data(1280, Mode::Dense), 1880);
        assert_eq!(max_chunk_data(1280, Mode::Robust), 923);
        assert_eq!(max_chunk_data(1280, Mode::Mono), 604);
        assert_eq!(max_chunk_data(960, Mode::Dense), 1400);
        assert_eq!(max_chunk_data(960, Mode::Mono), 444);
        assert_eq!(max_chunk_data(683, Mode::Dense), 984);
        assert_eq!(max_chunk_data(683, Mode::Mono), 305);
    }

    #[test]
    fn symbols_round_trip() {
        let b = [0xD5u8, 0x02, 0x7F, 0x00, 0xAB, 0xCD, 0x00, 0x01];
        for mode in Mode::ALL {
            let syms = crate::synth::to_symbols(&b, mode);
            assert_eq!(from_symbols(&syms, mode.bits(), b.len()).unwrap(), b);
        }
    }
}
