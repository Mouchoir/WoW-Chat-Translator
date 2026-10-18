//! Cross-implementation vectors written by tools/bridge/make_vectors.py (tests/vectors):
//! - frames/*.json: every frame the encoder (Python, and byte-identical Lua) composed is drawn as
//!   pixels and decoded, and the reassembler rebuilds exactly the reference's message list;
//! - tags.json: every tag's colours, and every tag drawn and scanned back;
//! - messages.json: the builders' bytes parse (or are refused) exactly as the reference does;
//! - strips.json: distorted captures give the reference decoder's verdict and bytes.
//! Plus message bytes built by addon/TranslateProbe/Bridge.lua (run under fengari).

use std::path::PathBuf;

use serde_json::Value;
use tp_bridge::msg::{parse_message, Message, ParseError, View};
use tp_bridge::reasm::Reassembler;
use tp_bridge::strip::{self, decode_strip, Mode, StripRead};
use tp_bridge::synth::{self, Canvas, Rng};
use tp_bridge::{image, tags};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn vectors() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("tests").join("vectors")
}

fn load(p: PathBuf) -> Value {
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// A canonical description of a message (test-only; synthetic content).
fn describe(m: &Message) -> String {
    match m {
        Message::Translate(t) => {
            let pairs: Vec<String> =
                t.pairs.iter().map(|(a, b)| format!("{}={}", to_hex(a.as_str().as_bytes()), to_hex(b.as_str().as_bytes()))).collect();
            format!(
                "translate {} {} {} {} {} {} {} [{}]",
                t.req_id,
                t.slot,
                t.rows,
                t.no_cache,
                t.plate,
                t.target,
                to_hex(t.text.as_str().as_bytes()),
                pairs.join(",")
            )
        }
        Message::Cancel { req_id, slot } => format!("cancel {req_id} {slot}"),
        Message::Clear => "clear".into(),
        Message::Test { nonce } => format!("test {nonce}"),
        Message::View(v) => format!("view {v:?}"),
    }
}

fn frame_fields(f: &strip::Frame) -> String {
    let chunks: Vec<String> = f
        .chunks
        .iter()
        .map(|c| format!("{}:{}/{}:{}:{:08x}:{}", c.msg_id, c.idx, c.count, c.total, c.msg_crc, to_hex(&c.data)))
        .collect();
    format!("{:?} {} {} {}x{} b{} [{}]", f.mode, f.seq, f.session, f.phys_w, f.phys_h, f.addon_build, chunks.join(" "))
}

const BLOCK_CHARS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz-_";

#[test]
fn frame_traces_decode_and_reassemble() {
    let dir = vectors().join("frames");
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    assert_eq!(names.len(), 30, "10 traces x 3 modes");
    let mut rng = Rng::new(9);
    let (mut nframes, mut nmsgs) = (0, 0);
    for p in names {
        let t = load(p.clone());
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let mut reasm = Reassembler::new();
        let mut got = Vec::new();
        for fr in t["frames"].as_array().unwrap() {
            let keys: Vec<u8> = fr["blocks"].as_str().unwrap().bytes().map(|c| BLOCK_CHARS.find(c as char).unwrap() as u8).collect();
            let w = fr["physW"].as_i64().unwrap() as i32;
            let shade = rng.int(0, 255) as f32;
            let mut cv = Canvas::new(w, 8, [shade, 255.0 - shade, shade * 0.5]);
            synth::draw_strip(&mut cv, &keys, 0.0, 0.0);
            let cap = cv.capture();
            let want = strip::parse_frame_bytes(&hex(fr["bytes"].as_str().unwrap())).expect("reference frame parses");
            let StripRead::Frame(f) = decode_strip(&cap.bgra()) else { panic!("{name}: frame at tick {} not decoded", fr["tick"]) };
            assert_eq!(frame_fields(&f), frame_fields(&want), "{name}: frame at tick {}", fr["tick"]);
            assert!(f.calib.min_gap > 40.0);
            nframes += 1;
            for m in reasm.push(&f, fr["t_ms"].as_f64().unwrap() as u64) {
                got.push(describe(&m));
            }
        }
        let want: Vec<String> = t["delivered"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| describe(&parse_message(&hex(h.as_str().unwrap())).ok().expect("reference message parses")))
            .collect();
        assert_eq!(got, want, "{name}: rebuilt messages");
        nmsgs += got.len();
        let st = reasm.stats();
        assert_eq!((st.bad_crc, st.parse_errors), (0, 0), "{name}");
    }
    println!("trace frames decoded {nframes}, messages rebuilt {nmsgs}");
}

#[test]
fn tag_vectors() {
    let t = load(vectors().join("tags.json"));
    let tags_v = t["tags"].as_array().unwrap();
    assert_eq!(tags_v.len(), 1024);
    let mut found = 0;
    for (i, e) in tags_v.iter().enumerate() {
        let (slot, row, sb) = (e["slot"].as_u64().unwrap() as u8, e["row"].as_u64().unwrap() as u8, e["sessBit"].as_u64().unwrap() as u8);
        let want: Vec<[u8; 3]> = e["colors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                let c = c.as_array().unwrap();
                [c[0].as_u64().unwrap() as u8, c[1].as_u64().unwrap() as u8, c[2].as_u64().unwrap() as u8]
            })
            .collect();
        assert_eq!(tags::tag_colors(slot, row, sb).to_vec(), want, "tag {slot}/{row}/{sb}");
        assert_eq!(u64::from(tp_bridge::crc::crc5_tag(e["payload"].as_u64().unwrap() as u16)), e["crc5"].as_u64().unwrap());
        // drawn (alternating hard and antialiased edges) and scanned back
        let mut cv = Canvas::new(160, 24, [20.0, 22.0, 30.0]);
        cv.aa = i % 2 == 0;
        synth::draw_tag(&mut cv, &synth::TagDraw {
            slot,
            row,
            sess_bit: sb,
            x: 20.0 + (i % 7) as f64 * 0.37,
            y: 5.0,
            block_w: 6.0,
            block_h: 13.0,
            gap: 0.0,
            alpha: 1.0,
            plate_w: 70.0,
            colors: None,
        });
        let cap = cv.capture();
        let mut hits = Vec::new();
        tags::scan(&cap.bgra(), image::Rect { x: 0, y: 0, w: 160, h: 24 }, None, &mut hits);
        assert_eq!(hits.len(), 1, "tag {slot}/{row}/{sb} scanned");
        assert_eq!((hits[0].slot, hits[0].row, hits[0].sess_bit), (slot, row, sb));
        found += 1;
    }
    assert_eq!(found, 1024);
}

