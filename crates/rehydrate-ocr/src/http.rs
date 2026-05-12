//! Restricted ureq agent: every outbound request must target the
//! single host the user configured for the Ollama daemon (default
//! `127.0.0.1:11434`, but the URL is settable). Redirects are
//! disabled — a 30x to anywhere else would defeat the host pin.
//!
//! This file mirrors the one in `crates/rehydrate-publish/src/http.rs`
//! deliberately. Two-file duplication keeps the `no_egress.rs`
//! integration test sharp: the test allow-lists `rehydrate-ocr` and
//! `rehydrate-publish` as sanctioned egress crates, and asserts both
//! reach the network through a `RestrictedAgent`. A refactor that
//! collapses the duplicate into a shared crate is fine to do later,
//! but the test would need to learn about the new location at the
//! same time.

use std::net::IpAddr;
use std::time::Duration;

/// Wraps `ureq::Agent` and pins it to one host. Constructing a new
/// `RestrictedAgent` is the only sanctioned way to obtain an agent
/// inside this crate.
///
/// `Clone` is derived because `ureq::Agent` is internally
/// reference-counted — cloning the wrapper preserves the connection
/// pool, which matters when a single OCR run fires ~50 sequential
/// requests at the same Ollama host: without clone-and-reuse, each
/// page would tear down and re-establish the TCP connection.
#[derive(Clone)]
pub struct RestrictedAgent {
    inner: ureq::Agent,
    host: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("URL is unsafe to fetch: {0}")]
    UnsafeUrl(String),
    #[error("request host {found:?} does not match agent host {expected:?}")]
    CrossHost { found: String, expected: String },
    #[error("network: {0}")]
    Network(String),
}

impl RestrictedAgent {
    /// Construct an agent pinned to the host of `base_url`. Returns
    /// an error if the URL doesn't parse, has no host, or points at
    /// something we refuse to fetch (see [`validate_remote_url`]).
    /// Default timeouts (connect/read/write) are 30 s each.
    pub fn for_base(base_url: &str) -> Result<Self, AgentError> {
        Self::for_base_with_timeout(base_url, Duration::from_secs(30))
    }

    /// As `for_base`, but lets callers override the per-stage timeout.
    /// Page transcription with the larger Qwen builds can take
    /// well over a minute per page on CPU — the OCR caller passes a
    /// longer read timeout for `/api/generate` calls than the
    /// shorter one used for `/api/tags` health probes.
    pub fn for_base_with_timeout(base_url: &str, timeout: Duration) -> Result<Self, AgentError> {
        validate_remote_url(base_url)?;
        let host = host_of(base_url)
            .ok_or_else(|| AgentError::InvalidUrl(format!("URL has no host: {base_url}")))?;
        let inner = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(timeout)
            .timeout_write(timeout)
            .redirects(0)
            .build();
        Ok(Self { inner, host })
    }

    /// The host this agent is pinned to.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// POST a JSON body to a path on the pinned host.
    pub fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<HttpResponse, AgentError> {
        self.guard(url)?;
        match self.inner.post(url).send_json(body.clone()) {
            Ok(resp) => Ok(HttpResponse::from_ureq(resp)),
            Err(ureq::Error::Status(code, resp)) => Ok(HttpResponse {
                status: code,
                body: resp.into_string().unwrap_or_default(),
            }),
            Err(ureq::Error::Transport(t)) => Err(AgentError::Network(t.to_string())),
        }
    }

    /// GET a path on the pinned host.
    pub fn get(&self, url: &str) -> Result<HttpResponse, AgentError> {
        self.guard(url)?;
        match self.inner.get(url).call() {
            Ok(resp) => Ok(HttpResponse::from_ureq(resp)),
            Err(ureq::Error::Status(code, resp)) => Ok(HttpResponse {
                status: code,
                body: resp.into_string().unwrap_or_default(),
            }),
            Err(ureq::Error::Transport(t)) => Err(AgentError::Network(t.to_string())),
        }
    }

