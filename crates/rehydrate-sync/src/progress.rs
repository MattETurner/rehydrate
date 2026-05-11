use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProgressEvent {
    PlanReady {
        total_documents: usize,
    },
    DocumentStarted {
        document_id: String,
        visible_name: String,
    },
    FileFetched {
        document_id: String,
        file: String,
        bytes: u64,
        deduped: bool,
    },
    DocumentCompleted {
        document_id: String,
        unchanged: bool,
    },
    DocumentSkipped {
        document_id: String,
        reason: String,
    },
    /// Non-fatal warning the UI should surface (e.g. a push wrote
    /// all files successfully but the tablet's xochitl restart
    /// failed, so the user must reboot to see the changes).
    Warning {
        message: String,
    },
    Done {
        recorded: usize,
        unchanged: usize,
        skipped: usize,
    },
    Cancelled,
}

pub type Progress = mpsc::Sender<ProgressEvent>;

pub fn channel(buffer: usize) -> (mpsc::Sender<ProgressEvent>, mpsc::Receiver<ProgressEvent>) {
    mpsc::channel(buffer)
}
