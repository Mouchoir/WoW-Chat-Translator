//! The scenario file written by `tests\e2e\make_scenario.js` (format 1).
//!
//! ```text
//! { format: 1, name, locale, physW, physH, durationMs, helperStartMs, session,
//!   display: {gamma, contrast}, fade: {visible, duration} (seconds),
//!   events: [ { t (ms from the scenario start), any of:
//!     ui: bool                                   UIParent shown (false = Alt-Z)
//!     frames: [ {id, name, shown, clip: [x,y,w,h] px, fontPx, ppu, spacingPx, scroll} ]
//!     history: [ {id, entries: [ {m, rgb: [r,g,b], ev, ts (ms)} ] } ]   oldest first, last 40
//!     strip: {shown, rgb: hex (6 per block, block k at x = 2*floor(k/2), y = 2*(k%2))}
//!     sent: {kind, req, slot, rows, bytes}       a message handed to Bridge.Send
//!     cover: {id, on, rect?: [x,y,w,h], frame?, slot?, row?, fx?: [from, to], pad?: [t,r,b,l]}
//!     drop: {on, req, slot}                      strip not painted while on (a missed request)
//!     mark: string                               annotation only
//! } ] }
//! ```

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Display {
    #[serde(default = "one")]
    pub gamma: f64,
    #[serde(default = "one")]
    pub contrast: f64,
}

fn one() -> f64 {
    1.0
}

impl Default for Display {
    fn default() -> Self {
        Display { gamma: 1.0, contrast: 1.0 }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Fade {
    /// Seconds a line stays fully visible (WoW chat frames: 120).
    pub visible: f64,
    /// Seconds of the fade out (WoW: 3).
    pub duration: f64,
}

impl Default for Fade {
    fn default() -> Self {
        Fade { visible: 120.0, duration: 3.0 }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FrameState {
    pub id: u8,
    #[serde(default)]
    pub name: String,
    pub shown: bool,
    /// Chat clip rectangle (FontStringContainer) in client px, y down.
    pub clip: [i32; 4],
    #[serde(rename = "fontPx")]
    pub font_px: i32,
    /// Physical px per chat-frame unit (inline texture sizes are in units).
    pub ppu: f64,
    #[serde(rename = "spacingPx", default)]
    pub spacing_px: f64,
    #[serde(default)]
    pub scroll: i32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Entry {
    pub m: String,
    #[serde(default = "white")]
    pub rgb: [u8; 3],
    #[serde(default)]
    pub ev: String,
    /// Entry timestamp (ms from the scenario start); TransformMessages resets it.
    pub ts: f64,
}

fn white() -> [u8; 3] {
    [255, 255, 255]
}

#[derive(Clone, Debug, Deserialize)]
pub struct History {
    pub id: u8,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Strip {
    pub shown: bool,
    #[serde(default)]
    pub rgb: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Sent {
    pub kind: String,
    #[serde(default)]
    pub req: u32,
    #[serde(default)]
    pub slot: u32,
    #[serde(default)]
    pub rows: u32,
    #[serde(default)]
    pub bytes: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Cover {
    pub id: u32,
    pub on: bool,
    /// Absolute rectangle (client px).
    #[serde(default)]
    pub rect: Option<[i32; 4]>,
    /// Or relative to a plate, resolved when the cover appears: chat frame id, slot and row.
    #[serde(default)]
    pub frame: Option<u8>,
    #[serde(default)]
    pub slot: Option<u8>,
    #[serde(default)]
    pub row: Option<u8>,
    /// Horizontal part of the plate covered, as fractions of its width.
    #[serde(default)]
    pub fx: Option<[f64; 2]>,
    /// Extension beyond that part: top, right, bottom, left (px).
    #[serde(default)]
    pub pad: Option<[i32; 4]>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Drop {
    pub on: bool,
    #[serde(default)]
    pub req: u32,
    #[serde(default)]
    pub slot: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    pub t: f64,
    #[serde(default)]
    pub ui: Option<bool>,
    #[serde(default)]
    pub frames: Option<Vec<FrameState>>,
    #[serde(default)]
    pub history: Option<Vec<History>>,
    #[serde(default)]
    pub strip: Option<Strip>,
    #[serde(default)]
    pub sent: Option<Sent>,
    #[serde(default)]
    pub cover: Option<Cover>,
    #[serde(default)]
    pub drop: Option<Drop>,
    #[serde(default)]
    pub mark: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Scenario {
    pub format: u32,
    pub name: String,
    #[serde(rename = "physW")]
    pub phys_w: i32,
    #[serde(rename = "physH")]
    pub phys_h: i32,
    #[serde(rename = "durationMs")]
    pub duration_ms: f64,
    #[serde(rename = "helperStartMs", default)]
    pub helper_start_ms: f64,
    #[serde(default)]
    pub session: u32,
    #[serde(default)]
    pub display: Display,
    #[serde(default)]
    pub fade: Fade,
    pub events: Vec<Event>,
}

impl Scenario {
    pub fn parse(text: &str) -> Result<Scenario, String> {
        let s: Scenario = serde_json::from_str(text).map_err(|e| format!("scenario JSON: {e}"))?;
        if s.format != 1 {
            return Err(format!("scenario format {} (expected 1)", s.format));
        }
        if s.phys_w < 64 || s.phys_h < 64 || s.phys_w > 7680 || s.phys_h > 4320 {
            return Err("scenario screen size out of range".into());
        }
        if s.events.windows(2).any(|w| w[1].t < w[0].t) {
            return Err("scenario events are not in time order".into());
        }
        Ok(s)
    }
}

/// Strip block colours from the hex string (6 hex digits per block).
pub fn strip_blocks(hex: &str) -> Vec<[u8; 3]> {
    let b = hex.as_bytes();
    let d = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
    b.chunks_exact(6).map(|c| [d(c[0]) * 16 + d(c[1]), d(c[2]) * 16 + d(c[3]), d(c[4]) * 16 + d(c[5])]).collect()
}

/// Client position of strip block k (2x2 px).
pub fn block_pos(k: usize) -> (i32, i32) {
    (2 * (k as i32 / 2), 2 * (k as i32 % 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_scenario() {
        let s = Scenario::parse(
            r#"{"format":1,"name":"x","physW":1920,"physH":1080,"durationMs":1000,"events":[
            {"t":0,"ui":true,"frames":[{"id":1,"shown":true,"clip":[28,686,563,253],"fontPx":20,"ppu":1.40625}]},
            {"t":5,"strip":{"shown":true,"rgb":"00000068686892"}},{"t":6,"strip":{"shown":false}},
            {"t":7,"sent":{"kind":"translate","req":1,"slot":0,"rows":2}}]}"#,
        )
        .unwrap();
        assert_eq!(s.events.len(), 4);
        assert_eq!(s.fade.visible, 120.0);
        assert_eq!(strip_blocks(s.events[1].strip.as_ref().unwrap().rgb.as_str()), vec![[0, 0, 0], [0x68, 0x68, 0x68]]);
        assert_eq!(block_pos(0), (0, 0));
        assert_eq!(block_pos(1), (0, 2));
        assert_eq!(block_pos(5), (4, 2));
    }

    #[test]
    fn rejects_unordered_events() {
        let r = Scenario::parse(r#"{"format":1,"name":"x","physW":1920,"physH":1080,"durationMs":1,"events":[{"t":5},{"t":1}]}"#);
        assert!(r.is_err());
    }
}