#[test]
fn builder_vectors_parse_like_the_reference() {
    let v = load(vectors().join("messages.json"));
    let mut n = 0;
    for m in v.as_array().unwrap() {
        if m["fn"] == "utf8cut" {
            continue;
        }
        let b = hex(m["hex"].as_str().unwrap());
        let r = parse_message(&b);
        let p = &m["parsed"];
        if p.is_null() {
            assert!(r.is_err(), "{}: refused by the reference", m["note"]);
            continue;
        }
        let r = r.unwrap_or_else(|e| panic!("{}: {e:?}", m["note"]));
        let want = match p["type"].as_str().unwrap() {
            "translate" => {
                let pairs: Vec<String> = p["pairs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|q| format!("{}={}", q[0].as_str().unwrap(), q[1].as_str().unwrap()))
                    .collect();
                format!(
                    "translate {} {} {} {} {} {} {} [{}]",
                    p["req"], p["slot"], p["rows"], p["noCache"], p["plate"], p["target"].as_str().unwrap(),
                    p["text"].as_str().unwrap(), pairs.join(",")
                )
            }
            "cancel" => format!("cancel {} {}", p["req"], p["slot"]),
            "clear" => "clear".into(),
            "test" => format!("test {}", p["nonce"]),
            "view" => {
                let views: Vec<View> = p["views"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|q| View {
                        id: q["id"].as_u64().unwrap() as u8,
                        x: q["x"].as_u64().unwrap() as u16,
                        y: q["y"].as_u64().unwrap() as u16,
                        w: q["w"].as_u64().unwrap() as u16,
                        h: q["h"].as_u64().unwrap() as u16,
                        font_px: q["fontPx"].as_u64().unwrap() as u8,
                        row_px: q["rowPx"].as_u64().unwrap() as u8,
                    })
                    .collect();
                format!("view {views:?}")
            }
            other => panic!("type {other}"),
        };
        assert_eq!(describe(&r), want);
        n += 1;
    }
    assert!(n >= 25);
}

#[test]
fn distorted_strips_match_the_reference_decoder() {
    let v = load(vectors().join("strips.json"));
    let mut agree = 0;
    for (i, s) in v.as_array().unwrap().iter().enumerate() {
        let (w, h) = (s["w"].as_u64().unwrap() as u32, s["h"].as_u64().unwrap() as u32);
        let rgb = hex(s["rgb"].as_str().unwrap());
        let mut buf = Vec::with_capacity(rgb.len() / 3 * 4);
        for p in rgb.chunks(3) {
            buf.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
        let img = image::Bgra { px: &buf, width: w, height: h, stride: w * 4, origin_x: 0, origin_y: 0 };
        let got = decode_strip(&img);
        match (s["status"].as_str().unwrap(), got) {
            ("ok", StripRead::Frame(f)) => {
                let want = strip::parse_frame_bytes(&hex(s["bytes"].as_str().unwrap())).unwrap();
                assert_eq!(frame_fields(&f), frame_fields(&want), "strip {i}");
            }
            ("absent", StripRead::Absent) | ("torn", StripRead::Torn) => {}
            (st, StripRead::Rejected) if st != "ok" && st != "absent" && st != "torn" => {}
            (st, _) => panic!("strip {i} ({}, {}): reference {st}, decoder differs", s["mode"], s["kind"]),
        }
        agree += 1;
    }
    assert_eq!(agree, 36);
}

#[test]
fn every_mode_magic() {
    assert_eq!(Mode::ALL.map(|m| m.magic()), [0xD5, 0x2A, 0x5A]);
}

#[test]
fn lua_translate_with_marks_and_pairs() {
    // Bridge.TranslateMessage(9, 4, 2, 2, "fr", "lf2m dps \2[Linen Cloth]\3 cheap \2{skull}\3",
    //   {{"lf2m", "looking for 2 more"}, {"cheap", "pas cher"}})
    let b = hex(concat!(
        "10000904020202667200286c66326d2064707320025b4c696e656e20436c6f74685d0320636865617020027b73",
        "6b756c6c7d0302046c66326d126c6f6f6b696e6720666f722032206d6f7265056368656170087061732063686572"
    ));
    let Ok(Message::Translate(t)) = parse_message(&b) else { panic!("translate") };
    assert_eq!((t.req_id, t.slot, t.rows, t.no_cache, t.plate), (9, 4, 2, false, true));
    assert_eq!(t.target, "fr");
    assert_eq!(t.text.as_str(), "lf2m dps \u{2}[Linen Cloth]\u{3} cheap \u{2}{skull}\u{3}");
    let pairs: Vec<(&str, &str)> = t.pairs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(pairs, vec![("lf2m", "looking for 2 more"), ("cheap", "pas cher")]);
}

#[test]
fn lua_translate_utf8_and_limits() {
    // Bridge.TranslateMessage(65535, 126, 4, 1, "zh-TW", "\xc3\xa9t\xc3\xa9 \xe4\xbd\xa0\xe5\xa5\xbd", {})
    let b = hex("10ffff7e0401057a682d5457000cc3a974c3a920e4bda0e5a5bd00");
    let Ok(Message::Translate(t)) = parse_message(&b) else { panic!("translate") };
    assert_eq!((t.req_id, t.slot, t.rows, t.no_cache, t.plate), (65535, 126, 4, true, false));
    assert_eq!(t.target, "zh-TW");
    assert_eq!(t.text.as_str(), "\u{e9}t\u{e9} \u{4f60}\u{597d}");
    assert!(t.pairs.is_empty());
}

#[test]
fn lua_out_of_range_values_are_refused() {
    // slot 127 (TEST_SLOT) and rows 9 are written as given by the addon and refused here
    assert!(matches!(parse_message(&hex("1000017f010002656e00017800")), Err(ParseError::Range)));
    assert!(matches!(parse_message(&hex("10000100090002656e00017800")), Err(ParseError::Range)));
}

#[test]
fn lua_small_messages() {
    assert!(matches!(parse_message(&hex("1102017e")), Ok(Message::Cancel { req_id: 513, slot: 126 })));
    assert!(matches!(parse_message(&hex("12")), Ok(Message::Clear)));
    assert!(matches!(parse_message(&hex("139c40")), Ok(Message::Test { nonce: 40000 })));
    let Ok(Message::View(v)) = parse_message(&hex("140201000a038401ae00b4121403064003e801f400c80e10")) else {
        panic!("view")
    };
    assert_eq!(
        v,
        vec![
            View { id: 1, x: 10, y: 900, w: 430, h: 180, font_px: 18, row_px: 20 },
            View { id: 3, x: 1600, y: 1000, w: 500, h: 200, font_px: 14, row_px: 16 },
        ]
    );
}
