//! Test helpers: temp folders and an in-process llama-server HTTP stub.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

/// A new empty folder under the system temp dir.
pub fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let p = std::env::temp_dir().join(format!("tp_engine_test_{}_{}_{}", tag, std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// One scripted `/completion` reply.
#[derive(Clone)]
pub enum Reply {
    /// 200 with `{"content": ...}`.
    Json(String),
    /// 200 SSE stream of content pieces, with a delay before each piece.
    Stream(Vec<String>, Duration),
    Status(u16),
    /// Never answers (until the stub stops or the client gives up).
    Hang,
    /// Closes the connection without an answer.
    Drop,
    /// Waits, then sends the inner reply.
    Delay(Duration, Box<Reply>),
}

type Responder = Box<dyn Fn(&Value) -> Reply + Send>;

#[derive(Default)]
struct State {
    health: u16,
    replies: VecDeque<Reply>,
    responder: Option<Responder>,
    last_auth: Option<String>,
    bodies: Vec<Value>,
    completions: u32,
}

/// Minimal HTTP/1.1 server answering `/health` and `/completion` like llama-server.
pub struct FakeHttp {
    port: u16,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    accept: Option<std::thread::JoinHandle<()>>,
}

impl FakeHttp {
    pub fn start() -> FakeHttp {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State { health: 200, ..Default::default() }));
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, st2) = (state.clone(), stop.clone());
        let accept = std::thread::spawn(move || {
            for conn in l.incoming() {
                if st2.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(c) = conn {
                    let (s3, st3) = (s2.clone(), st2.clone());
                    std::thread::spawn(move || handle(c, s3, st3));
                }
            }
        });
        FakeHttp { port, state, stop, accept: Some(accept) }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn set_health(&self, code: u16) {
        self.state.lock().unwrap().health = code;
    }

    pub fn push(&self, r: Reply) {
        self.state.lock().unwrap().replies.push_back(r);
    }

    /// Reply used when no scripted reply is queued.
    pub fn respond_with(&self, f: impl Fn(&Value) -> Reply + Send + 'static) {
        self.state.lock().unwrap().responder = Some(Box::new(f));
    }

    pub fn last_auth(&self) -> Option<String> {
        self.state.lock().unwrap().last_auth.clone()
    }

    pub fn bodies(&self) -> Vec<Value> {
        self.state.lock().unwrap().bodies.clone()
    }

    pub fn completions(&self) -> u32 {
        self.state.lock().unwrap().completions
    }
}

impl Drop for FakeHttp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(a) = self.accept.take() {
            let _ = a.join();
        }
    }
}

fn handle(c: TcpStream, state: Arc<Mutex<State>>, stop: Arc<AtomicBool>) {
    let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
    let mut r = BufReader::new(c.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut len = 0usize;
    let mut auth = None;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 {
            return;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0),
                "authorization" => auth = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; len];
    if r.read_exact(&mut body).is_err() {
        return;
    }
    let mut w = c;
    if method == "GET" && path == "/health" {
        let code = state.lock().unwrap().health;
        let _ = write_json(&mut w, code, &serde_json::json!({"status": if code == 200 { "ok" } else { "loading" }}));
        return;
    }
    if method == "POST" && path == "/completion" {
        let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let reply = {
            let mut s = state.lock().unwrap();
            s.last_auth = auth;
            s.completions += 1;
            s.bodies.push(v.clone());
            match s.replies.pop_front() {
                Some(r) => r,
                None => match &s.responder {
                    Some(f) => f(&v),
                    None => Reply::Json("OK".into()),
                },
            }
        };
        let stream = v.get("stream").and_then(Value::as_bool) == Some(true);
        send(&mut w, reply, &stop, stream);
        return;
    }
    let _ = write_json(&mut w, 404, &serde_json::json!({}));
}

fn write_json(w: &mut TcpStream, code: u16, v: &Value) -> std::io::Result<()> {
    let b = v.to_string();
    write!(w, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}", b.len())?;
    w.flush()
}

fn send(w: &mut TcpStream, reply: Reply, stop: &AtomicBool, stream: bool) {
    match reply {
        // Like llama-server, a streamed request gets its content as SSE.
        Reply::Json(s) if stream => send(w, Reply::Stream(vec![s], Duration::ZERO), stop, stream),
        Reply::Json(s) => {
            let _ = write_json(w, 200, &serde_json::json!({"content": s, "stop": true}));
        }
        Reply::Status(code) => {
            let _ = write_json(w, code, &serde_json::json!({"error": "x"}));
        }
        Reply::Stream(pieces, delay) => {
            if write!(w, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").is_err() {
                return;
            }
            let n = pieces.len();
            for (i, p) in pieces.into_iter().enumerate() {
                std::thread::sleep(delay);
                let ev = format!("data: {}\n\n", serde_json::json!({"content": p, "stop": i + 1 == n}));
                if write!(w, "{:x}\r\n{}\r\n", ev.len(), ev).is_err() || w.flush().is_err() {
                    return;
                }
            }
            let _ = write!(w, "0\r\n\r\n");
            let _ = w.flush();
        }
        Reply::Hang => {
            let t = Instant::now();
            while !stop.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(20));
                // A dead peer makes the probe write fail: stop hanging.
                if w.write(b"").is_err() {
                    return;
                }
            }
        }
        Reply::Drop => {
            let _ = w.shutdown(std::net::Shutdown::Both);
        }
        Reply::Delay(d, inner) => {
            std::thread::sleep(d);
            send(w, *inner, stop, stream);
        }
    }
}
