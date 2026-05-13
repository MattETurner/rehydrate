//! WordPress REST API client.
//!
//! Auth: HTTP Basic with the user's username and an Application
//! Password (WP 5.6+, generated under Users → Profile → Application
//! Passwords). HTTPS is required by the WP core.
//!
//! Draft creation: `POST /wp-json/wp/v2/posts` with body
//! `{"title","content","status":"draft"}` — WP accepts HTML in
//! `content` directly, so the OCR HTML body is what we ship.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::http::RestrictedAgent;
use crate::{DraftPost, PublishError, PublishResult, PublishResultT, PublishTarget, Publisher};

#[derive(Clone, Serialize, Deserialize)]
pub struct WordpressCredentials {
    /// Base URL of the WP site, e.g. `https://example.com`. The
    /// REST API path `/wp-json/wp/v2/...` is appended internally.
    pub base_url: String,
    pub username: String,
    pub application_password: String,
}

// Hand-rolled Debug that redacts `application_password`. The
// `derive(Debug)` path printed it verbatim — see the matching
// rationale on GhostCredentials. Username + base URL stay visible
// because they're useful for diagnosing connection problems and
// aren't themselves authentication material.
impl std::fmt::Debug for WordpressCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WordpressCredentials")
            .field("base_url", &self.base_url)
            .field("username", &self.username)
            .field("application_password", &"[REDACTED]")
            .finish()
    }
}

pub struct WordpressClient {
    creds: WordpressCredentials,
    agent: RestrictedAgent,
}

#[derive(Deserialize)]
struct WpPostResponse {
    id: u64,
    #[serde(default)]
    link: Option<String>,
}

impl WordpressClient {
    pub fn new(creds: WordpressCredentials) -> PublishResultT<Self> {
        let agent = RestrictedAgent::for_base(&creds.base_url)?;
        if creds.username.is_empty() || creds.application_password.is_empty() {
            return Err(PublishError::InvalidConfig(
                "WordPress username and application password are both required".into(),
            ));
        }
        Ok(Self { creds, agent })
    }

    fn auth_header(&self) -> String {
        let pair = format!(
            "{}:{}",
            self.creds.username,
            // WP application passwords are usually shown as
            // space-separated groups of four. The user might paste
            // them as-is; strip whitespace so the auth header is
            // clean.
            self.creds.application_password.replace(' ', "")
        );
        format!("Basic {}", B64.encode(pair.as_bytes()))
    }

    fn rest_url(&self, path: &str) -> String {
        let base = self.creds.base_url.trim_end_matches('/');
        format!("{base}/wp-json/wp/v2{path}")
    }

    fn admin_edit_url(&self, post_id: u64) -> String {
        let base = self.creds.base_url.trim_end_matches('/');
        format!("{base}/wp-admin/post.php?action=edit&post={post_id}")
    }
}

impl Publisher for WordpressClient {
    fn target(&self) -> PublishTarget {
        PublishTarget::Wordpress
    }

    fn publish_draft(&self, post: &DraftPost) -> PublishResultT<PublishResult> {
        let body = serde_json::json!({
            "title": post.title,
            "content": post.html,
            "status": "draft",
        });
        let url = self.rest_url("/posts");
        let auth = self.auth_header();
        let resp = self.agent.post_json(
            &url,
            &[
                ("Authorization", auth.as_str()),
                ("Content-Type", "application/json"),
            ],
            &body,
        )?;
        if resp.status == 401 || resp.status == 403 {
            return Err(PublishError::AuthFailed(resp.status));
        }
        if !(200..300).contains(&resp.status) {
            return Err(PublishError::Rejected {
                status: resp.status,
                body: resp.body,
            });
        }
        let parsed: WpPostResponse = serde_json::from_str(&resp.body)
            .map_err(|e| PublishError::BadResponse(e.to_string()))?;
        Ok(PublishResult {
            post_id: parsed.id.to_string(),
            edit_url: parsed
                .link
                .unwrap_or_else(|| self.admin_edit_url(parsed.id)),
            target: PublishTarget::Wordpress,
        })
    }

    fn ping(&self) -> PublishResultT<()> {
        // /wp-json/wp/v2/users/me requires auth and returns 200 if
        // the credentials are valid; 401/403 if not.
        let url = self.rest_url("/users/me");
        let auth = self.auth_header();
        let resp = self.agent.get(&url, &[("Authorization", auth.as_str())])?;
        if resp.status == 401 || resp.status == 403 {
            return Err(PublishError::AuthFailed(resp.status));
        }
        if !(200..300).contains(&resp.status) {
            return Err(PublishError::Rejected {
                status: resp.status,
                body: resp.body,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_application_password() {
        // Mirrors the GhostCredentials redaction test — same class
        // of bug, same regression guard.
        let creds = WordpressCredentials {
            base_url: "https://blog.example.com".into(),
            username: "alice".into(),
            application_password: "abcd efgh ijkl mnop".into(),
        };
        let s = format!("{creds:?}");
        assert!(
            !s.contains("ijkl mnop"),
            "Debug must NOT reveal the application password; got: {s}",
        );
        assert!(s.contains("REDACTED"));
        // Username + URL stay visible for diagnostics.
        assert!(s.contains("alice"));
        assert!(s.contains("blog.example.com"));
    }

    #[test]
    fn auth_header_strips_whitespace_in_app_password() {
        let c = WordpressClient::new(WordpressCredentials {
            base_url: "https://blog.example.com".into(),
            username: "alice".into(),
            application_password: "abcd efgh ijkl mnop".into(),
        })
        .unwrap();
        let header = c.auth_header();
        assert!(header.starts_with("Basic "));
        let decoded = B64.decode(header.trim_start_matches("Basic ")).unwrap();
        assert_eq!(decoded, b"alice:abcdefghijklmnop");
    }

    #[test]
    fn rejects_empty_creds() {
        let r = WordpressClient::new(WordpressCredentials {
            base_url: "https://blog.example.com".into(),
            username: "".into(),
            application_password: "abcd efgh".into(),
        });
        assert!(matches!(r, Err(PublishError::InvalidConfig(_))));
    }
}
