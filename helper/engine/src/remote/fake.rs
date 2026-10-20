//! A scripted [`Transport`] for the tests: replies in order (or from a responder), every request
//! recorded with its parsed JSON body.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use super::http::{HttpRequest, HttpResponse, Timeouts, Transport, TransportError, Url};

#[derive(Clone)]
pub enum FakeReply {
    /// Status, JSON body, `retry-after`.
    Http(u16, String, Option<Duration>),
    Fail(TransportError),
    /// Waits (at most the request's total timeout, then `Timeout`), then the inner reply.
    Delay(Duration, Box<FakeReply>),
}

impl FakeReply {
    pub fn ok(body: Value) -> FakeReply {
        FakeReply::Http(200, body.to_string(), None)
    }

    pub fn status(code: u16, body: Value) -> FakeReply {
        FakeReply::Http(code, body.to_string(), None)
    }
}

pub struct Seen {
    pub url: Url,
    pub headers: Vec<(&'static str, String)>,
    pub body: Value,
    pub timeouts: Timeouts,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

type Responder = Box<dyn Fn(&Value) -> FakeReply + Send>;

#[derive(Default)]
pub struct FakeTransport {
    replies: Mutex<VecDeque<FakeReply>>,
    responder: Mutex<Option<Responder>>,
    seen: Mutex<Vec<Seen>>,
}

impl FakeTransport {
    pub fn new(replies: Vec<FakeReply>) -> FakeTransport {
        FakeTransport { replies: Mutex::new(replies.into()), ..Default::default() }
    }

    pub fn push(&self, r: FakeReply) {
        self.replies.lock().unwrap().push_back(r);
    }

    /// Answers every request the scripted replies do not cover.
    pub fn respond_with(&self, f: impl Fn(&Value) -> FakeReply + Send + 'static) {
        *self.responder.lock().unwrap() = Some(Box::new(f));
    }

    pub fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    pub fn with_seen<R>(&self, f: impl FnOnce(&[Seen]) -> R) -> R {
        f(&self.seen.lock().unwrap())
    }
}

fn run(r: FakeReply, t: Timeouts) -> Result<HttpResponse, TransportError> {
    match r {
        FakeReply::Http(status, body, retry_after) => Ok(HttpResponse { status, retry_after, body: body.into_bytes() }),
        FakeReply::Fail(e) => Err(e),
        FakeReply::Delay(d, inner) => {
            if d >= t.total {
                std::thread::sleep(t.total);
                return Err(TransportError::Timeout);
            }
            std::thread::sleep(d);
            run(*inner, Timeouts { total: t.total - d, ..t })
        }
    }
}

impl Transport for FakeTransport {
    fn post(&self, req: &HttpRequest, t: Timeouts) -> Result<HttpResponse, TransportError> {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let scripted = self.replies.lock().unwrap().pop_front();
        let reply = match scripted {
            Some(r) => r,
            None => match self.responder.lock().unwrap().as_ref() {
                Some(f) => f(&body),
                None => FakeReply::Fail(TransportError::Other),
            },
        };
        self.seen.lock().unwrap().push(Seen { url: req.url.clone(), headers: req.headers.clone(), body, timeouts: t });
        run(reply, t)
    }
}
