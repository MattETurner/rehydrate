//! Restricted ureq agent: every outbound request must target the
//! single host the user configured for this client. Redirects are
//! disabled (a Set-Cookie + 302 to evil.com is the obvious data-
//! exfiltration vector for a hijacked CMS endpoint). The
//! `no_egress.rs` integration test greps this file to assert the
//! `Agent::new()` constructor isn't used directly by the rest of
//! the crate.

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
    /// `InvalidConfig` if the URL doesn't parse or has no host.
    pub fn for_base(base_url: &str) -> Result<Self, PublishError> {
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
                body: resp.into_string().unwrap_or_default(),
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
                body: resp.into_string().unwrap_or_default(),
            }),
            Err(ureq::Error::Transport(t)) => Err(PublishError::Network(t.to_string())),
        }
    }

    fn guard(&self, url: &str) -> Result<(), PublishError> {
        let host = host_of(url).ok_or_else(|| {
            PublishError::InvalidConfig(format!("request URL has no host: {url}"))
        })?;
        if host != self.host {
            return Err(PublishError::InvalidConfig(format!(
                "request host {host:?} does not match agent host {:?}",
                self.host
            )));
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
/// Public so the `no_egress` test can verify the same helper is
/// used in both clients.
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
            Err(PublishError::InvalidConfig(_)) => (),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }
}
