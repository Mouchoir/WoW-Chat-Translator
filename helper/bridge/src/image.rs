//! Captured pixels, addressed in client coordinates (physical px from the WoW client's top-left).

/// A rectangle in client coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A borrowed BGRA capture of part of the client area.
/// `px[row * stride + col * 4 ..]` is B, G, R, A of the pixel at client
/// `(origin_x + col, origin_y + row)`. `stride` is in bytes.
pub struct Bgra<'a> {
    pub px: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub origin_x: i32,
    pub origin_y: i32,
}

impl<'a> Bgra<'a> {
    /// `[R, G, B]` of the pixel at client `(x, y)`, or None outside the capture.
    pub fn rgb(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        let lx = i64::from(x) - i64::from(self.origin_x);
        let ly = i64::from(y) - i64::from(self.origin_y);
        if lx < 0 || ly < 0 || lx >= i64::from(self.width) || ly >= i64::from(self.height) {
            return None;
        }
        let o = ly as usize * self.stride as usize + lx as usize * 4;
        let p = self.px.get(o..o + 3)?;
        Some([p[2], p[1], p[0]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_uses_client_coordinates() {
        // 2x2 capture at client (10, 20), stride with 4 bytes of padding per row
        let px = [
            1, 2, 3, 255, 4, 5, 6, 255, 0, 0, 0, 0, //
            7, 8, 9, 255, 10, 11, 12, 255, 0, 0, 0, 0,
        ];
        let img = Bgra { px: &px, width: 2, height: 2, stride: 12, origin_x: 10, origin_y: 20 };
        assert_eq!(img.rgb(10, 20), Some([3, 2, 1]));
        assert_eq!(img.rgb(11, 21), Some([12, 11, 10]));
        assert_eq!(img.rgb(9, 20), None);
        assert_eq!(img.rgb(12, 20), None);
        assert_eq!(img.rgb(10, 22), None);
    }
}
