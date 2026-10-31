//! Shared, I/O-free data model used by every crate in the workspace, including
//! the wasm UI build. Nothing in this crate shells out to a process or touches
//! the filesystem — that stays in `mkv-core` (native) and `mkv-volume` (native).

mod job;
mod media;
mod plan;
mod track;
mod volume;

pub use job::{JobId, JobOutcome, JobPhase, JobProgress};
pub use media::{FileAnalysis, InfoRow, InfoSection, MediaAnalysis, MediaGeneral, MediaTrack};
pub use plan::{MuxInput, MuxPlan, MuxTrack, RemuxPlan, TrackEdit};
pub use track::{is_image_codec, AttachmentInfo, ContainerInfo, ProbeResult, TrackFlags, TrackInfo, TrackType};
pub use volume::{DirEntry, DirListing, RootInfo};

use serde::{Deserialize, Serialize};

/// Wire-format error returned by the server API and consumed by the wasm UI.
/// Native-side crates use their own richer `thiserror` error types and convert
/// into this at the process/API boundary.
#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
