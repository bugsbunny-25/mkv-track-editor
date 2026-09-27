use crate::{AttachmentInfo, TrackFlags, TrackType};
use serde::{Deserialize, Serialize};

/// Desired final state for one kept track. Every kept track carries one of
/// these — even fields the user didn't touch are round-tripped from the probe
/// result, so command construction never has to distinguish "unchanged" from
/// "explicitly set to the same value". `track_type` is carried here (rather
/// than looked up from the original probe) so a `RemuxPlan` is self-contained
/// enough to build an `mkvmerge` command from directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackEdit {
    pub id: u64,
    pub uid: u64,
    pub track_type: TrackType,
    pub name: Option<String>,
    pub language: Option<String>,
    pub flags: TrackFlags,
}

/// A complete description of the desired output: which tracks to keep and in
/// what order, plus the final metadata for each. Tracks from the source that
/// are absent from `kept_track_order` are dropped, as are the attachments in
/// `remove_attachments`; every other attachment is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemuxPlan {
    pub source_path: String,
    pub output_path: String,
    pub kept_track_order: Vec<u64>,
    pub edits: Vec<TrackEdit>,
    #[serde(default)]
    pub remove_attachments: Vec<AttachmentInfo>,
}
