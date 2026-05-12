//! Host-pinned, redirect-refusing HTTP agent for reHydrate's two
//! sanctioned outbound paths (`rehydrate-ocr` → Ollama,
//! `rehydrate-publish` → Ghost/WordPress).
//!
//! Both downstream crates re-export the types defined here. Keeping
//! one implementation eliminates the drift that the audit flagged —
//! the previous file-for-file duplication had already started
//! diverging on timeout APIs.
//!
//! Security invariants enforced by this crate:
//!
//! 1. **Host pinning.** Every agent is constructed for exactly one
//!    host. Cross-host requests (whether from a redirect or a
//!    misconfigured caller) return [`HttpError::CrossHost`].
//! 2. **No redirects.** Set-Cookie + 302 to a different host is the
//!    obvious data-exfiltration path; `redirects(0)` shuts it.
//! 3. **URL safety.** [`validate_remote_url`] rejects literal
//!    private/link-local/loopback (for HTTPS to anywhere but
//!    loopback)/multicast/metadata IPs and any scheme other than
//!    `http`/`https`. The OCR and publish layers both call this
//!    inside `RestrictedAgent::for_base`, so the IPC probe and the
//!    production fetch always see the same answer.
//! 4. **Credential redaction.** Response bodies are scrubbed of
//!    `Authorization` / `Bearer` / `Basic` / `Ghost` shapes before
//!    they're handed back to the caller — some servers reflect
//!    headers in error JSON, and we don't want them leaking into
//!    UI logs.
//!
//! The `tests-integration::no_egress` test grep-asserts that this
//! crate is the only place `ureq::AgentBuilder::new()` is called
//! from inside the workspace. Bypassing the wrapper would be visible
//! in code review and would fail CI.

use std::net::IpAddr;
use std::time::Duration;

mod redact;
pub use redact::redact_credentials;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("URL is unsafe to fetch: {0}")]
    UnsafeUrl(String),

    #[error("request host {found:?} does not match agent host {expected:?}")]
    CrossHost { found: String, expected: String },

    #[error("network: {0}")]
    Network(String),
}

/// Wraps `ureq::Agent` and pins every request to a single host.
///
/// `Clone` is intentional: `ureq::Agent` is internally reference-
/// counted, so cloning the wrapper preserves the connection pool.
/// The OCR path cares — a 50-page transcription fires 50 sequential
/// requests against the same Ollama host, and without clone-and-
/// reuse each page would tear down and re-establish the TCP
/// connection.
#[derive(Clone)]
pub struct RestrictedAgent {
    inner: ureq::Agent,
    host: String,
}

impl RestrictedAgent {
    /// Construct an agent pinned to the host of `base_url` with
    /// default per-stage timeouts (30s connect/read/write).
    pub fn for_base(base_url: &str) -> Result<Self, HttpError> {
        Self::for_base_with_timeout(base_url, Duration::from_secs(30))
    }

    /// As [`Self::for_base`], but lets callers override the
    /// per-stage timeout. Page transcription with the larger Qwen
    /// builds can take over a minute per page on CPU, so the OCR
    /// caller passes a longer read timeout for `/api/generate` than
    /// for the shorter `/api/tags` probe.
    pub fn for_base_with_timeout(base_url: &str, timeout: Duration) -> Result<Self, HttpError> {
        validate_remote_url(base_url)?;
        let host = host_of(base_url)
            .ok_or_else(|| HttpError::InvalidUrl(format!("URL has no host: {base_url}")))?;
        let inner = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(timeout)
            .timeout_write(timeout)
            .redirects(0)
            .build();
        Ok(Self { inner, host })
    }

