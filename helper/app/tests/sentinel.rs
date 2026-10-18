//! Log sentinel test (P4 acceptance): the scene, the engine (echo translator), the overlay
//! rendering and the logger handle a request whose text, glossary pairs and translation all carry
//! "ZZSENTINELZZ"; no file the Helper writes may contain it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tp_bridge::cards::{Card, Mask, RowBand};
use tp_bridge::image::Rect;
use tp_bridge::msg::{ChatText, Message, Translate};
use tp_engine::{BackendPref, Engine, EngineConfig, EngineEvent, JobKey, Status, Translator, Update};
use tp_helper::app::{card_json, msg_json};
use tp_helper::i18n::Lang;
use tp_helper::log::{DiagSink, Level, Logger};
use tp_helper::overlay::{status_name, OverlayManager};
use tp_helper::scene::{Action, SceneMachine, TickInput, WowGeom, WowTick};

const SENTINEL: &str = "ZZSENTINELZZ";

fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("tp_sentinel_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn card(slot: u8) -> Card {
    let band = |row: u8, y: i32| RowBand {
        row,
        text_rect: Rect { x: 60, y, w: 300, h: 16 },
        mask: Some(Mask { rect: Rect { x: 57, y, w: 310, h: 16 }, bits: vec![255; 310 * 16] }),
        tag_rect: Rect { x: 10, y, w: 47, h: 16 },
    };
    Card { slot, view_id: 0, bands: vec![band(0, 900), band(1, 918)], gaps: vec![], alpha: 1.0, plate: true, plate_rgb: [32, 40, 64], text_width_px: 300 }
}

#[test]
fn sentinel_never_reaches_a_file() {
    let dir = temp_dir();
    let log = Logger::start(&dir.join("logs"), Level::Debug);
    let diag = DiagSink::open(&dir.join("diag.jsonl"));

    let l = log.clone();
    let engine_log: Arc<dyn Fn(EngineEvent) + Send + Sync> = Arc::new(move |ev: EngineEvent| l.engine(&ev));
    let cfg = EngineConfig {
        runtime_dir: None,
        model_path: None,
        backend: BackendPref::Cpu,
        gpu_margin_mb: 1500,
        cpu_threads: 0,
        idle_unload: Duration::from_secs(600),
        keep_loaded: false,
        translator: Translator::Echo,
        data_dir: dir.join("data"),
        log: engine_log,
    };
    let (tx, rx) = mpsc::channel::<Update>();
    let engine = Engine::start(cfg, Box::new(move |u| drop(tx.send(u))), Box::new(|_| {}));
    let handle = engine.handle();

    // The request, as the strip would deliver it.
    let text = format!("hello {SENTINEL} \u{2}[{SENTINEL} Cloth]\u{3} cheap");
    let msg = Message::Translate(Translate {
        req_id: 7,
        slot: 3,
        rows: 2,
        no_cache: false,
        plate: true,
        target: "en".into(),
        text: ChatText::new(text),
        pairs: vec![(ChatText::new(SENTINEL.into()), ChatText::new(format!("{SENTINEL} meaning")))],
    });
    let (record, event) = msg_json(&msg);
    diag.emit(record);
    log.app(Level::Info, event);
    let mut scene = SceneMachine::new();
    for a in scene.on_message(77, msg) {
        match a {
            Action::Submit(job) => handle.submit(job),
            Action::Cancel(k) => handle.cancel(k),
            Action::ClearSession(s) => handle.clear_session(s),
            _ => {}
        }
    }

    // Wait for the echo translation.
    let key = JobKey { session: 77, req_id: 7 };
    let mut results: HashMap<JobKey, Status> = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut done_text = None;
    while Instant::now() < deadline {
        let Ok(u) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
        diag.emit(json!({"ev": "engine", "req": u.key.req_id, "status": status_name(&u.status)}));
        if let Status::Done(t) = &u.status {
            done_text = Some(t.as_str().to_string());
        }
        let finished = matches!(u.status, Status::Done(_));
        results.insert(u.key, u.status);
        if finished {
            break;
        }
    }
    let done = done_text.expect("echo translation");
    assert!(done.contains(SENTINEL), "the pipeline really carried the sentinel");

    // Scene and rendering (no window), with the diag records the app writes.
    let input = TickInput {
        now_ms: 100,
        wow: Some(WowTick { geom: WowGeom { hwnd: 1, origin_x: 0, origin_y: 0, client_w: 1920, client_h: 1080 }, minimized: false, foreground: true }),
        fresh: true,
    };
    let s = scene.update(&input, vec![card(3)], &[], &mut |_| false);
    assert_eq!(s.card_count(), 1);
    let list: Vec<_> = s.views.iter().flat_map(|v| v.cards.iter()).map(|c| card_json(c, "done")).collect();
    diag.emit(json!({"ev": "cards", "cards": list}));
    let mut overlay = OverlayManager::new(true, true, [0x9F, 0xD4, 0xFF], 1.0);
    let reports = overlay.render(&s, &results, 1.0, Lang::En);
    assert!(!reports.is_empty() && reports[0].stats.text_px > 0, "the translation was drawn into memory");
    for r in reports {
        diag.emit(json!({"ev": "overlay", "view": r.id, "shown": r.shown, "rect": r.rect, "px": r.px}));
    }
    assert!(results.contains_key(&key));

    engine.shutdown();
    diag.flush();
    log.shutdown();

    let mut all = Vec::new();
    files(&dir, &mut all);
    assert!(all.iter().any(|p| p.ends_with("helper.log")), "{all:?}");
    assert!(all.iter().any(|p| p.ends_with("diag.jsonl")));
    let needle = SENTINEL.as_bytes();
    let lower = SENTINEL.to_lowercase();
    for p in &all {
        let bytes = std::fs::read(p).unwrap();
        assert!(!bytes.is_empty() || !p.ends_with("helper.log"), "empty log");
        assert!(!bytes.windows(needle.len()).any(|w| w == needle), "{} contains the sentinel", p.display());
        assert!(!bytes.windows(needle.len()).any(|w| w == lower.as_bytes()), "{} contains the sentinel", p.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
