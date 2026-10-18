//! HTTP client for llama-server: /health, warm-up and /completion (streamed on CPU).
//!
//! The server only listens on 127.0.0.1; no proxy is ever used, whatever the environment says.

use std::io::{BufRead, BufReader};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Streaming partial updates at most 5 times per second.
pub const PARTIAL_EVERY: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReqError {
    /// Non-200 status.
    Http(u16),
    /// Connection refused or reset (the server died or was killed).
    Io,
    Timeout,
    /// 200 but no usable JSON.
    Bad,
}

#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    base: String,
    auth: String,
}

/// The `/completion` body (design D.4 "Per request").
pub fn completion_body(prompt: &str, n_predict: u32, grammar: Option<&str>, stream: bool) -> Value {
    let mut b = json!({
        "prompt": prompt,
        "n_predict": n_predict,
        "temperature": 0.2,
        "top_p": 0.8,
        "top_k": 20,
        "seed": 42,
        "stop": ["<|im_end|>"],
        "cache_prompt": true,
        "stream": stream,
    });
    if let Some(g) = grammar {
        b["grammar"] = json!(g);
    }
    b
}

fn map_err(e: ureq::Error) -> ReqError {
    match e {
        ureq::Error::Timeout(_) => ReqError::Timeout,
        ureq::Error::StatusCode(c) => ReqError::Http(c),
        _ => ReqError::Io,
    }
}

impl Client {
    pub fn new(port: u16, key: &str) -> Client {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(2)))
            .max_idle_connections(4)
            .build()
            .into();
        Client { agent, base: format!("http://127.0.0.1:{port}"), auth: format!("Bearer {key}") }
    }

    /// `GET /health` status (200 when the model is loaded, 503 while loading); None when the
    /// server does not answer within `timeout`.
    pub fn health(&self, timeout: Duration) -> Option<u16> {
        let r = self.agent.get(format!("{}/health", self.base)).config().timeout_global(Some(timeout)).build().call();
        r.ok().map(|r| r.status().as_u16())
    }

    /// `POST /completion`. With `stream`, `on_partial` receives the text so far at most 5 times
    /// per second. Returns the full content.
    pub fn completion(&self, body: &Value, timeout: Duration, on_partial: &mut dyn FnMut(&str)) -> Result<String, ReqError> {
        let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        let resp = self
            .agent
            .post(format!("{}/completion", self.base))
            .header("Authorization", &self.auth)
            .config()
            .timeout_global(Some(timeout))
            .build()
            .send_json(body)
            .map_err(map_err)?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(ReqError::Http(status));
        }
        if !stream {
            let v: Value = resp.into_body().read_json().map_err(map_err_body)?;
            return v.get("content").and_then(Value::as_str).map(String::from).ok_or(ReqError::Bad);
        }
        let reader = BufReader::new(resp.into_body().into_reader());
        let mut text = String::new();
        let mut last = Instant::now();
        let mut sent_len = 0;
        for line in reader.lines() {
            let line = line.map_err(|e| if e.kind() == std::io::ErrorKind::TimedOut { ReqError::Timeout } else { ReqError::Io })?;
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(c) = v.get("content").and_then(Value::as_str) {
                text.push_str(c);
            }
            if v.get("stop").and_then(Value::as_bool) == Some(true) {
                break;
            }
            if text.len() != sent_len && last.elapsed() >= PARTIAL_EVERY {
                on_partial(&text);
                sent_len = text.len();
                last = Instant::now();
            }
        }
        Ok(text)
    }
}

fn map_err_body(e: ureq::Error) -> ReqError {
    match e {
        ureq::Error::Timeout(_) => ReqError::Timeout,
        ureq::Error::Io(_) => ReqError::Io,
        _ => ReqError::Bad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeHttp, Reply};

    #[test]
    fn body_fields() {
        let b = completion_body("P", 40, Some("root ::= [a]*"), true);
        assert_eq!(b["n_predict"], 40);
        assert_eq!(b["temperature"], 0.2);
        assert_eq!(b["top_p"], 0.8);
        assert_eq!(b["top_k"], 20);
        assert_eq!(b["seed"], 42);
        assert_eq!(b["stop"], json!(["<|im_end|>"]));
        assert_eq!(b["cache_prompt"], true);
        assert_eq!(b["stream"], true);
        assert_eq!(b["grammar"], "root ::= [a]*");
        assert!(completion_body("P", 1, None, false).get("grammar").is_none());
    }

    #[test]
    fn health_json_and_stream() {
        let http = FakeHttp::start();
        let c = Client::new(http.port(), "k123");
        http.set_health(503);
        assert_eq!(c.health(Duration::from_millis(500)), Some(503));
        http.set_health(200);
        assert_eq!(c.health(Duration::from_millis(500)), Some(200));
        http.push(Reply::Json("Bonjour".into()));
        let mut parts = Vec::new();
        let out = c.completion(&completion_body("P", 8, None, false), Duration::from_secs(5), &mut |p| parts.push(p.to_string()));
        assert_eq!(out.unwrap(), "Bonjour");
        assert_eq!(http.last_auth().as_deref(), Some("Bearer k123"));
        http.push(Reply::Stream(vec!["Bon".into(), "jour".into(), " à tous".into()], Duration::from_millis(250)));
        let out = c.completion(&completion_body("P", 8, None, true), Duration::from_secs(5), &mut |p| parts.push(p.to_string()));
        assert_eq!(out.unwrap(), "Bonjour à tous");
        assert!(!parts.is_empty() && parts.iter().all(|p| "Bonjour à tous".starts_with(p.as_str())));
        http.push(Reply::Status(401));
        assert_eq!(c.completion(&completion_body("P", 8, None, false), Duration::from_secs(5), &mut |_| {}), Err(ReqError::Http(401)));
        http.push(Reply::Hang);
        let t = Instant::now();
        let r = c.completion(&completion_body("P", 8, None, false), Duration::from_millis(400), &mut |_| {});
        assert_eq!(r, Err(ReqError::Timeout));
        assert!(t.elapsed() < Duration::from_secs(3));
        http.push(Reply::Drop);
        assert_eq!(c.completion(&completion_body("P", 8, None, false), Duration::from_secs(5), &mut |_| {}), Err(ReqError::Io));
        drop(http);
        assert_eq!(c.health(Duration::from_millis(300)), None);
    }
}
