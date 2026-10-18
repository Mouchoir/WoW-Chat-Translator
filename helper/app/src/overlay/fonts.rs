//! GDI fonts, text measurement and text coverage (antialiased greyscale, not ClearType).
//!
//! Text is rasterized at 3x and box-filtered down: GDI's `ANTIALIASED_QUALITY` follows each font's
//! gasp table, and the Latin face drawn directly at 16 px came out with no intermediate coverage
//! value at all (measured by `measures_and_rasterizes_text`), in the middle of the chat font range.
//! Measurement uses the same 3x metrics so that wrapping matches the drawn text.
//!
//! Faces (design D.3): Latin and Cyrillic targets use "Arial Narrow" when installed, else "Arial"
//! narrowed to 0.82 x its normal average character width; zh "Microsoft YaHei UI", zh-TW
//! "Microsoft JhengHei UI", ko "Malgun Gothic". GDI font linking covers other scripts.

use std::collections::HashMap;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;

use super::render::Coverage;

/// Supersampling factor of the text rasterizer.
pub const SS: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Latin,
    ZhHans,
    ZhHant,
    Ko,
    /// Toasts.
    Ui,
}

pub fn face_for_target(target: &str) -> Face {
    match target {
        "zh" => Face::ZhHans,
        "zh-TW" => Face::ZhHant,
        "ko" => Face::Ko,
        _ => Face::Latin,
    }
}

pub struct Fonts {
    dc: HDC,
    fonts: HashMap<(Face, i32), HFONT>,
    narrow_installed: Option<bool>,
    cell_ratios: HashMap<Face, f32>,
}

impl Default for Fonts {
    fn default() -> Self {
        Fonts::new()
    }
}

impl Fonts {
    pub fn new() -> Fonts {
        let dc = unsafe { CreateCompatibleDC(None) };
        Fonts { dc, fonts: HashMap::new(), narrow_installed: None, cell_ratios: HashMap::new() }
    }

    fn create(face: PCWSTR, px: i32, width: i32, weight: i32) -> HFONT {
        unsafe {
            CreateFontW(
                -px,
                width,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
                face,
            )
        }
    }

