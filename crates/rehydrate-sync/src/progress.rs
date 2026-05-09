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
