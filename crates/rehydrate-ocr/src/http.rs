//! Compatibility re-exports of the shared host-pinned HTTP agent.
//!
//! Before the `rehydrate-http` extraction, this file held a copy of
//! `RestrictedAgent`/`HttpError`/`validate_remote_url` that drifted
//! from the parallel copy in `rehydrate-publish`. The audit flagged
//! the duplication; the agent now lives in a single crate and the
//! two consumers re-export it under names familiar to existing
//! callers (`AgentError` in particular — that name was used in
//! `OllamaBackend`).
//!
//! The `tests-integration::no_egress` test asserts that the only
//! place in the workspace constructing a bare ureq agent is the
//! shared crate; this thin re-export keeps that invariant intact
//! while the surface remains stable for `rehydrate-ocr` consumers.

pub use rehydrate_http::{
    host_of, redact_credentials, validate_remote_url, HttpError as AgentError, HttpResponse,
    RestrictedAgent,
};
