//! MediaInfo-derived facts about a file. These are raw-ish MediaInfo field
//! values (not our labels): turning them into "DV8 HDR10" / "DD+ 5.1 Atmos"
//! is `mkv-profiles`' job, so that logic is pure and runs in the browser too.

use crate::{ProbeResult, TrackType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaGeneral {
    pub format: Option<String>,
    pub title: Option<String>,
    pub duration_seconds: Option<f64>,
    pub file_size: Option<u64>,
    pub overall_bit_rate: Option<u64>,
}

/// One track's MediaInfo fields, keyed by Matroska Track UID (MediaInfo's
/// `UniqueID`), which is how it's joined with mkvmerge's `TrackInfo`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaTrack {
    pub uid: u64,
    pub kind: Option<TrackType>,
    /// e.g. "HEVC", "E-AC-3", "MLP FBA", "DTS", "UTF-8", "PGS".
    pub format: Option<String>,
    /// e.g. "Dolby Digital Plus with Dolby Atmos", "DTS-HD Master Audio".
    pub format_commercial: Option<String>,
    /// e.g. "Main 10", "Layer 3", "MA / Core".
    pub format_profile: Option<String>,
    /// e.g. "JOC", "XLL X", "XBR", "ES", "16-ch".
    pub format_additional_features: Option<String>,
    pub codec_id: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,

    // Video
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bit_depth: Option<u32>,
    pub frame_rate: Option<f64>,
    /// e.g. "Dolby Vision / SMPTE ST 2086", "SMPTE ST 2094 App 4 / SMPTE ST 2086".
    pub hdr_format: Option<String>,
    /// e.g. "dvhe.08.06 / ".
    pub hdr_format_profile: Option<String>,
    /// e.g. "HDR10 / HDR10", "HDR10+ Profile A / HDR10", "Blu-ray / HDR10".
    pub hdr_format_compatibility: Option<String>,
    /// e.g. "PQ", "HLG", "BT.709".
    pub transfer_characteristics: Option<String>,
    pub has_mastering_display: bool,
    /// >1 means stereoscopic 3D.
    pub multiview_count: Option<u32>,
    /// e.g. "x264", "x265".
    pub encoded_library_name: Option<String>,

    // Audio
    pub channels: Option<u32>,
    /// e.g. "L R C LFE Ls Rs".
    pub channel_layout: Option<String>,
    pub sampling_rate: Option<u32>,

    // Text
    pub element_count: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaAnalysis {
    pub general: MediaGeneral,
    pub tracks: Vec<MediaTrack>,
}

impl MediaAnalysis {
    pub fn track(&self, uid: u64) -> Option<&MediaTrack> {
        self.tracks.iter().find(|t| t.uid == uid)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InfoRow {
    pub key: String,
    pub value: String,
}

/// A human-readable section of MediaInfo's report ("General", "Video",
/// "Audio #2", ...), for the structured media-info viewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InfoSection {
    pub title: String,
    pub rows: Vec<InfoRow>,
}

/// Everything the UI needs about one opened file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileAnalysis {
    /// Just the file name (no directories) — input for the release-name parser.
    pub file_name: String,
    pub probe: ProbeResult,
    pub media: MediaAnalysis,
    pub sections: Vec<InfoSection>,
}
