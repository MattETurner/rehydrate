//! Publish layer: take an OCR transcript and POST it as a draft to
//! either Ghost (Admin API) or WordPress (REST API). The only crate
//! in the workspace allowed to make outbound HTTP, and only ever to
//! a host the user explicitly named.
//!
//! Privacy invariant: every request goes through `RestrictedAgent`,
//! which is constructed with the user-configured host and refuses to
//! follow redirects or open connections to anywhere else. The
//! `no_egress` integration test asserts this layout — anything that
//! drops the `RestrictedAgent` wrapper trips the static check.

mod ghost;
mod http;
mod wordpress;

pub use ghost::{GhostClient, GhostCredentials};
pub use http::{host_of, RestrictedAgent};
pub use wordpress::{WordpressClient, WordpressCredentials};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What's being posted: either a Ghost draft or a WordPress draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublishTarget {
    Ghost,
    Wordpress,
}

impl PublishTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            PublishTarget::Ghost => "ghost",
            PublishTarget::Wordpress => "wordpress",
        }
    }
}

/// A draft post in the canonical shape both backends consume. The
/// caller is responsible for converting Markdown → HTML before
/// constructing this — the publish layer just ships bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftPost {
    pub title: String,
    pub html: String,
    /// Optional tags. Both Ghost and WordPress treat unknown tags as
    /// "create on the fly" so it's safe to include freely.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// What the backend gave us back after creating the draft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishResult {
    /// Backend-supplied identifier for the new post.
    pub post_id: String,
    /// Best-effort URL the user can click to keep editing the draft.
    /// For Ghost this is `<base>/ghost/#/editor/post/<id>`; for
    /// WordPress it's the `link` field returned by the REST API,
    /// falling back to `<base>/wp-admin/post.php?action=edit&post=<id>`.
    pub edit_url: String,
    pub target: PublishTarget,
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("network: {0}")]
    Network(String),

    #[error("auth failed (HTTP {0})")]
    AuthFailed(u16),

    #[error("server rejected the post (HTTP {status}): {body}")]
    Rejected { status: u16, body: String },

    #[error("could not parse server response: {0}")]
    BadResponse(String),
}

pub type PublishResultT<T> = std::result::Result<T, PublishError>;

/// Trait implemented by both `GhostClient` and `WordpressClient` so
/// the IPC layer can call them through one interface.
///
/// Sync because `ureq` is sync; the Tauri command should call this
/// from `tauri::async_runtime::spawn_blocking`.
pub trait Publisher: Send + Sync {
    fn target(&self) -> PublishTarget;
    fn publish_draft(&self, post: &DraftPost) -> PublishResultT<PublishResult>;
    /// Cheap auth check — used for the "Test connection" button in
    /// the UI. Returns Ok(()) when the credentials and URL look
    /// usable.
    fn ping(&self) -> PublishResultT<()>;
}
