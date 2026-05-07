//! Sync engine. Phase 1 implements pull only.
//!
//! `plan_pull` enumerates the device and classifies every document. `execute`
//! consumes a plan and applies it, with a progress channel and cancellation.

pub mod error;
pub mod plan;
pub mod progress;

mod execute;

pub use error::{SyncError, SyncResult};
pub use execute::execute_pull;
pub use plan::{plan_pull, DocumentPlan, PlanItemStatus, PullPlan};
pub use progress::{Progress, ProgressEvent};
