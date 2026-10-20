//! [`Transport`] over WinHTTP: HTTPS with the OS's TLS (1.2 and 1.3 only), the system proxy
//! settings (`WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY`; a loopback host or a private network address
//! literal goes direct), no redirects
//! (a redirect would carry the key to another host), no cookies, no automatic authentication.
//!
//! Timeouts: WinHTTP gets resolve and connect `Timeouts::connect`, send and receive the time left
//! of `Timeouts::total`. Fact (2026-10-20, loopback stub that reads the request and never
//! answers): synchronous `WinHttpSendRequest` returned only after about 4 s with every timeout
//! at 600 ms, and only when the server answered with 5 s timeouts, so WinHTTP's own timeouts do
//! not bound the total. Each request therefore runs on its own short thread and the caller waits
//! at most `Timeouts::total`; a request that is late finishes (or times out) on its thread and
//! its result is dropped. No handle is ever closed from another thread.
//!
//! One WinHTTP session per proxy mode lives as long as the transport (and any request thread
//! still running), so connections are reused.

use std::ffi::c_void;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::GetLastError;
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_DISABLE_AUTHENTICATION,
    WINHTTP_DISABLE_COOKIES, WINHTTP_FLAG_SECURE, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3, WINHTTP_OPEN_REQUEST_FLAGS,
    WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, WINHTTP_OPTION_RECEIVE_TIMEOUT, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_OPTION_SECURE_PROTOCOLS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_RETRY_AFTER, WINHTTP_QUERY_STATUS_CODE,
};

use super::http::{parse_retry_after, HttpRequest, HttpResponse, Timeouts, Transport, TransportError, Url, MAX_BODY};

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: a handle WinHTTP gave us, closed once, by the thread that used it.
            let _ = unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// [system proxy, direct] session handles (0 = not opened yet), closed when the last user
/// (the transport or a request thread) drops them.
struct Sessions(Mutex<[usize; 2]>);

impl Sessions {
    fn get(&self, direct: bool) -> Result<*mut c_void, TransportError> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let i = usize::from(direct);
        if s[i] == 0 {
            let access = if direct { WINHTTP_ACCESS_TYPE_NO_PROXY } else { WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY };
            // SAFETY: static strings; a null handle is checked.
            let h = unsafe { WinHttpOpen(w!("TranslateCompanion"), access, PCWSTR::null(), PCWSTR::null(), 0) };
            if h.is_null() {
                return Err(last_error());
            }
            let both = (WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3).to_le_bytes();
            // SAFETY: a valid session handle and a 4-byte option value.
            if unsafe { WinHttpSetOption(Some(h as *const c_void), WINHTTP_OPTION_SECURE_PROTOCOLS, Some(&both)) }.is_err() {
                // A build without TLS 1.3 in WinHTTP: TLS 1.2 alone.
                let _ = unsafe { WinHttpSetOption(Some(h as *const c_void), WINHTTP_OPTION_SECURE_PROTOCOLS, Some(&WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2.to_le_bytes())) };
            }
            s[i] = h as usize;
        }
        Ok(s[i] as *mut c_void)
    }
}

impl Drop for Sessions {
    fn drop(&mut self) {
        let s = self.0.get_mut().unwrap_or_else(|e| e.into_inner());
        for h in s.iter_mut() {
            if *h != 0 {
                drop(Handle(*h as *mut c_void));
                *h = 0;
            }
        }
    }
}

pub struct WinHttp {
    sessions: Arc<Sessions>,
}

impl Default for WinHttp {
    fn default() -> Self {
        WinHttp::new()
    }
}

