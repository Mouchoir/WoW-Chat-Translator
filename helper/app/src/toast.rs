//! The bridge toast ("TranslateProbe Helper: bridge OK (...)"), 4 s at client (16, 24): a small
//! layered window of class `TPHelperToast`, click-through and never activated. Under
//! `--no-overlay` it is only reported, never shown.

use std::time::{Duration, Instant};

use tp_bridge::image::Rect;

use crate::overlay::fonts::{Face, Fonts};
use crate::overlay::render::Canvas;
use crate::overlay::window::{LayeredWindow, TOAST_CLASS};

pub const TOAST_SECS: u64 = 4;
const PX: i32 = 15;
const PAD: i32 = 10;

pub struct Toaster {
    no_overlay: bool,
    exclude: bool,
    win: Option<LayeredWindow>,
    fonts: Fonts,
    until: Option<Instant>,
}

impl Toaster {
    pub fn new(no_overlay: bool) -> Toaster {
        Toaster { no_overlay, exclude: false, win: None, fonts: Fonts::new(), until: None }
    }

    pub fn set_exclude(&mut self, on: bool) {
        self.exclude = on;
        if let Some(w) = self.win.as_mut() {
            w.set_exclude(on);
        }
    }

    /// Renders the toast panel: dark rounded box at 85%, light text.
    pub fn render(&mut self, text: &str, max_w: i32) -> Canvas {
        let tw = self.fonts.measure(Face::Ui, PX, text);
        let w = (tw + 2 * PAD).clamp(40, max_w.max(40));
        let h = PX * 2 + PAD;
        let mut c = Canvas::new(Rect { x: 0, y: 0, w, h });
        let r = 6.0f32;
        for y in 0..h {
            for x in 0..w {
                let cx = (x as f32 + 0.5).clamp(r, w as f32 - r);
                let cy = (y as f32 + 0.5).clamp(r, h as f32 - r);
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let shape = (r - d + 0.5).clamp(0.0, 1.0);
                c.blend(x, y, [16, 20, 24], 0.85 * shape);
            }
        }
        let cov = self.fonts.coverage(Face::Ui, PX, text, w - 2 * PAD, h);
        for y in 0..h {
            for x in 0..w - 2 * PAD {
                let a = cov.at(x, y);
                if a != 0 {
                    c.blend(x + PAD, y, [235, 245, 250], f32::from(a) / 255.0);
                }
            }
        }
        c
    }

    /// Shows `text` with its top-left at screen `(x, y)` for 4 s. Returns true when a window was
    /// shown (never under `--no-overlay`).
    pub fn show(&mut self, text: &str, x: i32, y: i32, max_w: i32) -> bool {
        if self.no_overlay {
            return false;
        }
        let canvas = self.render(text, max_w);
        if self.win.is_none() {
            self.win = LayeredWindow::create(TOAST_CLASS);
            let ex = self.exclude;
            if let Some(w) = self.win.as_mut() {
                w.set_exclude(ex);
            }
        }
        let shown = self.win.as_mut().is_some_and(|w| w.present(x, y, &canvas));
        if shown {
            self.until = Some(Instant::now() + Duration::from_secs(TOAST_SECS));
        }
        shown
    }

    /// Hides the toast once its 4 s are over; returns true while it is still showing.
    pub fn tick(&mut self) -> bool {
        match self.until {
            Some(t) if Instant::now() >= t => {
                self.hide();
                false
            }
            Some(_) => true,
            None => false,
        }
    }

    pub fn hide(&mut self) {
        self.until = None;
        if let Some(w) = self.win.as_mut() {
            w.hide();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_panel_with_text_and_never_shows_under_no_overlay() {
        let mut t = Toaster::new(true);
        let c = t.render("TranslateProbe Helper: bridge OK (dense, margin 42, WGC 30 Hz)", 2000);
        assert!(c.rect.w > 200 && c.rect.h == 40);
        let (_, a) = c.pixel(c.rect.w / 2, 1).unwrap();
        assert!(a > 200, "panel at 85%: {a}");
        assert_eq!(c.pixel(0, 0).unwrap().1, 0, "rounded corner");
        assert!(!t.show("x", 0, 0, 100));
        assert!(t.win.is_none());
        assert!(!t.tick());
    }
}
