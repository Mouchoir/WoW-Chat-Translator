//! Display curves (`tone`): cards of 3 rows with fractional, antialiased block edges over a chat
//! ground, captured through a gamma curve. With the curve fitted from the strip's reference levels
//! and undone before the scan, every row is found with its plate.

use tp_bridge::msg::View;
use tp_bridge::synth::{self, Canvas, Rng, TagDraw};
use tp_bridge::tone::{Tone, DRAWN};
use tp_bridge::{cards, image, tags};

fn curve(g: f32) -> impl Fn(u8) -> u8 {
    move |v| (255.0 * (f32::from(v) / 255.0).powf(1.0 / g)).round() as u8
}

fn rows_found(img: &image::Bgra, area: image::Rect, view: View) -> usize {
    let mut hits = Vec::new();
    tags::scan(img, area, None, &mut hits);
    let cs = cards::build(img, &hits, &[view], &|_| Some(true), Some(0));
    cs.iter().map(|c| c.bands.len()).sum()
}

#[test]
fn display_curves_are_undone_before_the_scan() {
    let mut rng = Rng::new(0x70E5);
    for g in [2.2f32, 1.6, 0.4545] {
        let (mut raw, mut lin, mut total) = (0, 0, 0);
        for case in 0..40 {
            let (w, h) = (520, 140);
            let mut cv = Canvas::world(&mut rng, w, h);
            cv.darken(0.7);
            cv.noise(&mut rng, 3);
            cv.aa = true;
            let bw = rng.uniform(4.6, 8.0);
            let x = 20.0 + rng.uniform(-1.0, 1.0);
            let mut y = 10.0 + rng.uniform(0.0, 1.0);
            let slot = rng.int(0, 126) as u8;
            for row in 0..3 {
                synth::draw_tag(&mut cv, &TagDraw {
                    slot,
                    row,
                    sess_bit: 0,
                    x,
                    y,
                    block_w: bw,
                    block_h: 18.0,
                    gap: rng.uniform(0.0, 1.5),
                    alpha: 1.0,
                    plate_w: 300.0,
                    colors: None,
                });
                y += 20.0;
            }
            let mut cap = cv.capture();
            let f = curve(g);
            for (i, b) in cap.buf.iter_mut().enumerate() {
                if i % 4 != 3 {
                    *b = f(*b);
                }
            }
            let levels: [[f32; 3]; 4] = std::array::from_fn(|k| [f32::from(f(DRAWN[k] as u8)); 3]);
            let tone = Tone::from_levels(&levels);
            assert!(!tone.is_identity(), "gamma {g}");
            let img = cap.bgra();
            let area = image::Rect { x: 0, y: 0, w, h };
            let view = View { id: 1, x: 0, y: 0, w: w as u16, h: h as u16, font_px: 18, row_px: 20 };
            raw += rows_found(&img, area, view);
            let mut buf = Vec::new();
            let n = rows_found(&tone.apply(&img, &mut buf), area, view);
            assert_eq!(n, 3, "gamma {g} case {case}: rows with a plate after the curve is undone");
            lin += n;
            total += 3;
        }
        println!("gamma {g}: rows shown {raw}/{total} on the raw capture, {lin}/{total} with the curve undone");
    }
}