    fn guard(&self, url: &str) -> Result<(), AgentError> {
        let host = host_of(url)
            .ok_or_else(|| AgentError::InvalidUrl(format!("request URL has no host: {url}")))?;
        if host != self.host {
            return Err(AgentError::CrossHost {
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
        let body = resp.into_string().unwrap_or_default();
        Self { status, body }
    }
}

/// Extract the ASCII-lowercased host (without port) from a URL string.
///
/// `to_ascii_lowercase` rather than `to_lowercase` so Unicode confusables
/// like fullwidth-E in `Ｅxample.com` don't get silently normalized into
/// the ASCII form and bypass the host-pin equality check.
pub fn host_of(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    parsed.host_str().map(|h| h.to_ascii_lowercase())
}

/// Refuse URLs we should never fetch from OCR / publish flows.
///
/// Rules:
/// 1. Must parse, must have a host, scheme must be `http` or `https`.
/// 2. `http://` is only allowed when the host is loopback. The OCR
///    daemon's default is `http://127.0.0.1:11434`; anything else
///    crosses the network in plaintext and exposes page imagery /
///    transcripts to anyone on-path.
/// 3. If the host is a literal IP address, reject private /
///    link-local / loopback (for `https`) / unspecified / multicast
///    / cloud-metadata (`169.254.169.254`) ranges. A renderer XSS
///    that flipped the configured base URL to one of these could
///    exfiltrate to an internal host the user never intended; the
///    audit specifically flagged `https://169.254.169.254` and
///    `https://10.0.0.5` as previously-accepted SSRF targets.
///
/// Loopback over `http://` (the Ollama default) is allowed because
/// the daemon doesn't speak TLS and we control both endpoints.
///
/// LAN inference is supported via DNS hostname — give the box an
/// mDNS or DNS name and a TLS cert (or a reverse proxy that does)
/// and use `https://ollama.lan:11434`. Literal-IP `http://10.x.y.z`
/// is rejected on purpose.
pub fn validate_remote_url(url_str: &str) -> Result<(), AgentError> {
    let parsed = url::Url::parse(url_str)
        .map_err(|e| AgentError::InvalidUrl(format!("not a valid URL: {e}")))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(AgentError::UnsafeUrl(format!(
            "scheme must be http or https (got {scheme:?})"
        )));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AgentError::InvalidUrl(format!("URL has no host: {url_str}")))?;
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
        // Literal IP. Loopback is OK over http; everything else off
        // the public internet is rejected on both schemes.
        if ip.is_loopback() {
            return Ok(());
        }
        if is_disallowed_ip(&ip) {
            return Err(AgentError::UnsafeUrl(format!(
                "{ip} is a private / link-local / metadata address — \
                 point reHydrate at a public host or `127.0.0.1`"
            )));
        }
        // Public IP with http://: still refuse — there's no excuse
        // for shipping plaintext to the open internet.
        if scheme == "http" {
            return Err(AgentError::UnsafeUrl(format!(
                "{ip} requires https:// — plain HTTP is only allowed for loopback"
            )));
        }
        return Ok(());
    }

    // Named host.
    if scheme == "http" && !is_named_loopback {
        return Err(AgentError::UnsafeUrl(format!(
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
        // `Ipv6Addr::is_unique_local` is unstable on some MSRVs; check
        // the prefix manually. fc00::/7 = unique-local; fe80::/10 =
        // link-local; ::ffff:0:0/96 = IPv4-mapped (also unsafe — let
        // it through the IPv4 path by rejecting blanket here).
        IpAddr::V6(v6) => {
            let segs = v6.segments();
            let is_unique_local = (segs[0] & 0xfe00) == 0xfc00;
            let is_link_local = (segs[0] & 0xffc0) == 0xfe80;
            // ::ffff:a.b.c.d — IPv4-mapped. Reject; if the user wants
            // that address they should write the IPv4 form, which
            // routes through the v4 rules above.
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
    fn host_of_extracts_ascii_lowercased_host() {
        assert_eq!(
            host_of("http://Localhost:11434/").as_deref(),
            Some("localhost")
        );
        assert_eq!(
            host_of("https://OLLAMA.example.com").as_deref(),
            Some("ollama.example.com")
        );
        assert_eq!(host_of("not-a-url"), None);
        assert_eq!(host_of(""), None);
    }

    #[test]
    fn agent_refuses_cross_host_request() {
        let agent = RestrictedAgent::for_base("http://localhost:11434").unwrap();
        let r = agent.get("https://evil.com/x");
        match r {
            Err(AgentError::CrossHost { .. }) => (),
            other => panic!("expected CrossHost, got {other:?}"),
        }
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
        assert!(matches!(err, AgentError::UnsafeUrl(_)), "{err:?}");
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
            let err = validate_remote_url(bad)
                .unwrap_err_or_else_value(|| panic!("expected reject for {bad}"));
            assert!(matches!(err, AgentError::UnsafeUrl(_)), "{bad} → {err:?}");
        }
    }

    #[test]
    fn validate_rejects_weird_schemes() {
        for bad in ["file:///etc/passwd", "ftp://example.com", "gopher://x"] {
            let err = validate_remote_url(bad).unwrap_err();
            assert!(matches!(err, AgentError::UnsafeUrl(_)), "{bad} → {err:?}");
        }
    }

    // Small helper so the test reads naturally without a custom macro.
    trait ResultUnwrapErr<T, E> {
        fn unwrap_err_or_else_value<F: FnOnce() -> E>(self, f: F) -> E;
    }
    impl<T, E> ResultUnwrapErr<T, E> for Result<T, E> {
        fn unwrap_err_or_else_value<F: FnOnce() -> E>(self, f: F) -> E {
            match self {
                Ok(_) => f(),
                Err(e) => e,
            }
        }
    }
}
