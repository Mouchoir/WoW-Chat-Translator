//! What the remote providers need from HTTP: one POST with headers and a JSON body, a status, the
//! `retry-after` header and the body. [`Transport`] is implemented by WinHTTP
//! (`super::winhttp`) and by a fake in the tests.

use std::time::Duration;

/// An absolute `http://` or `https://` URL, split for WinHTTP. No user info, no fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub https: bool,
    /// Host name or IP literal (IPv6 without its brackets), lower case.
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: String,
}

impl Url {
    /// Parses `scheme://host[:port][/path][?query]`. None for another scheme, user info, an empty
    /// or malformed host, a bad port, control characters or spaces.
    pub fn parse(s: &str) -> Option<Url> {
        let s = s.trim();
        if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return None;
        }
        let lower = s.to_ascii_lowercase();
        let (https, rest) = if lower.starts_with("https://") {
            (true, &s[8..])
        } else if lower.starts_with("http://") {
            (false, &s[7..])
        } else {
            return None;
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        if authority.is_empty() || authority.contains('@') {
            return None;
        }
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let close = v6.find(']')?;
            let host = &v6[..close];
            let after = &v6[close + 1..];
            if host.is_empty() || !host.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
                return None;
            }
            let port = match after {
                "" => None,
                p => Some(p.strip_prefix(':')?),
            };
            (host.to_string(), port)
        } else {
            let (host, port) = match authority.rsplit_once(':') {
                Some((h, p)) => (h, Some(p)),
                None => (authority, None),
            };
            if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_') {
                return None;
            }
            (host.to_string(), port)
        };
        let port = match port {
            None => {
                if https {
                    443
                } else {
                    80
                }
            }
            Some(p) => {
                if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                match p.parse::<u16>() {
                    Ok(n) if n > 0 => n,
                    _ => return None,
                }
            }
        };
        // The fragment never goes to the server.
        let tail = tail.split('#').next().unwrap_or("");
        let path = if tail.is_empty() {
            "/".to_string()
        } else if tail.starts_with('?') {
            format!("/{tail}")
        } else {
            tail.to_string()
        };
        Some(Url { https, host: host.to_ascii_lowercase(), port, path })
    }

    /// `localhost` itself, a 127.0.0.0/8 literal or `::1`: the text never leaves this PC. Other
    /// names (`*.localhost` included) are not: Windows sends them to the DNS server, whose answer
    /// may point anywhere.
    pub fn is_loopback(&self) -> bool {
        self.host == "localhost" || self.ip().is_some_and(|ip| ip.is_loopback())
    }

    /// A private network address literal (10/8, 172.16/12, 192.168/16, 169.254/16, fc00::/7,
    /// fe80::/10): a server on the player's own network (a NAS, another PC). Names never count.
    pub fn is_private_lan(&self) -> bool {
        match self.ip() {
            Some(std::net::IpAddr::V4(v4)) => v4.is_private() || v4.is_link_local(),
            Some(std::net::IpAddr::V6(v6)) => {
                let s = v6.segments()[0];
                (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
            }
            None => false,
        }
    }

    /// The host as an IP literal, if it is one.
    fn ip(&self) -> Option<std::net::IpAddr> {
        self.host.parse::<std::net::IpAddr>().ok()
    }

    /// This URL with `suffix` appended to its path (one `/` between them; the query is dropped).
    pub fn join(&self, suffix: &str) -> Url {
        let base = self.path.split('?').next().unwrap_or("/").trim_end_matches('/');
        let suffix = suffix.trim_start_matches('/');
        Url { path: format!("{base}/{suffix}"), ..self.clone() }
    }

    /// `host` or `host:port` when the port is not the scheme's default (the identity of an
    /// OpenAI-compatible server in the cache key and the consent id).
    pub fn authority(&self) -> String {
        let default = if self.https { 443 } else { 80 };
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == default {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

/// One POST. No Debug: the headers hold the API key and the body holds chat text.
pub struct HttpRequest {
    pub url: Url,
    /// Extra headers (`Content-Type: application/json` is always sent).
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Drop for HttpRequest {
    /// The header values (one holds the key) are zeroed before their memory is freed (best
    /// effort: a copy made while a string grew is not reached).
    fn drop(&mut self) {
        for (_, v) in &mut self.headers {
            // SAFETY: zeros are valid UTF-8.
            unsafe { v.as_mut_vec() }.fill(0);
        }
    }
}

/// The answer. No Debug: the body may echo chat text.
pub struct HttpResponse {
    pub status: u16,
    /// `retry-after` in seconds (the HTTP-date form is ignored).
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

/// Why no answer came back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The connect or total timeout passed.
    Timeout,
    /// Name resolution, connection refused or reset.
    Connect,
    /// TLS handshake or certificate failure.
    Tls,
    /// Anything else (WinHTTP failure, oversized answer).
    Other,
}

/// Timeouts of one POST.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    pub total: Duration,
}

/// The largest answer read (1 MiB): a translation is a few hundred bytes.
pub const MAX_BODY: usize = 1 << 20;

pub trait Transport: Send + Sync {
    fn post(&self, req: &HttpRequest, t: Timeouts) -> Result<HttpResponse, TransportError>;
}

/// `retry-after` value: whole seconds only.
pub fn parse_retry_after(v: &str) -> Option<Duration> {
    let v = v.trim();
    (!v.is_empty() && v.len() <= 6 && v.bytes().all(|b| b.is_ascii_digit())).then(|| Duration::from_secs(v.parse().unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_parsing() {
        let u = Url::parse("https://api.openai.com/v1").unwrap();
        assert_eq!((u.https, u.host.as_str(), u.port, u.path.as_str()), (true, "api.openai.com", 443, "/v1"));
        assert_eq!(u.join("chat/completions").path, "/v1/chat/completions");
        assert_eq!(Url::parse("https://api.openai.com/v1/").unwrap().join("/chat/completions").path, "/v1/chat/completions");
        let u = Url::parse("HTTP://LocalHost:11434/v1").unwrap();
        assert_eq!((u.https, u.host.as_str(), u.port), (false, "localhost", 11434));
        assert!(u.is_loopback());
        assert_eq!(u.authority(), "localhost:11434");
        assert_eq!(Url::parse("https://openrouter.ai/api/v1").unwrap().authority(), "openrouter.ai");
        let u = Url::parse("http://[::1]:1234/v1").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("::1", 1234));
        assert!(u.is_loopback());
        assert_eq!(u.authority(), "[::1]:1234");
        assert!(Url::parse("http://127.0.0.1:8080").unwrap().is_loopback());
        assert_eq!(Url::parse("http://127.0.0.1:8080").unwrap().path, "/");
        assert_eq!(Url::parse("https://x.example?a=1#frag").unwrap().path, "/?a=1");
        assert!(!Url::parse("http://192.168.1.10:1234/v1").unwrap().is_loopback());
        assert!(!Url::parse("http://127.0.0.1.evil.example/").unwrap().is_loopback());
        assert!(!Url::parse("https://localhost.evil.example/").unwrap().is_loopback());
        // review finding: Windows resolves *.localhost through DNS
        assert!(!Url::parse("http://my-llm.localhost:8080/v1").unwrap().is_loopback());
        assert!(Url::parse("http://127.8.9.10/").unwrap().is_loopback());
        assert!(!Url::parse("http://127.0.0.01/").unwrap().is_loopback(), "not a valid literal");
        // private network literals (a NAS), never names
        for lan in ["http://192.168.1.69:11434/v1", "http://10.0.0.5/", "http://172.20.1.1/", "http://169.254.3.4/", "http://[fd12::1]/", "http://[fe80::1]/"] {
            assert!(Url::parse(lan).unwrap().is_private_lan(), "{lan}");
        }
        for not_lan in ["http://172.32.0.1/", "http://8.8.8.8/", "http://nas.local/", "http://127.0.0.1/", "http://[2001:db8::1]/"] {
            assert!(!Url::parse(not_lan).unwrap().is_private_lan(), "{not_lan}");
        }
        for bad in [
            "ftp://x.example",
            "api.openai.com/v1",
            "https://",
            "https://user:pw@x.example/",
            "https://x.example:0/",
            "https://x.example:99999/",
            "https://x.example:ab/",
            "https://x .example/",
            "https://x.example/\u{7}",
            "http://[zz]/",
            "https://exa$mple.com/",
        ] {
            assert!(Url::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn retry_after_values() {
        assert_eq!(parse_retry_after(" 7 "), Some(Duration::from_secs(7)));
        assert_eq!(parse_retry_after("0"), Some(Duration::ZERO));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after(""), None);
    }
}
