use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackType {
    Video,
    Audio,
    Subtitles,
}

/// The Matroska per-track boolean flags this tool reads and writes. All of
/// them map 1:1 onto `mkvmerge --<name>-flag` and `mkvpropedit flag-<name>`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackFlags {
    pub default: bool,
    pub forced: bool,
    /// "Original language" — the track is in the content's original language.
    pub original: bool,
    pub hearing_impaired: bool,
    /// Audio description for the visually impaired.
    pub visual_impaired: bool,
    pub commentary: bool,
}

/// A single track as reported by `mkvmerge -J`.
///
/// `id` is mkvmerge's own track numbering for the source file, used to build
/// `mkvmerge` command-line arguments (`-a/-d/-s`, `--track-order`, etc). `uid`
/// is the Matroska Track UID, which is stable regardless of track order and is
/// used to target `mkvpropedit --edit track:=<uid>` in the metadata-only fast
/// path, and to join mkvmerge's view of a track with MediaInfo's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackInfo {
    pub id: u64,
    pub uid: u64,
    pub track_type: TrackType,
    pub codec: String,
    pub language: Option<String>,
    pub name: Option<String>,
    pub flags: TrackFlags,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub title: Option<String>,
    pub duration_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub container: ContainerInfo,
    pub tracks: Vec<TrackInfo>,
}
