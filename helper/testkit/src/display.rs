//! The colour path between the game and the capture: WoW's Gamma and Contrast settings.
//!
//! `out = 255 * clamp((in/255 - 0.5) * contrast + 0.5) ^ (1 / gamma)`, so gamma 2.2 brightens like
//! a higher WoW Gamma setting and gamma below 1 darkens (the bridge simulation's exponent is
//! `1 / gamma` here). Identity at gamma 1, contrast 1.

#[derive(Clone)]
pub struct Lut {
    pub table: [u8; 256],
    pub identity: bool,
}

impl Lut {
    pub fn new(gamma: f64, contrast: f64) -> Lut {
        let g = if gamma > 0.0 { gamma } else { 1.0 };
        let mut table = [0u8; 256];
        for (i, t) in table.iter_mut().enumerate() {
            let x = ((i as f64 / 255.0 - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
            *t = (255.0 * x.powf(1.0 / g)).round().clamp(0.0, 255.0) as u8;
        }
        let identity = table.iter().enumerate().all(|(i, v)| *v as usize == i);
        Lut { table, identity }
    }

    pub fn map(&self, rgb: [u8; 3]) -> [u8; 3] {
        [self.table[rgb[0] as usize], self.table[rgb[1] as usize], self.table[rgb[2] as usize]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_monotonic() {
        assert!(Lut::new(1.0, 1.0).identity);
        let l = Lut::new(2.2, 1.0);
        assert!(!l.identity);
        assert_eq!(l.table[0], 0);
        assert_eq!(l.table[255], 255);
        assert!(l.table[104] > 104, "gamma 2.2 brightens");
        assert!(l.table.windows(2).all(|w| w[0] <= w[1]));
        let d = Lut::new(1.0 / 2.2, 1.0);
        assert!(d.table[104] < 104, "gamma below 1 darkens");
    }
}
