//! tp_bridge: the addon-to-helper pixel bridge, protocol version 2 (normative spec: docs/BRIDGE.md).
//!
//! - `strip`: decodes the 4 px strip of 2x2 blocks the addon draws at the client's top-left corner.
//! - `reasm`: rebuilds messages from strip chunks (dedup, expiry, loss counters).
//! - `msg`: strict parsers for TRANSLATE, CANCEL, CLEAR, TEST and VIEW.
//! - `marks`: the link marks inside a TRANSLATE text (spans, link colours), shared by the engine
//!   and the app.
//! - `tags`: finds the row tags (sync pair + 6 base-6 digits with CRC-5) inside chat frames.
//! - `cards`: groups tags into cards and measures the plate pixels the overlay may paint.
//! - `tone`: undoes a display curve (measured on the strip) before the tag scan and the cards.
//!
//! Chat text only ever travels inside [`msg::ChatText`], which has no Debug, Display or
//! Serialize, so it cannot reach a log by accident.

pub mod cards;
pub mod crc;
pub mod image;
pub mod marks;
pub mod msg;
pub mod reasm;
pub mod strip;
#[cfg(any(test, feature = "synth"))]
pub mod synth;
pub mod tags;
pub mod tone;
