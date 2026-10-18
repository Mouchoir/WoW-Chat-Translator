//! GDI fallback: `BitBlt` of client regions from the screen DC. It reads whatever is on screen,
//! so the overlay windows are made invisible to it with `WDA_EXCLUDEFROMCAPTURE`.
//!
//! Each region index keeps two buffers used in turn (no allocation per tick), so the previous
//! capture of a region stays readable ([`Grabber::previous`]).

use std::time::Instant;

use tp_bridge::image::{Bgra, Rect};
use windows::Win32::Graphics::Gdi::*;

use super::{clip, GrabStatus, GrabTiming, Grabber, Image, Method};
use crate::wow::WowInfo;

pub struct GdiGrabber {
    screen: HDC,
    mem: HDC,
    dib: Option<(HBITMAP, HGDIOBJ, *mut u8, i32, i32)>,
    timing: GrabTiming,
    /// Two buffers per region index, used in turn.
    slots: Vec<Slot>,
    frames: u64,
    readable: bool,
}

/// The two buffers of one region index.
#[derive(Default)]
struct Slot {
    img: [Image; 2],
    /// Grab number of each buffer's content; None while unknown.
    held: [Option<u64>; 2],
    /// The buffer with the newest capture.
    cur: usize,
}

impl GdiGrabber {
    pub fn new() -> GdiGrabber {
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            GdiGrabber { screen, mem, dib: None, timing: GrabTiming::default(), slots: Vec::new(), frames: 0, readable: false }
        }
    }

    /// Makes sure the DIB is at least `w` x `h`; returns its bits and width. It only grows (to the
    /// largest width and height asked so far), so the strip and the scan area of one tick share it
    /// without a new DIB per region.
    fn ensure(&mut self, w: i32, h: i32) -> Option<(*mut u8, i32)> {
        let mut size = (w.max(64), h.max(16));
        if let Some((_, _, bits, dw, dh)) = self.dib {
            if dw >= w && dh >= h {
                return Some((bits, dw));
            }
            size = (size.0.max(dw), size.1.max(dh));
        }
        self.free_dib();
        let (nw, nh) = size;
        unsafe {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: nw,
                    biHeight: -nh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(self.screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            if bits.is_null() {
                let _ = DeleteObject(bmp.into());
                return None;
            }
            let old = SelectObject(self.mem, bmp.into());
            self.dib = Some((bmp, old, bits as *mut u8, nw, nh));
            Some((bits as *mut u8, nw))
        }
    }

    fn free_dib(&mut self) {
        if let Some((bmp, old, _, _, _)) = self.dib.take() {
            unsafe {
                SelectObject(self.mem, old);
                let _ = DeleteObject(bmp.into());
            }
        }
    }
}

impl Default for GdiGrabber {
    fn default() -> Self {
        GdiGrabber::new()
    }
}

impl Drop for GdiGrabber {
    fn drop(&mut self) {
        self.free_dib();
        unsafe {
            let _ = DeleteDC(self.mem);
            ReleaseDC(None, self.screen);
        }
    }
}

impl Grabber for GdiGrabber {
    fn method(&self) -> Method {
        Method::Gdi
    }

    fn timing(&self) -> GrabTiming {
        self.timing
    }

    fn grab(&mut self, wow: &WowInfo, regions: &[Rect]) -> GrabStatus {
        let t0 = Instant::now();
        let r = self.grab_inner(wow, regions);
        self.readable = r == GrabStatus::Fresh;
        self.timing = GrabTiming { copy: t0.elapsed(), ..GrabTiming::default() };
        r
    }

    fn image(&self, i: usize) -> Option<Bgra<'_>> {
        let slot = self.slots.get(i).filter(|_| self.readable)?;
        let img = &slot.img[slot.cur];
        (!img.is_empty()).then(|| img.view())
    }

    fn previous(&self, i: usize) -> Option<(Bgra<'_>, u64)> {
        let slot = self.slots.get(i).filter(|_| self.readable)?;
        let (now, prev) = (&slot.img[slot.cur], &slot.img[1 - slot.cur]);
        let n = slot.held[1 - slot.cur]?;
        (!prev.is_empty() && prev.rect_key() == now.rect_key()).then(|| (prev.view(), n))
    }

    fn frame_number(&self) -> u64 {
        self.frames
    }

    fn release(&mut self) {
        self.readable = false;
    }
}

