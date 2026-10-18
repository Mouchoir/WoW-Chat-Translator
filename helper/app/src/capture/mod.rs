//! Capture of client regions of the game window.
//!
//! - [`wgc::WgcGrabber`]: Windows.Graphics.Capture on the game window (primary). It sees the game
//!   even under other windows and never sees our overlay.
//! - [`gdi::GdiGrabber`]: `BitBlt` from the screen DC (fallback). Our windows are excluded from it
//!   with `WDA_EXCLUDEFROMCAPTURE`.
//!
//! Regions are in client coordinates (physical px from the client's top-left corner).

pub mod gdi;
pub mod wgc;

use std::time::Duration;

use tp_bridge::image::{Bgra, Rect};

use crate::wow::WowInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Wgc,
    Gdi,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::Wgc => "wgc",
            Method::Gdi => "gdi",
        }
    }

    /// For the bridge toast ("WGC 30 Hz").
    pub fn label(self) -> &'static str {
        match self {
            Method::Wgc => "WGC",
            Method::Gdi => "GDI",
        }
    }
}

/// An owned BGRA copy of one client region (tightly packed rows).
#[derive(Clone, Debug, Default)]
pub struct Image {
    pub px: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub origin_x: i32,
    pub origin_y: i32,
}

impl Image {
    pub fn view(&self) -> Bgra<'_> {
        Bgra { px: &self.px, width: self.width, height: self.height, stride: self.width * 4, origin_x: self.origin_x, origin_y: self.origin_y }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// `[R, G, B]` at client `(x, y)`.
    pub fn rgb(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        self.view().rgb(x, y)
    }

    pub fn rect(&self) -> Rect {
        Rect { x: self.origin_x, y: self.origin_y, w: self.width as i32, h: self.height as i32 }
    }

    /// Origin and size, for "same rectangle" checks.
    pub fn rect_key(&self) -> [i32; 4] {
        [self.origin_x, self.origin_y, self.width as i32, self.height as i32]
    }

    /// Makes the image empty at `(x, y)`, keeping its buffer for reuse.
    pub fn set_empty(&mut self, x: i32, y: i32) {
        self.px.clear();
        self.width = 0;
        self.height = 0;
        self.origin_x = x;
        self.origin_y = y;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrabStatus {
    /// A new frame was read; its regions are readable through [`Grabber::image`].
    Fresh,
    /// The source has no frame newer than the last one (WGC only); nothing is readable.
    NoNewFrame,
    Failed,
}

/// Time spent in the last `grab`: taking the newest frame, issuing the copies, waiting for the
/// GPU, and mapping (WGC) or reading (GDI) the pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GrabTiming {
    pub acquire: Duration,
    pub copy: Duration,
    pub wait: Duration,
    pub read: Duration,
}

pub trait Grabber {
    fn method(&self) -> Method;
    /// Reads each region (clipped to the client area) of the newest frame. After `Fresh`, region
    /// `i` is readable through [`Grabber::image`] until the next `grab`, `skip` or `release`.
    fn grab(&mut self, wow: &WowInfo, regions: &[Rect]) -> GrabStatus;
    /// Region `i` of the last fresh grab, in client coordinates; None when nothing was left of it
    /// after clipping, or when nothing is readable.
    fn image(&self, i: usize) -> Option<Bgra<'_>>;
    /// The capture of region `i` before the last one, when it had the same rectangle, with the
    /// number of the grab it came from (readable until the next `grab`, `skip` or `release`).
    fn previous(&self, _i: usize) -> Option<(Bgra<'_>, u64)> {
        None
    }
    /// Number of the last fresh grab (1 for the first).
    fn frame_number(&self) -> u64 {
        0
    }
    /// Ends the reading of the last grab (WGC unmaps its staging textures).
    fn release(&mut self) {}
    /// A tick with nothing to grab: drop pending frames so that the next grab is not stale.
    fn skip(&mut self) {}
    /// Caps how often the source produces frames (WGC); sources read on demand (GDI) ignore it.
    fn set_min_interval(&mut self, _ms: u32) {}
    /// Ends the capture and hands back its D3D11 device for a later session (WGC only).
    fn into_wgc_device(self: Box<Self>) -> Option<wgc::WgcDevice> {
        None
    }
    /// Timing of the last `grab` (zero when the grabber does not measure it).
    fn timing(&self) -> GrabTiming {
        GrabTiming::default()
    }
}

/// `r` clipped to `0..w` x `0..h`; None when nothing is left.
pub fn clip(r: Rect, w: i32, h: i32) -> Option<Rect> {
    let x0 = r.x.max(0);
    let y0 = r.y.max(0);
    let x1 = (r.x + r.w).min(w);
    let y1 = (r.y + r.h).min(h);
    (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
}

/// Smallest rectangle holding both.
pub fn union(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_and_union() {
        let r = clip(Rect { x: -5, y: 10, w: 20, h: 100 }, 12, 50).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (0, 10, 12, 40));
        assert!(clip(Rect { x: 20, y: 0, w: 5, h: 5 }, 10, 10).is_none());
        let u = union(Rect { x: 0, y: 0, w: 2, h: 2 }, Rect { x: 5, y: 1, w: 1, h: 4 });
        assert_eq!((u.x, u.y, u.w, u.h), (0, 0, 6, 5));
    }

    #[test]
    fn image_view_uses_client_coordinates() {
        let img = Image { px: vec![1, 2, 3, 255, 4, 5, 6, 255], width: 2, height: 1, origin_x: 10, origin_y: 7 };
        assert_eq!(img.rgb(11, 7), Some([6, 5, 4]));
        assert_eq!(img.rgb(9, 7), None);
    }
}
