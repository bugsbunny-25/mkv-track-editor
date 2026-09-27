use crate::error::CoreError;
use crate::process::ProcessRunner;
use mkv_types::{AttachmentInfo, ContainerInfo, ProbeResult, TrackFlags, TrackInfo, TrackType};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct RawIdentify {
    #[serde(default)]
    container: RawContainer,
    #[serde(default)]
    tracks: Vec<RawTrack>,
    #[serde(default)]
    attachments: Vec<RawAttachment>,
}

#[derive(Debug, Deserialize)]
struct RawAttachment {
    id: u64,
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    properties: RawAttachmentProperties,
}

#[derive(Debug, Deserialize, Default)]
struct RawAttachmentProperties {
    uid: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct RawContainer {
    #[serde(default)]
    properties: RawContainerProperties,
}

#[derive(Debug, Deserialize, Default)]
struct RawContainerProperties {
    title: Option<String>,
    duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawTrack {
    id: u64,
    #[serde(rename = "type")]
    track_type: String,
    codec: String,
    #[serde(default)]
    properties: RawTrackProperties,
}

#[derive(Debug, Deserialize, Default)]
struct RawTrackProperties {
    uid: Option<u64>,
    track_name: Option<String>,
    language: Option<String>,
    language_ietf: Option<String>,
    #[serde(default)]
    default_track: bool,
    #[serde(default)]
    forced_track: bool,
    // mkvmerge omits these entirely when false.
    #[serde(default)]
    flag_original: bool,
    #[serde(default)]
    flag_hearing_impaired: bool,
    #[serde(default)]
    flag_visual_impaired: bool,
    #[serde(default)]
    flag_commentary: bool,
}

/// Parses the raw JSON from `mkvmerge -J` into our track model. Track types
/// outside video/audio/subtitles (e.g. DVD "buttons" tracks) are outside this
/// tool's scope and are skipped rather than causing the whole probe to fail.
pub(crate) fn parse_identify_json(raw_json: &str) -> Result<ProbeResult, CoreError> {
    let raw: RawIdentify = serde_json::from_str(raw_json)?;

    let tracks = raw
        .tracks
        .into_iter()
        .filter_map(|t| {
            let track_type = match t.track_type.as_str() {
                "video" => TrackType::Video,
                "audio" => TrackType::Audio,
                "subtitles" => TrackType::Subtitles,
                other => {
                    tracing::warn!(track_id = t.id, kind = other, "skipping unsupported track type");
                    return None;
                }
            };
            let uid = match t.properties.uid {
                Some(uid) => uid,
                None => {
                    tracing::warn!(track_id = t.id, "track has no Matroska UID, skipping");
                    return None;
                }
            };
            Some(TrackInfo {
                id: t.id,
                uid,
                track_type,
                codec: t.codec,
                language: t.properties.language_ietf.or(t.properties.language),
                name: t.properties.track_name,
                flags: TrackFlags {
                    default: t.properties.default_track,
                    forced: t.properties.forced_track,
                    original: t.properties.flag_original,
                    hearing_impaired: t.properties.flag_hearing_impaired,
                    visual_impaired: t.properties.flag_visual_impaired,
                    commentary: t.properties.flag_commentary,
                },
            })
        })
        .collect();

    let attachments = raw
        .attachments
        .into_iter()
        .filter_map(|a| {
            let Some(uid) = a.properties.uid else {
                tracing::warn!(attachment_id = a.id, "attachment has no UID, skipping");
                return None;
            };
            Some(AttachmentInfo {
                id: a.id,
                uid,
                file_name: a.file_name,
                mime_type: a.content_type,
                size: a.size,
            })
        })
        .collect();

    Ok(ProbeResult {
        container: ContainerInfo {
            title: raw.container.properties.title,
            duration_ns: raw.container.properties.duration,
        },
        tracks,
        attachments,
    })
}

/// Probes `path` with `mkvmerge -J` via the given runner.
pub async fn probe(runner: &dyn ProcessRunner, path: &Path) -> Result<ProbeResult, CoreError> {
    let stdout = runner.identify(path).await?;
    parse_identify_json(&stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"
    {
      "container": { "properties": { "title": "Sample", "duration": 123456789 } },
      "tracks": [
        {
          "id": 0, "type": "video", "codec": "AVC/H.264",
          "properties": { "uid": 111, "number": 1, "default_track": true, "forced_track": false }
        },
        {
          "id": 1, "type": "audio", "codec": "AAC",
          "properties": { "uid": 222, "number": 2, "track_name": "English", "language_ietf": "en", "default_track": true, "forced_track": false, "flag_original": true, "flag_commentary": true, "flag_visual_impaired": true }
        },
        {
          "id": 2, "type": "subtitles", "codec": "SubRip/SRT",
          "properties": { "uid": 333, "number": 3, "language": "fre", "default_track": false, "forced_track": true, "flag_hearing_impaired": true }
        },
        {
          "id": 3, "type": "buttons", "codec": "VobBtn",
          "properties": { "uid": 444, "number": 4 }
        }
      ],
      "attachments": [
        { "id": 1, "file_name": "cover.jpg", "content_type": "image/jpeg", "size": 477, "properties": { "uid": 17232074509077980272 } },
        { "id": 2, "file_name": "font.ttf", "content_type": "font/ttf", "size": 8, "properties": { "uid": 555 } }
      ]
    }"#;

    #[test]
    fn parses_tracks_and_skips_unsupported_types() {
        let result = parse_identify_json(SAMPLE_JSON).unwrap();
        assert_eq!(result.container.title.as_deref(), Some("Sample"));
        assert_eq!(result.tracks.len(), 3);

        assert_eq!(result.tracks[0].track_type, TrackType::Video);
        assert_eq!(result.tracks[0].uid, 111);
        assert!(result.tracks[0].flags.default);
        assert!(!result.tracks[0].flags.original);

        assert_eq!(result.tracks[1].name.as_deref(), Some("English"));
        assert_eq!(result.tracks[1].language.as_deref(), Some("en"));
        assert!(result.tracks[1].flags.original);
        assert!(result.tracks[1].flags.commentary);
        assert!(result.tracks[1].flags.visual_impaired);

        assert_eq!(result.tracks[2].track_type, TrackType::Subtitles);
        assert_eq!(result.tracks[2].language.as_deref(), Some("fre"));
        assert!(result.tracks[2].flags.forced);
        assert!(result.tracks[2].flags.hearing_impaired);

        assert_eq!(result.attachments.len(), 2);
        assert_eq!(result.attachments[0].uid, 17232074509077980272);
        assert_eq!(result.attachments[0].file_name, "cover.jpg");
        assert!(result.attachments[0].is_image());
        assert!(!result.attachments[1].is_image());
    }

    #[test]
    fn attachments_are_optional() {
        let result = parse_identify_json(r#"{ "tracks": [] }"#).unwrap();
        assert!(result.attachments.is_empty());
    }

    #[tokio::test]
    async fn probe_uses_runner_identify_output() {
        let runner = crate::process::MockProcessRunner {
            identify_stdout: SAMPLE_JSON.to_string(),
            ..Default::default()
        };
        let result = probe(&runner, Path::new("/some/file.mkv")).await.unwrap();
        assert_eq!(result.tracks.len(), 3);
    }
}
