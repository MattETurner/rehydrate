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

use std::time::Duration;

/// Wraps `ureq::Agent` and pins it to one host. Constructing a new
/// `RestrictedAgent` is the only sanctioned way to obtain an agent
/// inside this crate.
pub struct RestrictedAgent {
    inner: ureq::Agent,
    host: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("request host {found:?} does not match agent host {expected:?}")]
    CrossHost { found: String, expected: String },
    #[error("network: {0}")]
    Network(String),
}

impl RestrictedAgent {
    /// Construct an agent pinned to the host of `base_url`. Returns
    /// an error if the URL doesn't parse or has no host. Default
    /// timeouts (connect/read/write) are 30 s each.
    pub fn for_base(base_url: &str) -> Result<Self, AgentError> {
        Self::for_base_with_timeout(base_url, Duration::from_secs(30))
    }

    /// As `for_base`, but lets callers override the per-stage timeout.
    /// Page transcription with the larger Qwen builds can take
    /// well over a minute per page on CPU — the OCR caller passes a
    /// longer read timeout for `/api/generate` calls than the
    /// shorter one used for `/api/tags` health probes.
    pub fn for_base_with_timeout(base_url: &str, timeout: Duration) -> Result<Self, AgentError> {
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

/// Extract the lowercased host (without port) from a URL string.
pub fn host_of(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    parsed.host_str().map(|h| h.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_extracts_lowercased_host() {
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
}