    /// The host this agent is pinned to. Callers use this to refuse
    /// requests that try to point at a different URL.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// POST a JSON body. Refuses cross-host targets.
    pub fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<HttpResponse, HttpError> {
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
            Err(ureq::Error::Transport(t)) => Err(HttpError::Network(t.to_string())),
        }
    }

    /// POST a JSON body and stream the response line by line through
    /// `on_line`. Designed for NDJSON / line-delimited-JSON endpoints
    /// like Ollama's `/api/generate?stream=true`, where each line is a
    /// generation step and the connection stays open until the model
    /// finishes.
    ///
    /// Why this exists instead of `post_json` returning bytes: with
    /// `stream: false`, Ollama buffers the entire response before
    /// sending it. On a 9B vision model running on CPU, that buffer
    /// can take 10+ minutes; ureq's per-read timeout fires before
    /// any byte arrives and the request fails with
    /// `Error encountered in the status line: timed out reading
    /// response`. Streaming flips the timeout's reference frame
    /// for the *generation* phase: the per-read cap covers
    /// "no token in N seconds" once tokens start flowing.
    ///
    /// Note that the daemon still buffers headers across its
    /// `prompt_eval` (image tokenisation) phase — `stream: true`
    /// is not "headers immediately". For vision OCR the caller's
    /// `timeout_read` must be sized to cover prompt_eval, not
    /// just the inter-token gap; the OCR backend's
    /// `GENERATE_TIMEOUT` is 600s for that reason.
    ///
    /// `on_line` is called for every non-empty line. Return `Ok(())`
    /// to continue, or `Err(HttpError)` to abort the read early
    /// (the caller's error is propagated up).
    pub fn post_json_streaming(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
        mut on_line: impl FnMut(&str) -> Result<(), HttpError>,
    ) -> Result<u16, HttpError> {
        use std::io::{BufRead, BufReader};

        self.guard(url)?;
        let mut req = self.inner.post(url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        let resp = match req.send_json(body.clone()) {
            Ok(r) => r,
            // Non-2xx status — return the status + buffered body
            // unchanged via the line callback's first call (which
            // also lets the caller surface the body to the user).
            Err(ureq::Error::Status(code, r)) => {
                let body_str = redact_credentials(&r.into_string().unwrap_or_default());
                if !body_str.is_empty() {
                    on_line(&body_str)?;
                }
                return Ok(code);
            }
            Err(ureq::Error::Transport(t)) => return Err(HttpError::Network(t.to_string())),
        };
        let status = resp.status();
        let reader = BufReader::new(resp.into_reader());
        for line in reader.lines() {
            let line = line.map_err(|e| HttpError::Network(e.to_string()))?;
            if line.is_empty() {
                continue;
            }
            on_line(&line)?;
        }
        Ok(status)
    }

    /// GET — same host-pin rules.
    pub fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse, HttpError> {
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
            Err(ureq::Error::Transport(t)) => Err(HttpError::Network(t.to_string())),
        }
    }

    fn guard(&self, url: &str) -> Result<(), HttpError> {
        let host = host_of(url)
            .ok_or_else(|| HttpError::InvalidUrl(format!("request URL has no host: {url}")))?;
        if host != self.host {
            return Err(HttpError::CrossHost {
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
///
/// `to_ascii_lowercase` rather than `to_lowercase` so Unicode
/// confusables (fullwidth `Ｅ`, etc.) can't get normalised into the
/// ASCII form and silently match a pinned host.
pub fn host_of(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    parsed.host_str().map(|h| h.to_ascii_lowercase())
}

/// Refuse URLs we should never fetch from sanctioned-egress flows.
///
/// Rules:
/// 1. Must parse, must have a host, scheme must be `http` or `https`.
/// 2. `http://` is only allowed when the host is loopback. Anything
///    else crosses the network in plaintext.
/// 3. If the host is a literal IP address, reject private /
///    link-local / loopback (for `https`) / unspecified /
///    multicast / cloud-metadata (`169.254.169.254`) ranges. A
///    renderer XSS that flipped the configured base URL to one of
///    these could exfiltrate to an internal host the user never
///    intended.
///
/// LAN inference is supported via DNS hostname — give the box an
/// mDNS or DNS name and a TLS cert (or a reverse proxy that does)
/// and use `https://ollama.lan:11434`. Literal-IP `http://10.x.y.z`
/// is rejected on purpose.
pub fn validate_remote_url(url_str: &str) -> Result<(), HttpError> {
    let parsed = url::Url::parse(url_str)
        .map_err(|e| HttpError::InvalidUrl(format!("not a valid URL: {e}")))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(HttpError::UnsafeUrl(format!(
            "scheme must be http or https (got {scheme:?})"
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| HttpError::InvalidUrl(format!("URL has no host: {url_str}")))?;
    let host_lc = host.to_ascii_lowercase();
    let is_named_loopback =
        host_lc == "localhost" || host_lc.ends_with(".localhost") || host_lc == "ip6-localhost";

    // IPv6 hosts come out of `host_str()` in bracketed form on some
    // versions of `url`; strip them so `[::1]` parses as an `IpAddr`.
    let host_for_ip = host_lc
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&host_lc);
    if let Ok(ip) = host_for_ip.parse::<IpAddr>() {
        if ip.is_loopback() {
            return Ok(());
        }
        if is_disallowed_ip(&ip) {
            return Err(HttpError::UnsafeUrl(format!(
                "{ip} is a private / link-local / metadata address — \
                 point reHydrate at a public host or `127.0.0.1`"
            )));
        }
        if scheme == "http" {
            return Err(HttpError::UnsafeUrl(format!(
                "{ip} requires https:// — plain HTTP is only allowed for loopback"
            )));
        }
        return Ok(());
    }

    if scheme == "http" && !is_named_loopback {
        return Err(HttpError::UnsafeUrl(format!(
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
            // `Ipv6Addr::is_unique_local` is unstable on some MSRVs;
            // check the prefix manually. fc00::/7 = unique-local;
            // fe80::/10 = link-local; ::ffff:0:0/96 = IPv4-mapped
            // (reject; if the user wants that v4 address they should
            // write it as IPv4 so the v4 rules above apply).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_ascii_lowercases() {
        assert_eq!(
            host_of("http://Localhost:11434/").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            host_of("https://Example.COM/foo").as_deref(),
            Some("example.com")
        );
        assert_eq!(host_of("not-a-url"), None);
        assert_eq!(host_of(""), None);
    }

    #[test]
    fn validate_accepts_loopback_and_public_https() {
        assert!(validate_remote_url("http://127.0.0.1:11434").is_ok());
        assert!(validate_remote_url("http://localhost:11434").is_ok());
        assert!(validate_remote_url("http://[::1]:11434").is_ok());
        assert!(validate_remote_url("https://ollama.example.com").is_ok());
        assert!(validate_remote_url("https://blog.example.com").is_ok());
    }

    #[test]
    fn validate_rejects_plain_http_to_lan() {
        let err = validate_remote_url("http://192.168.1.10:11434").unwrap_err();
        assert!(matches!(err, HttpError::UnsafeUrl(_)), "{err:?}");
    }

    #[test]
    fn validate_rejects_https_to_private_ip() {
        for bad in [
            "https://10.0.0.5",
            "https://192.168.1.10",
            "https://172.16.0.1",
            "https://169.254.169.254",
            "https://[fe80::1]",
            "https://[fc00::1]",
            "https://0.0.0.0",
            "https://224.0.0.1",
        ] {
            let r = validate_remote_url(bad);
            assert!(matches!(r, Err(HttpError::UnsafeUrl(_))), "{bad} → {r:?}");
        }
    }

    #[test]
    fn validate_rejects_weird_schemes() {
        for bad in ["file:///etc/passwd", "ftp://example.com", "gopher://x"] {
            let err = validate_remote_url(bad).unwrap_err();
            assert!(matches!(err, HttpError::UnsafeUrl(_)), "{bad} → {err:?}");
        }
    }

    #[test]
    fn agent_refuses_cross_host_request() {
        let agent = RestrictedAgent::for_base("http://localhost:11434").unwrap();
        let r = agent.get("https://evil.com/x", &[]);
        match r {
            Err(HttpError::CrossHost { .. }) => (),
            other => panic!("expected CrossHost, got {other:?}"),
        }
    }
}
