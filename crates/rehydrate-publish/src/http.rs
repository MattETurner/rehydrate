//! Compatibility re-exports of the shared host-pinned HTTP agent
//! plus the `HttpError → PublishError` conversion the publish-layer
//! code expects.
//!
//! Before the `rehydrate-http` extraction this file held a copy of
//! the agent that drifted from the parallel copy in `rehydrate-ocr`.
//! The shared crate is now the single source of truth.

use rehydrate_http::HttpError;

#[allow(unused_imports)]
pub use rehydrate_http::{
    host_of, redact_credentials, validate_remote_url, HttpResponse, RestrictedAgent,
};

use crate::PublishError;

impl From<HttpError> for PublishError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::InvalidUrl(msg) => PublishError::InvalidConfig(msg),
            HttpError::UnsafeUrl(msg) => PublishError::UnsafeUrl(msg),
            HttpError::CrossHost { found, expected } => PublishError::CrossHost { found, expected },
            HttpError::Network(msg) => PublishError::Network(msg),
        }
    }
}
