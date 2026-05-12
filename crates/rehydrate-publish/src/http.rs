//! Restricted ureq agent: every outbound request must target the
//! single host the user configured for this client. Redirects are
//! disabled (a Set-Cookie + 302 to evil.com is the obvious data-
//! exfiltration vector for a hijacked CMS endpoint). The
//! `no_egress.rs` integration test greps this file to assert the
//! `Agent::new()` constructor isn't used directly by the rest of
//! the crate.
//!
//! Deliberately mirrors `crates/rehydrate-ocr/src/http.rs`. See the
//! preamble there for why the duplication is load-bearing for the
//! `no_egress` test.

use std::net::IpAddr;
use std::time::Duration;

use crate::PublishError;

/// Wraps `ureq::Agent` and pins it to one host. Constructing a new
/// `RestrictedAgent` is the only sanctioned way to obtain an agent
/// inside this crate — the `Agent::new` API isn't used directly
/// elsewhere, so a refactor that bypasses this wrapper is visible
/// in code review.
pub struct RestrictedAgent {
    inner: ureq::Agent,
    host: String,
}

impl RestrictedAgent {
    /// Construct an agent pinned to the host of `base_url`. Returns
    /// `InvalidConfig` if the URL doesn't parse or has no host, and
    /// `UnsafeUrl` if it points at something this crate refuses to
    /// fetch (see [`validate_remote_url`]).
    pub fn for_base(base_url: &str) -> Result<Self, PublishError> {
        validate_remote_url(base_url)?;
        let host = host_of(base_url)
            .ok_or_else(|| PublishError::InvalidConfig(format!("URL has no host: {base_url}")))?;
        // 30 s connect/read covers slow shared CMS hosts without
        // hanging the UI indefinitely.
        let inner = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(30))
            .timeout_write(Duration::from_secs(30))
            // Refuse redirects: the user typed a host, anything else
            // is a redirection attack on credentials/content.
            .redirects(0)
            .build();
        Ok(Self { inner, host })
    }

    /// The host this agent is pinned to. Callers use this to refuse
    /// requests that try to point at a different URL.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// POST a JSON body to a path on the pinned host. Refuses if the
    /// caller hands us a URL whose host doesn't match.
    pub fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<HttpResponse, PublishError> {
        self.guard(url)?;
        let mut req = self.inner.post(url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.send_json(body.clone()) {
            Ok(resp) => Ok(HttpResponse::from_ureq(resp)),
            Err(ureq::Error::Status(code, resp)) => Ok(HttpResponse {
                status: code,
                body: redact_credentials(&resp.into_string().unwrap_or_default()),
            }),
            Err(ureq::Error::Transport(t)) => Err(PublishError::Network(t.to_string())),
        }
    }

    /// GET a path on the pinned host (used for the "Test connection"
    /// flow).
    pub fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, PublishError> {
        self.guard(url)?;
        let mut req = self.inner.get(url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.call() {
            Ok(resp) => Ok(HttpResponse::from_ureq(resp)),
            Err(ureq::Error::Status(code, resp)) => Ok(HttpResponse {
                status: code,
                body: redact_credentials(&resp.into_string().unwrap_or_default()),
            }),
            Err(ureq::Error::Transport(t)) => Err(PublishError::Network(t.to_string())),
        }
    }

    fn guard(&self, url: &str) -> Result<(), PublishError> {
        let host = host_of(url).ok_or_else(|| {
            PublishError::InvalidConfig(format!("request URL has no host: {url}"))
        })?;
        if host != self.host {
            return Err(PublishError::CrossHost {
                found: host,
                expected: self.host.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    fn from_ureq(resp: ureq::Response) -> Self {
        let status = resp.status();
        let body = redact_credentials(&resp.into_string().unwrap_or_default());
        Self { status, body }
    }
}

/// Extract the ASCII-lowercased host (without port) from a URL string.
/// Public so the `no_egress` test can verify the same helper is used
/// in both clients.
///
/// `to_ascii_lowercase` rather than `to_lowercase` so Unicode confusables
/// (fullwidth `Ｅ`, etc.) don't get normalised into the ASCII form and
/// silently match a pinned host.
pub fn host_of(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    parsed.host_str().map(|h| h.to_ascii_lowercase())
}

/// Refuse URLs that aren't safe to publish to. Mirrors
/// `rehydrate-ocr::http::validate_remote_url` — see that function's
/// docs for the full rationale. The publish layer additionally
/// requires `https://` for *every* non-loopback target (a CMS Admin
/// API call over plaintext leaks credentials on any path hop), so
/// the rules collapse to:
///
/// - `https://` to a named public host: OK.
/// - `http://` or `https://` to a loopback literal: OK (only useful
///   for tests against a local mock CMS).
/// - Anything else: rejected.
pub fn validate_remote_url(url_str: &str) -> Result<(), PublishError> {
    let parsed = url::Url::parse(url_str)
        .map_err(|e| PublishError::InvalidConfig(format!("not a valid URL: {e}")))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(PublishError::UnsafeUrl(format!(
            "scheme must be http or https (got {scheme:?})"
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| PublishError::InvalidConfig(format!("URL has no host: {url_str}")))?;
    let host_lc = host.to_ascii_lowercase();
    let is_named_loopback =
        host_lc == "localhost" || host_lc.ends_with(".localhost") || host_lc == "ip6-localhost";

    let host_for_ip = host_lc
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&host_lc);
    if let Ok(ip) = host_for_ip.parse::<IpAddr>() {
        if ip.is_loopback() {
            return Ok(());
        }
        if is_disallowed_ip(&ip) {
            return Err(PublishError::UnsafeUrl(format!(
                "{ip} is a private / link-local / metadata address — \
                 point reHydrate at a public hostname instead"
            )));
        }
        if scheme == "http" {
            return Err(PublishError::UnsafeUrl(format!(
                "{ip} requires https:// — plaintext credentials are never OK"
            )));
        }
        return Ok(());
    }

    if scheme == "http" && !is_named_loopback {
        return Err(PublishError::UnsafeUrl(format!(
            "plain HTTP is only allowed for loopback. {host:?} must use https://"
        )));
    }
    Ok(())
}

fn is_disallowed_ip(ip: &IpAddr) -> bool {
    if ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_link_local() || v4.is_broadcast() || v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            let segs = v6.segments();
            let is_unique_local = (segs[0] & 0xfe00) == 0xfc00;
            let is_link_local = (segs[0] & 0xffc0) == 0xfe80;
            let is_ipv4_mapped = segs[0] == 0
                && segs[1] == 0
                && segs[2] == 0
                && segs[3] == 0
                && segs[4] == 0
                && segs[5] == 0xffff;
            is_unique_local || is_link_local || is_ipv4_mapped
        }
    }
}

/// Strip bearer / basic-auth tokens out of strings that might end up
/// in error bodies surfaced to the renderer. Some servers (Ghost
/// included, in error paths) echo the incoming `Authorization` header
/// back in JSON error responses; without redaction, those tokens would
/// land in the UI's recent-logs panel and on disk.
///
/// The redactor is intentionally aggressive — it errs on the side of
/// over-redacting. False positives in a debug string are far cheaper
/// than a leaked Admin API key.
pub fn redact_credentials(s: &str) -> String {
    // Two pattern classes, each with their own "where does the token
    // end" rule:
    //
    // - JSON-shaped header: `"Authorization":"<token>"`. The value runs
    //   until the next unescaped `"`. Used by every JSON error body
    //   that includes request headers.
    // - Bare scheme prefix: `Bearer <token>`, `Basic …`, `Ghost …`.
    //   These can appear inline anywhere — header logs, Rust Debug
    //   strings, error messages. The token runs until whitespace or
    //   one of `",}]` (so JSON-quoted forms also terminate cleanly).
    //
    // Both classes are case-insensitive in the prefix.
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(prefix_len) = match_json_authorization(&bytes[i..]) {
            // JSON shape `"Authorization":"..."`. Emit the matched
            // prefix verbatim, then consume to the next `"`.
            out.push_str(&s[i..i + prefix_len]);
            i += prefix_len;
            let mut j = i;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            if j > i {
                out.push_str("[REDACTED]");
                i = j;
            }
            continue;
        }
        if let Some(prefix_len) = match_scheme_prefix(&bytes[i..]) {
            out.push_str(&s[i..i + prefix_len]);
            i += prefix_len;
            let mut j = i;
            while j < bytes.len() {
                let c = bytes[j];
                if c == b' '
                    || c == b'\t'
                    || c == b'\n'
                    || c == b'\r'
                    || c == b'"'
                    || c == b','
                    || c == b'}'
                    || c == b']'
                {
                    break;
                }
                j += 1;
            }
            if j > i {
                out.push_str("[REDACTED]");
                i = j;
            }
            continue;
        }
        let ch_len = utf8_char_len(bytes[i]);
        out.push_str(&s[i..i + ch_len]);
        i += ch_len;
    }
    out
}

/// `"Authorization":"` or `"authorization":"` (case-insensitive
/// prefix; the `":"` separator may have arbitrary whitespace, which
/// we don't bother to support — servers don't actually emit that).
fn match_json_authorization(b: &[u8]) -> Option<usize> {
    let mut i = 0;
    if i >= b.len() || b[i] != b'"' {
        return None;
    }
    i += 1;
    let key = b"authorization";
    if b.len() < i + key.len() {
        return None;
    }
    for (k, &kb) in key.iter().enumerate() {
        if !b[i + k].eq_ignore_ascii_case(&kb) {
            return None;
        }
    }
    i += key.len();
    // Expect `":"` exactly.
    if b.len() < i + 3 {
        return None;
    }
    if &b[i..i + 3] != b"\":\"" {
        return None;
    }
    Some(i + 3)
}

/// Match a bare scheme prefix (`Bearer `, `Basic `, `Ghost `) at the
/// start of `b`, case-insensitively. Returns the consumed length
/// including the trailing space.
fn match_scheme_prefix(b: &[u8]) -> Option<usize> {
    for prefix in [b"Bearer " as &[u8], b"Basic ", b"Ghost "] {
        if b.len() >= prefix.len()
            && b[..prefix.len()]
                .iter()
                .zip(prefix.iter())
                .all(|(a, p)| a.eq_ignore_ascii_case(p))
        {
            return Some(prefix.len());
        }
    }
    None
}

fn utf8_char_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b < 0xc0 {
        // Continuation byte mid-sequence — shouldn't happen on a
        // valid &str boundary, but be safe.
        1
    } else if b < 0xe0 {
        2
    } else if b < 0xf0 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_extracts_ascii_lowercased_host() {
        assert_eq!(
            host_of("https://Example.COM/foo").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            host_of("HTTPS://blog.example.com:443/x").as_deref(),
            Some("blog.example.com")
        );
        assert_eq!(host_of("not-a-url"), None);
        assert_eq!(host_of(""), None);
    }

    #[test]
    fn agent_refuses_cross_host_request() {
        let agent = RestrictedAgent::for_base("https://blog.example.com").unwrap();
        let r = agent.get("https://evil.com/x", &[]);
        match r {
            Err(PublishError::CrossHost { .. }) => (),
            other => panic!("expected CrossHost, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_plain_http_to_public_named() {
        let err = validate_remote_url("http://blog.example.com").unwrap_err();
        assert!(matches!(err, PublishError::UnsafeUrl(_)), "{err:?}");
    }

    #[test]
    fn validate_rejects_private_ips() {
        for bad in [
            "https://10.0.0.5",
            "https://192.168.1.10",
            "https://172.16.0.1",
            "https://169.254.169.254",
            "https://[fc00::1]",
            "https://[fe80::1]",
            "https://0.0.0.0",
        ] {
            let r = validate_remote_url(bad);
            assert!(
                matches!(r, Err(PublishError::UnsafeUrl(_))),
                "{bad} → {r:?}"
            );
        }
    }

    #[test]
    fn validate_accepts_loopback_and_public_https() {
        assert!(validate_remote_url("https://blog.example.com").is_ok());
        assert!(validate_remote_url("http://localhost:8080").is_ok());
        assert!(validate_remote_url("http://127.0.0.1:8080").is_ok());
    }

    #[test]
    fn redact_strips_bearer_tokens() {
        let s = r#"{"error":"invalid","Authorization":"Bearer eyJhbGciOi.x.y"}"#;
        let r = redact_credentials(s);
        assert!(!r.contains("eyJhbGciOi"), "{r}");
        assert!(r.contains("[REDACTED]"), "{r}");
    }

    #[test]
    fn redact_strips_basic_auth() {
        let s = "Authorization: Basic dXNlcjpwYXNz\nother stuff";
        let r = redact_credentials(s);
        assert!(!r.contains("dXNlcjpwYXNz"), "{r}");
    }

    #[test]
    fn redact_strips_ghost_token() {
        let s = "Authorization: Ghost eyJhbGciOiJIUzI1NiIs.xx.yy";
        let r = redact_credentials(s);
        assert!(!r.contains("eyJhbGciOiJIUzI1NiIs"), "{r}");
    }
}
