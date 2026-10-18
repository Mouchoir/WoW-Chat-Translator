//! TranslateProbe end-to-end rig (package P5).
//!
//! `fakewow` replays a scenario recorded by `tests\e2e\make_scenario.js` (the real addon running on
//! `tests\mock.lua`): it paints the addon's strip blocks and lays the chat history out bottom-up the
//! way the WoW chat frame does, with the reserved rows' inline textures (tags and plates). It writes
//! the ground truth (where every plate is, what covers it, when each message was shown) as JSON
//! Lines for `tests\e2e\check.py`, which compares it with the Helper's `--diag-json` output.
//!
//! Everything here is pure and testable; the window, GDI text and capture live in `bin\fakewow.rs`.

pub mod display;
pub mod layout;
pub mod markup;
pub mod raster;
pub mod scenario;
pub mod tagcode;
pub mod truth;