impl GdiGrabber {
    fn grab_inner(&mut self, wow: &WowInfo, regions: &[Rect]) -> GrabStatus {
        let mut slots = std::mem::take(&mut self.slots);
        if slots.len() < regions.len() {
            slots.resize_with(regions.len(), Slot::default);
        }
        let number = self.frames + 1;
        // Each region goes to the buffer that does not hold its newest capture.
        let mut targets: Vec<usize> = Vec::with_capacity(regions.len());
        let mut out: Vec<Image> = Vec::with_capacity(regions.len());
        for slot in slots.iter_mut().take(regions.len()) {
            let wb = if slot.held[slot.cur].is_some() { 1 - slot.cur } else { slot.cur };
            slot.held[wb] = None;
            targets.push(wb);
            out.push(std::mem::take(&mut slot.img[wb]));
        }
        let r = self.read_regions(wow, regions, &mut out);
        for ((slot, wb), img) in slots.iter_mut().zip(targets).zip(out) {
            slot.img[wb] = img;
            if r == GrabStatus::Fresh {
                slot.cur = wb;
                slot.held[wb] = Some(number);
            }
        }
        if r == GrabStatus::Fresh {
            self.frames = number;
        }
        self.slots = slots;
        r
    }

    fn read_regions(&mut self, wow: &WowInfo, regions: &[Rect], out: &mut [Image]) -> GrabStatus {
        for (r, img) in regions.iter().zip(out.iter_mut()) {
            let Some(c) = clip(*r, wow.client_w, wow.client_h) else {
                img.set_empty(r.x, r.y);
                continue;
            };
            let Some((bits, dw)) = self.ensure(c.w, c.h) else { return GrabStatus::Failed };
            unsafe {
                if BitBlt(self.mem, 0, 0, c.w, c.h, Some(self.screen), wow.origin_x + c.x, wow.origin_y + c.y, SRCCOPY).is_err() {
                    return GrabStatus::Failed;
                }
                let _ = GdiFlush();
                let row = (c.w * 4) as usize;
                let stride = (dw * 4) as usize;
                img.px.clear();
                img.px.reserve(row * c.h as usize);
                for y in 0..c.h as usize {
                    img.px.extend_from_slice(std::slice::from_raw_parts(bits.add(y * stride), row));
                }
            }
            img.width = c.w as u32;
            img.height = c.h as u32;
            img.origin_x = c.x;
            img.origin_y = c.y;
        }
        GrabStatus::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_info() -> WowInfo {
        // The primary monitor's top-left corner, read with BitBlt (nothing is drawn or shown).
        WowInfo { hwnd: 0, origin_x: 0, origin_y: 0, client_w: 64, client_h: 32, minimized: false, foreground: false, frame_dx: 0, frame_dy: 0, dpi: 96 }
    }

    #[test]
    fn two_buffers_keep_the_previous_capture() {
        let mut g = GdiGrabber::new();
        let info = screen_info();
        let a = Rect { x: 0, y: 0, w: 16, h: 8 };
        let b = Rect { x: 0, y: 8, w: 32, h: 4 };
        assert_eq!(g.grab(&info, &[a, b]), GrabStatus::Fresh);
        assert_eq!(g.frame_number(), 1);
        assert!(g.previous(0).is_none(), "nothing before the first capture");
        let img = g.image(0).unwrap();
        assert_eq!((img.width, img.height, img.stride, img.origin_x, img.origin_y), (16, 8, 64, 0, 0));
        assert_eq!(g.grab(&info, &[a, b]), GrabStatus::Fresh);
        assert_eq!(g.frame_number(), 2);
        let (prev, n) = g.previous(1).expect("previous capture of the same rectangle");
        assert_eq!(n, 1);
        assert_eq!((prev.width, prev.height, prev.origin_y), (32, 4, 8));
        // A region whose rectangle changed has no comparable previous capture.
        let c = Rect { x: 0, y: 0, w: 15, h: 8 };
        assert_eq!(g.grab(&info, &[c, b]), GrabStatus::Fresh);
        assert!(g.previous(0).is_none());
        assert_eq!(g.previous(1).map(|(_, n)| n), Some(2));
        // Nothing is readable after release.
        g.release();
        assert!(g.image(0).is_none() && g.previous(1).is_none());
        // A region clipped away is empty.
        assert_eq!(g.grab(&info, &[Rect { x: 100, y: 0, w: 4, h: 4 }]), GrabStatus::Fresh);
        assert!(g.image(0).is_none());
    }
}