    /// True when "Arial Narrow" exists (GDI substitutes another face otherwise).
    pub fn narrow_installed(&mut self) -> bool {
        if let Some(v) = self.narrow_installed {
            return v;
        }
        let f = Self::create(w!("Arial Narrow"), 20, 0, FW_NORMAL.0 as i32);
        let mut name = [0u16; 64];
        unsafe {
            let old = SelectObject(self.dc, f.into());
            GetTextFaceW(self.dc, Some(&mut name));
            SelectObject(self.dc, old);
            let _ = DeleteObject(f.into());
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        let got = String::from_utf16_lossy(&name[..end]);
        let v = got.eq_ignore_ascii_case("Arial Narrow");
        self.narrow_installed = Some(v);
        v
    }

    /// The font for `face` at `px` pixels (character height).
    pub fn font(&mut self, face: Face, px: i32) -> HFONT {
        let px = px.clamp(6, 300);
        if let Some(f) = self.fonts.get(&(face, px)) {
            return *f;
        }
        let f = match face {
            Face::ZhHans => Self::create(w!("Microsoft YaHei UI"), px, 0, FW_NORMAL.0 as i32),
            Face::ZhHant => Self::create(w!("Microsoft JhengHei UI"), px, 0, FW_NORMAL.0 as i32),
            Face::Ko => Self::create(w!("Malgun Gothic"), px, 0, FW_NORMAL.0 as i32),
            Face::Ui => Self::create(w!("Segoe UI"), px, 0, FW_SEMIBOLD.0 as i32),
            Face::Latin => {
                if self.narrow_installed() {
                    Self::create(w!("Arial Narrow"), px, 0, FW_NORMAL.0 as i32)
                } else {
                    let probe = Self::create(w!("Arial"), px, 0, FW_NORMAL.0 as i32);
                    let mut tm = TEXTMETRICW::default();
                    unsafe {
                        let old = SelectObject(self.dc, probe.into());
                        let _ = GetTextMetricsW(self.dc, &mut tm);
                        SelectObject(self.dc, old);
                        let _ = DeleteObject(probe.into());
                    }
                    let narrow = ((tm.tmAveCharWidth as f32) * 0.82).round().max(1.0) as i32;
                    Self::create(w!("Arial"), px, narrow, FW_NORMAL.0 as i32)
                }
            }
        };
        if self.fonts.len() > 48 {
            self.clear();
        }
        self.fonts.insert((face, px), f);
        f
    }

    /// Cell height (ascent + descent, `tmHeight`) over the character height of `face`: about
    /// 1.15 for Arial Narrow. Text is drawn vertically centred in a box one band tall, so a font
    /// whose character height equals the band loses the tops of accented capitals and the bottoms
    /// of descenders; the overlay caps the cell instead (`overlay::FONT_CELL_SLACK`).
    pub fn cell_ratio(&mut self, face: Face) -> f32 {
        if let Some(r) = self.cell_ratios.get(&face) {
            return *r;
        }
        const PROBE: i32 = 240;
        let f = self.font(face, PROBE);
        let mut tm = TEXTMETRICW::default();
        let ok = unsafe {
            let old = SelectObject(self.dc, f.into());
            let ok = GetTextMetricsW(self.dc, &mut tm).as_bool();
            SelectObject(self.dc, old);
            ok
        };
        let r = if ok && tm.tmHeight > 0 { (tm.tmHeight as f32 / PROBE as f32).clamp(1.0, 1.6) } else { 1.15 };
        self.cell_ratios.insert(face, r);
        r
    }

    fn clear(&mut self) {
        for (_, f) in self.fonts.drain() {
            unsafe {
                let _ = DeleteObject(f.into());
            }
        }
    }

    /// Width in px of `text` in `face` at `px`.
    pub fn measure(&mut self, face: Face, px: i32, text: &str) -> i32 {
        self.measure_ss(face, px * SS, text)
    }

    /// Width in px of `text` in `face` at `size` supersampled pixels (`SS` per pixel, so sizes
    /// move in thirds of a pixel).
    pub fn measure_ss(&mut self, face: Face, size: i32, text: &str) -> i32 {
        if text.is_empty() {
            return 0;
        }
        (self.measure_raw(face, size, text) + SS - 1) / SS
    }

    fn measure_raw(&mut self, face: Face, px: i32, text: &str) -> i32 {
        let f = self.font(face, px);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut sz = SIZE::default();
        unsafe {
            let old = SelectObject(self.dc, f.into());
            let _ = GetTextExtentPoint32W(self.dc, &wide, &mut sz);
            SelectObject(self.dc, old);
        }
        sz.cx
    }

    /// Greyscale coverage of `text` drawn left-aligned and vertically centred in `w` x `h`.
    pub fn coverage(&mut self, face: Face, px: i32, text: &str, w: i32, h: i32) -> Coverage {
        self.coverage_ss(face, px * SS, text, w, h)
    }

    /// [`Fonts::coverage`] at `size` supersampled pixels (`SS` per pixel).
    pub fn coverage_ss(&mut self, face: Face, size: i32, text: &str, w: i32, h: i32) -> Coverage {
        let (w, h) = (w.max(1), h.max(1));
        let mut cov = Coverage { w, h, a: vec![0; (w * h) as usize] };
        if text.is_empty() {
            return cov;
        }
        let (bw, bh) = (w * SS, h * SS);
        let f = self.font(face, size);
        unsafe {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: bw,
                    biHeight: -bh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(self.dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else { return cov };
            if bits.is_null() {
                let _ = DeleteObject(bmp.into());
                return cov;
            }
            std::ptr::write_bytes(bits as *mut u8, 0, (bw * bh * 4) as usize);
            let old_bmp = SelectObject(self.dc, bmp.into());
            let old_font = SelectObject(self.dc, f.into());
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
            let mut rc = RECT { left: 0, top: 0, right: bw, bottom: bh };
            let mut wide: Vec<u16> = text.encode_utf16().collect();
            DrawTextW(self.dc, &mut wide, &mut rc, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            let _ = GdiFlush();
            let src = std::slice::from_raw_parts(bits as *const u8, (bw * bh * 4) as usize);
            let n = (SS * SS) as u32;
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0u32;
                    for sy in 0..SS {
                        let row = ((y * SS + sy) * bw + x * SS) as usize * 4;
                        for sx in 0..SS as usize {
                            let p = &src[row + sx * 4..row + sx * 4 + 3];
                            sum += u32::from(p[0].max(p[1]).max(p[2]));
                        }
                    }
                    cov.a[(y * w + x) as usize] = ((sum + n / 2) / n) as u8;
                }
            }
            SelectObject(self.dc, old_font);
            SelectObject(self.dc, old_bmp);
            let _ = DeleteObject(bmp.into());
        }
        cov
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        self.clear();
        unsafe {
            let _ = DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faces_by_target() {
        assert_eq!(face_for_target("zh"), Face::ZhHans);
        assert_eq!(face_for_target("zh-TW"), Face::ZhHant);
        assert_eq!(face_for_target("ko"), Face::Ko);
        assert_eq!(face_for_target("ru"), Face::Latin);
        assert_eq!(face_for_target("fr"), Face::Latin);
    }

    #[test]
    fn capped_cell_keeps_accents_and_descenders() {
        // At the overlay's cap (cell = band + FONT_CELL_SLACK), a French line with accented
        // capitals and descenders loses at most 1.5% of its ink to the band, against 2.1-2.6%
        // with the character height at the band height (review of 2026-10-18).
        let mut f = Fonts::new();
        let ratio = f.cell_ratio(Face::Latin);
        assert!((1.0..1.6).contains(&ratio), "{ratio}");
        let text = "\u{c9}gypte: quelqu'un pour la qu\u{ea}te jpqy \u{c0}\u{c9}\u{c8}";
        for band_h in [12, 13, 14, 16, 18, 22] {
            let size = (((band_h as f32 + crate::overlay::FONT_CELL_SLACK) / ratio).min(band_h as f32) * SS as f32).round() as i32;
            let clipped = f.coverage_ss(Face::Latin, size, text, 600, band_h);
            let full = f.coverage_ss(Face::Latin, size, text, 600, band_h * 3);
            let ink = |c: &Coverage| c.a.iter().map(|v| u64::from(*v)).sum::<u64>() as f64;
            let lost = 1.0 - ink(&clipped) / ink(&full);
            println!("band {band_h}: size {:.2} px, {:.2}% of the ink lost", size as f32 / SS as f32, lost * 100.0);
            assert!(lost <= 0.015, "band {band_h}: {:.1}% of the ink lost", lost * 100.0);
        }
    }

    #[test]
    fn measures_and_rasterizes_text() {
        let mut f = Fonts::new();
        let one = f.measure(Face::Latin, 16, "Hello");
        let two = f.measure(Face::Latin, 16, "Hello Hello");
        assert!(one > 10 && two > one * 3 / 2, "{one} {two}");
        assert!(f.measure(Face::Latin, 32, "Hello") > one);
        assert_eq!(f.measure(Face::Latin, 16, ""), 0);
        let cov = f.coverage(Face::Latin, 16, "Hello", 80, 20);
        let lit = cov.a.iter().filter(|a| **a > 128).count();
        assert!(lit > 20, "glyph pixels: {lit}");
        // Antialiased: some partial coverage exists.
        assert!(cov.a.iter().any(|a| *a > 0 && *a < 255));
        // Left-aligned: nothing in the right half.
        assert!((0..20).all(|y| (60..80).all(|x| cov.at(x, y) == 0)));
        let cjk = f.coverage(Face::ZhHans, 16, "你好", 40, 20);
        assert!(cjk.a.iter().filter(|a| **a > 128).count() > 10);
        assert!(f.coverage(Face::Latin, 16, "", 10, 10).a.iter().all(|a| *a == 0));
    }
}