impl WinHttp {
    pub fn new() -> WinHttp {
        WinHttp { sessions: Arc::new(Sessions(Mutex::new([0, 0]))) }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// WinHTTP error codes (winhttp.h) to our kinds.
fn map_code(code: u32) -> TransportError {
    match code {
        12002 => TransportError::Timeout,
        // name not resolved, cannot connect, connection error, invalid server response
        12007 | 12029 | 12030 | 12152 => TransportError::Connect,
        // secure failure, certificate CN/date invalid, invalid CA, client cert needed, invalid
        // certificate, secure channel error
        12175 | 12038 | 12037 | 12045 | 12044 | 12169 | 12157 => TransportError::Tls,
        _ => TransportError::Other,
    }
}

fn map_err(e: windows::core::Error) -> TransportError {
    let hr = e.code().0 as u32;
    // HRESULT_FROM_WIN32: facility 7.
    if hr >> 16 == 0x8007 {
        map_code(hr & 0xFFFF)
    } else {
        TransportError::Other
    }
}

fn last_error() -> TransportError {
    // SAFETY: no arguments.
    map_code(unsafe { GetLastError() }.0)
}

fn millis(d: Duration) -> i32 {
    d.as_millis().clamp(1, i32::MAX as u128) as i32
}

fn set_receive_timeout(h: *mut c_void, d: Duration) {
    let v = (millis(d) as u32).to_le_bytes();
    // SAFETY: a valid request handle and 4-byte option values.
    unsafe {
        let _ = WinHttpSetOption(Some(h as *const c_void), WINHTTP_OPTION_RECEIVE_TIMEOUT, Some(&v));
        let _ = WinHttpSetOption(Some(h as *const c_void), WINHTTP_OPTION_RECEIVE_RESPONSE_TIMEOUT, Some(&v));
    }
}

/// The request as the thread owns it (the header values hold the key: zeroed after sending).
struct Owned {
    url: Url,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        for (_, v) in self.headers.iter_mut() {
            // SAFETY: zeros are valid UTF-8.
            unsafe { v.as_mut_vec() }.fill(0);
        }
    }
}

impl Transport for WinHttp {
    fn post(&self, req: &HttpRequest, t: Timeouts) -> Result<HttpResponse, TransportError> {
        let start = Instant::now();
        let job = Owned { url: req.url.clone(), headers: req.headers.clone(), body: req.body.clone() };
        let sessions = self.sessions.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tp-winhttp".into())
            .spawn(move || {
                let r = post_blocking(&sessions, &job, t, start);
                let _ = tx.send(r);
            })
            .map_err(|_| TransportError::Other)?;
        match rx.recv_timeout(t.total.saturating_sub(start.elapsed())) {
            Ok(r) => r,
            Err(_) => Err(TransportError::Timeout),
        }
    }
}

fn post_blocking(sessions: &Sessions, req: &Owned, t: Timeouts, start: Instant) -> Result<HttpResponse, TransportError> {
    let left = || t.total.saturating_sub(start.elapsed());
    // this PC and private network literals (a NAS) go direct, never through a proxy
    let session = sessions.get(req.url.is_loopback() || req.url.is_private_lan())?;
    let host = wide(&req.url.host);
    // SAFETY: valid session handle and a NUL-terminated host.
    let conn = Handle(unsafe { WinHttpConnect(session, PCWSTR(host.as_ptr()), req.url.port, 0) });
    if conn.0.is_null() {
        return Err(last_error());
    }
    let path = wide(&req.url.path);
    let flags = if req.url.https { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
    // SAFETY: valid connection handle, NUL-terminated strings, default accept types.
    let r = Handle(unsafe { WinHttpOpenRequest(conn.0, w!("POST"), PCWSTR(path.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags) });
    if r.0.is_null() {
        return Err(last_error());
    }
    // SAFETY: valid request handle; option values are 4-byte integers.
    unsafe {
        let c = millis(t.connect.min(left()));
        let _ = WinHttpSetTimeouts(r.0, c, c, millis(left()), millis(left()));
        let _ = WinHttpSetOption(Some(r.0 as *const c_void), WINHTTP_OPTION_REDIRECT_POLICY, Some(&WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_le_bytes()));
        let _ = WinHttpSetOption(Some(r.0 as *const c_void), WINHTTP_OPTION_DISABLE_FEATURE, Some(&(WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION).to_le_bytes()));
    }
    set_receive_timeout(r.0, left());
    let mut hdr = String::from("Content-Type: application/json\r\nAccept: application/json\r\n");
    for (k, v) in &req.headers {
        hdr.push_str(k);
        hdr.push_str(": ");
        hdr.push_str(v);
        hdr.push_str("\r\n");
    }
    let mut hdr_w: Vec<u16> = hdr.encode_utf16().collect();
    // SAFETY: valid request handle; the header and body buffers outlive the call.
    let sent = unsafe { WinHttpSendRequest(r.0, Some(&hdr_w), Some(req.body.as_ptr() as *const c_void), req.body.len() as u32, req.body.len() as u32, 0) };
    // The headers held the key.
    hdr_w.fill(0);
    // SAFETY: zeros are valid UTF-8.
    unsafe { hdr.as_mut_vec() }.fill(0);
    sent.map_err(map_err)?;
    set_receive_timeout(r.0, left());
    // SAFETY: valid request handle.
    unsafe { WinHttpReceiveResponse(r.0, std::ptr::null_mut()) }.map_err(map_err)?;
    let mut status: u32 = 0;
    let mut len: u32 = 4;
    // SAFETY: a 4-byte buffer for a numeric header.
    unsafe { WinHttpQueryHeaders(r.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut u32 as *mut c_void), &mut len, std::ptr::null_mut()) }
        .map_err(map_err)?;
    let mut buf = [0u16; 64];
    let mut blen: u32 = (buf.len() * 2) as u32;
    // SAFETY: a 128-byte buffer; a missing header is an error we ignore.
    let retry_after = unsafe { WinHttpQueryHeaders(r.0, WINHTTP_QUERY_RETRY_AFTER, PCWSTR::null(), Some(buf.as_mut_ptr() as *mut c_void), &mut blen, std::ptr::null_mut()) }
        .ok()
        .and_then(|_| parse_retry_after(&String::from_utf16_lossy(&buf[..(blen as usize / 2).min(buf.len())])));
    let mut body = Vec::new();
    loop {
        if start.elapsed() >= t.total {
            return Err(TransportError::Timeout);
        }
        set_receive_timeout(r.0, left());
        let mut avail: u32 = 0;
        // SAFETY: valid request handle.
        unsafe { WinHttpQueryDataAvailable(r.0, &mut avail) }.map_err(map_err)?;
        if avail == 0 {
            break;
        }
        let at = body.len();
        if at + avail as usize > MAX_BODY {
            return Err(TransportError::Other);
        }
        body.resize(at + avail as usize, 0);
        let mut read: u32 = 0;
        // SAFETY: the buffer has `avail` bytes free at `at`.
        unsafe { WinHttpReadData(r.0, body[at..].as_mut_ptr() as *mut c_void, avail, &mut read) }.map_err(map_err)?;
        body.truncate(at + read as usize);
        if read == 0 {
            break;
        }
    }
    Ok(HttpResponse { status: status.min(u32::from(u16::MAX)) as u16, retry_after, body })
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    use super::super::http::Url;
    use super::*;

    /// What the stub received.
    #[derive(Default, Clone)]
    struct Seen {
        line: String,
        headers: Vec<String>,
        body: Vec<u8>,
    }

    enum Act {
        Reply(String),
        Hang,
    }

    /// A one-request-per-connection HTTP/1.1 stub on 127.0.0.1.
    fn stub(acts: Vec<Act>) -> (u16, Arc<Mutex<Vec<Seen>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let s2 = seen.clone();
        std::thread::spawn(move || {
            let mut acts = acts.into_iter();
            for conn in l.incoming() {
                let Ok(c) = conn else { return };
                let Some(act) = acts.next() else { return };
                let s3 = s2.clone();
                std::thread::spawn(move || serve(c, act, s3));
            }
        });
        (port, seen)
    }

    fn serve(c: TcpStream, act: Act, seen: Arc<Mutex<Vec<Seen>>>) {
        let mut r = BufReader::new(c.try_clone().unwrap());
        let mut s = Seen::default();
        r.read_line(&mut s.line).unwrap();
        let mut len = 0usize;
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap_or(0);
            }
            s.headers.push(h.trim_end().to_string());
        }
        s.body = vec![0; len];
        r.read_exact(&mut s.body).unwrap();
        seen.lock().unwrap().push(s);
        let mut w = c;
        match act {
            Act::Reply(text) => {
                let _ = w.write_all(text.as_bytes());
            }
            Act::Hang => std::thread::sleep(Duration::from_secs(5)),
        }
    }

