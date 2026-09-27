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
    #[serde(default)]
    pub attachments: Vec<AttachmentInfo>,
}

/// A file attached to the container (cover art, fonts for styled
/// subtitles, …) as reported by `mkvmerge -J`. `id` is mkvmerge's number for
/// `--attachments`; `uid` is stable and targets `mkvpropedit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentInfo {
    pub id: u64,
    pub uid: u64,
    pub file_name: String,
    pub mime_type: String,
    pub size: u64,
}

impl AttachmentInfo {
    /// Cover art and other pictures. Fonts and everything else are not
    /// images: styled (ASS) subtitles need their fonts.
    pub fn is_image(&self) -> bool {
        self.mime_type.to_ascii_lowercase().starts_with("image/")
    }
}

impl TrackInfo {
    /// A still picture stored as a video track — e.g. ffmpeg writes cover art
    /// ("attached pic") as a one-frame `V_MJPEG` track.
    pub fn is_image(&self) -> bool {
        self.track_type == TrackType::Video && is_image_codec(&self.codec)
    }
}

/// Still-image codecs, as mkvmerge names them (`V_MJPEG`, `MJPEG`, `PNG`, …).
pub fn is_image_codec(codec: &str) -> bool {
    let codec = codec.to_ascii_lowercase();
    ["mjpeg", "jpeg", "jpg", "png", "bmp", "gif", "webp", "tiff"]
        .iter()
        .any(|image| codec.contains(image))
}
