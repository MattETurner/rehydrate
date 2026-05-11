//! Ghost Admin API client.
//!
//! Auth: Admin API key shaped as `<id>:<hex_secret>`. We sign a
//! short-lived (≤5 min) HS256 JWT with `kid = id`, payload
//! `{iat, exp, aud:"/admin/"}`, secret = hex-decoded second half.
//!
//! Draft creation: `POST /ghost/api/admin/posts/?source=html` with
//! body `{"posts":[{"title","html","status":"draft","tags":[…]}]}`.
//! `?source=html` makes Ghost convert HTML→Lexical server-side, so
//! we don't need a client-side mobiledoc/lexical builder.

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::http::RestrictedAgent;
use crate::{DraftPost, PublishError, PublishResult, PublishResultT, PublishTarget, Publisher};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhostCredentials {
    /// Base URL of the Ghost site, e.g. `https://blog.example.com`.
    /// No trailing `/ghost/api/...` path — we append it.
    pub base_url: String,
    /// `<id>:<hex_secret>` Admin API key from Settings → Integrations.
    pub admin_api_key: String,
}

pub struct GhostClient {
    creds: GhostCredentials,
    agent: RestrictedAgent,
}

#[derive(Serialize)]
struct JwtClaims<'a> {
    iat: i64,
    exp: i64,
    aud: &'a str,
}

#[derive(Deserialize)]
struct PostsEnvelope {
    posts: Vec<PostResponse>,
}

#[derive(Deserialize)]
struct PostResponse {
    id: String,
    #[serde(default)]
    url: Option<String>,
}

impl GhostClient {
    pub fn new(creds: GhostCredentials) -> PublishResultT<Self> {
        let agent = RestrictedAgent::for_base(&creds.base_url)?;
        // Validate the key shape up-front so the error surfaces at
        // configuration time, not the first POST.
        Self::split_key(&creds.admin_api_key)?;
        Ok(Self { creds, agent })
    }

    fn split_key(key: &str) -> PublishResultT<(&str, Vec<u8>)> {
        let (id, hex_secret) = key.split_once(':').ok_or_else(|| {
            PublishError::InvalidConfig(
                "Ghost Admin API key must be in <id>:<hex_secret> form".into(),
            )
        })?;
        let secret = hex::decode(hex_secret).map_err(|_| {
            PublishError::InvalidConfig("Ghost Admin API key secret is not hex".into())
        })?;
        Ok((id, secret))
    }

    fn sign_jwt(&self) -> PublishResultT<String> {
        let (id, secret) = Self::split_key(&self.creds.admin_api_key)?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let claims = JwtClaims {
            iat: now,
            // Ghost rejects tokens with exp > iat + 5min.
            exp: now + 4 * 60,
            aud: "/admin/",
        };
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some(id.to_string());
        encode(&header, &claims, &EncodingKey::from_secret(&secret))
            .map_err(|e| PublishError::InvalidConfig(format!("JWT sign: {e}")))
    }

    fn admin_url(&self, path: &str) -> String {
        // Trim trailing slash on base then append the canonical
        // admin path. Ghost's API lives at `/ghost/api/admin/...`.
        let base = self.creds.base_url.trim_end_matches('/');
        format!("{base}/ghost/api/admin{path}")
    }

    fn editor_url(&self, post_id: &str) -> String {
        let base = self.creds.base_url.trim_end_matches('/');
        format!("{base}/ghost/#/editor/post/{post_id}")
    }
}

impl Publisher for GhostClient {
    fn target(&self) -> PublishTarget {
        PublishTarget::Ghost
    }

    fn publish_draft(&self, post: &DraftPost) -> PublishResultT<PublishResult> {
        let token = self.sign_jwt()?;
        let auth = format!("Ghost {token}");

        let body = serde_json::json!({
            "posts": [{
                "title": post.title,
                "html": post.html,
                "status": "draft",
                "tags": post.tags,
            }]
        });

        let url = format!("{}/posts/?source=html", self.admin_url(""));
        let resp = self.agent.post_json(
            &url,
            &[
                ("Authorization", auth.as_str()),
                ("Content-Type", "application/json"),
                ("Accept-Version", "v5.0"),
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
        let env: PostsEnvelope = serde_json::from_str(&resp.body)
            .map_err(|e| PublishError::BadResponse(e.to_string()))?;
        let post = env
            .posts
            .into_iter()
            .next()
            .ok_or_else(|| PublishError::BadResponse("Ghost returned no posts".into()))?;
        Ok(PublishResult {
            edit_url: post.url.unwrap_or_else(|| self.editor_url(&post.id)),
            post_id: post.id,
            target: PublishTarget::Ghost,
        })
    }

    fn ping(&self) -> PublishResultT<()> {
        let token = self.sign_jwt()?;
        let auth = format!("Ghost {token}");
        let url = format!("{}/site/", self.admin_url(""));
        let resp = self.agent.get(
            &url,
            &[("Authorization", auth.as_str()), ("Accept-Version", "v5.0")],
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_key() {
        let r = GhostClient::new(GhostCredentials {
            base_url: "https://blog.example.com".into(),
            admin_api_key: "no-colon-here".into(),
        });
        assert!(matches!(r, Err(PublishError::InvalidConfig(_))));
    }

    #[test]
    fn rejects_non_hex_secret() {
        let r = GhostClient::new(GhostCredentials {
            base_url: "https://blog.example.com".into(),
            admin_api_key: "id:not-hex-😀".into(),
        });
        assert!(matches!(r, Err(PublishError::InvalidConfig(_))));
    }

    #[test]
    fn jwt_signing_round_trips() {
        // 32-byte key = 64 hex chars, but Ghost uses 26-byte keys
        // (52 hex chars) — accept whatever decodes from hex.
        let client = GhostClient::new(GhostCredentials {
            base_url: "https://blog.example.com".into(),
            admin_api_key: "abc123:0011223344556677889900aabbccddeeff".into(),
        })
        .unwrap();
        let token = client.sign_jwt().unwrap();
        // JWT shape: three dot-separated segments.
        assert_eq!(token.matches('.').count(), 2);
    }
}