    fn reply(status: &str, extra: &str, body: &str) -> Act {
        Act::Reply(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()))
    }

    fn req(port: u16, path: &str) -> HttpRequest {
        HttpRequest {
            url: Url::parse(&format!("http://127.0.0.1:{port}{path}")).unwrap(),
            headers: vec![("x-api-key", "k-123".into()), ("anthropic-version", "2023-06-01".into())],
            body: "{\"a\":\"été\"}".as_bytes().to_vec(),
        }
    }

    const T: Timeouts = Timeouts { connect: Duration::from_secs(2), total: Duration::from_secs(5) };

    #[test]
    fn post_against_a_loopback_stub() {
        let (port, seen) = stub(vec![
            reply("200 OK", "", r#"{"ok":"Bonjour à tous"}"#),
            reply("429 Too Many Requests", "Retry-After: 3\r\n", r#"{"error":{"message":"slow down"}}"#),
            reply("307 Temporary Redirect", "Location: http://127.0.0.1:1/elsewhere\r\n", ""),
        ]);
        let t = WinHttp::new();
        let r = t.post(&req(port, "/v1/messages"), T).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(String::from_utf8(r.body).unwrap(), r#"{"ok":"Bonjour à tous"}"#);
        assert_eq!(r.retry_after, None);
        let s = seen.lock().unwrap()[0].clone();
        assert_eq!(s.line.trim_end(), "POST /v1/messages HTTP/1.1");
        assert!(s.headers.iter().any(|h| h == "x-api-key: k-123"));
        assert!(s.headers.iter().any(|h| h == "anthropic-version: 2023-06-01"));
        assert!(s.headers.iter().any(|h| h.eq_ignore_ascii_case("content-type: application/json")));
        assert!(!s.headers.iter().any(|h| h.to_ascii_lowercase().starts_with("cookie")));
        assert_eq!(s.body, "{\"a\":\"été\"}".as_bytes());
        let r = t.post(&req(port, "/x"), T).unwrap();
        assert_eq!((r.status, r.retry_after), (429, Some(Duration::from_secs(3))));
        // redirects are never followed (the key would go to another host)
        let r = t.post(&req(port, "/y"), T).unwrap();
        assert_eq!(r.status, 307);
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[test]
    fn timeout_and_refused_connection() {
        let (port, _) = stub(vec![Act::Hang]);
        let t = WinHttp::new();
        let start = Instant::now();
        let e = t.post(&req(port, "/"), Timeouts { connect: Duration::from_secs(2), total: Duration::from_millis(600) }).err();
        assert_eq!(e, Some(TransportError::Timeout));
        assert!(start.elapsed() < Duration::from_secs(3), "total timeout enforced: {:?}", start.elapsed());
        // a port nobody listens on
        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let e = t.post(&req(free, "/"), T).err();
        assert_eq!(e, Some(TransportError::Connect));
    }

    #[test]
    fn error_codes() {
        assert_eq!(map_code(12002), TransportError::Timeout);
        assert_eq!(map_code(12029), TransportError::Connect);
        assert_eq!(map_code(12175), TransportError::Tls);
        assert_eq!(map_code(5), TransportError::Other);
    }
}
