//! MediaInfo integration: `--Output=JSON` for machine-readable analysis
//! (HDR/Dolby Vision, Atmos, DTS:X, channel layout, ...), and the default text
//! report for the human-readable structured viewer.

use crate::error::CoreError;
use crate::probe::probe;
use crate::process::{MediaInfoOutput, ProcessRunner};
use mkv_types::{FileAnalysis, InfoRow, InfoSection, MediaAnalysis, MediaGeneral, MediaTrack, TrackType};
use serde_json::{Map, Value};
use std::path::Path;
use std::str::FromStr;

/// Probe (mkvmerge) + MediaInfo JSON + MediaInfo text, run concurrently.
pub async fn analyze(runner: &dyn ProcessRunner, path: &Path) -> Result<FileAnalysis, CoreError> {
    let (probe_result, json, text) = tokio::join!(
        probe(runner, path),
        runner.mediainfo(path, MediaInfoOutput::Json),
        runner.mediainfo(path, MediaInfoOutput::Text),
    );
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    Ok(FileAnalysis {
        probe: probe_result?,
        media: parse_mediainfo_json(&json?)?,
        sections: parse_mediainfo_text(&text?, &file_name),
        file_name,
    })
}

/// Parses `mediainfo --Output=JSON`. Every MediaInfo value is a JSON string
/// (even numbers), so fields are read as strings and parsed leniently.
pub fn parse_mediainfo_json(raw: &str) -> Result<MediaAnalysis, CoreError> {
    let root: Value = serde_json::from_str(raw)?;
    let tracks = root
        .get("media")
        .and_then(|m| m.get("track"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut analysis = MediaAnalysis::default();
    for track in tracks {
        let Value::Object(fields) = track else { continue };
        let f = Fields(&fields);
        match f.str("@type").as_deref() {
            Some("General") => {
                analysis.general = MediaGeneral {
                    format: f.str("Format"),
                    title: f.str("Title").or_else(|| f.str("Movie")),
                    duration_seconds: f.num("Duration"),
                    file_size: f.num("FileSize"),
                    overall_bit_rate: f.num("OverallBitRate"),
                };
            }
            Some(kind @ ("Video" | "Audio" | "Text")) => {
                // Without a UniqueID there's no way to join with mkvmerge's
                // view of the track; skip it rather than guess.
                let Some(uid) = f.num::<u64>("UniqueID") else { continue };
                let kind = match kind {
                    "Video" => TrackType::Video,
                    "Audio" => TrackType::Audio,
                    _ => TrackType::Subtitles,
                };
                analysis.tracks.push(MediaTrack {
                    uid,
                    kind: Some(kind),
                    format: f.str("Format"),
                    format_commercial: f.str("Format_Commercial_IfAny"),
                    format_profile: f.str("Format_Profile"),
                    format_additional_features: f.str("Format_AdditionalFeatures"),
                    codec_id: f.str("CodecID"),
                    language: f.str("Language"),
                    title: f.str("Title"),
                    width: f.num("Width"),
                    height: f.num("Height"),
                    bit_depth: f.num("BitDepth"),
                    frame_rate: f.num("FrameRate"),
                    hdr_format: f.str("HDR_Format"),
                    hdr_format_profile: f.str("HDR_Format_Profile"),
                    hdr_format_compatibility: f.str("HDR_Format_Compatibility"),
                    transfer_characteristics: f.str("transfer_characteristics"),
                    has_mastering_display: f.has("MasteringDisplay_ColorPrimaries")
                        || f.has("MasteringDisplay_Luminance"),
                    multiview_count: f.num("MultiView_Count"),
                    encoded_library_name: f.str("Encoded_Library_Name"),
                    channels: f.num("Channels"),
                    channel_layout: f.str("ChannelLayout"),
                    sampling_rate: f.num("SamplingRate"),
                    element_count: f.num("ElementCount"),
                });
            }
            _ => {}
        }
    }
    Ok(analysis)
}

struct Fields<'a>(&'a Map<String, Value>);

impl Fields<'_> {
    fn str(&self, key: &str) -> Option<String> {
        match self.0.get(key)? {
            Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    fn has(&self, key: &str) -> bool {
        self.str(key).is_some()
    }

    /// Parses the leading number of a field, tolerating values like
    /// "8 / 6" (multiple sub-streams) or "23.976".
    fn num<T: FromStr>(&self, key: &str) -> Option<T> {
        let s = self.str(key)?;
        let leading: String = s
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        leading.parse().ok().or_else(|| {
            // Integer targets can't parse "23.976"; fall back to the part
            // before the decimal point.
            leading.split('.').next()?.parse().ok()
        })
    }
}

/// Parses MediaInfo's default text report into titled sections. The
/// "Complete name" row (an absolute server-side path) is replaced with just
/// the file name so no container/host filesystem layout leaks to clients.
pub fn parse_mediainfo_text(text: &str, file_name: &str) -> Vec<InfoSection> {
    let mut sections: Vec<InfoSection> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        match line.split_once(" : ") {
            Some((key, value)) => {
                let key = key.trim().to_string();
                let value = if key == "Complete name" {
                    file_name.to_string()
                } else {
                    value.trim().to_string()
                };
                match sections.last_mut() {
                    Some(section) => section.rows.push(InfoRow { key, value }),
                    None => sections.push(InfoSection {
                        title: "General".into(),
                        rows: vec![InfoRow { key, value }],
                    }),
                }
            }
            None => sections.push(InfoSection {
                title: line.trim().to_string(),
                rows: Vec::new(),
            }),
        }
    }
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"media":{"track":[
      {"@type":"General","Format":"Matroska","Duration":"7200.5","FileSize":"123456"},
      {"@type":"Video","UniqueID":"5712197752799084509","Format":"HEVC","Width":"3840","Height":"2160",
       "BitDepth":"10","FrameRate":"23.976","HDR_Format":"Dolby Vision / SMPTE ST 2086",
       "HDR_Format_Profile":"dvhe.08.06 / ","HDR_Format_Compatibility":"HDR10 / HDR10",
       "transfer_characteristics":"PQ","MasteringDisplay_Luminance":"min: 0.0050 cd/m2, max: 1000 cd/m2",
       "Encoded_Library_Name":"x265"},
      {"@type":"Audio","UniqueID":"12032068110368226109","Format":"E-AC-3",
       "Format_Commercial_IfAny":"Dolby Digital Plus with Dolby Atmos","Format_AdditionalFeatures":"JOC",
       "Channels":"6","ChannelLayout":"L R C LFE Ls Rs","Language":"en"},
      {"@type":"Audio","UniqueID":"42","Format":"DTS","Channels":"8 / 6","Language":"hi"},
      {"@type":"Text","UniqueID":"8546186038945566966","Format":"UTF-8","Title":"English SDH","ElementCount":"1234"},
      {"@type":"Text","Format":"PGS"},
      {"@type":"Menu"}
    ]}}"#;

    #[test]
    fn parses_tracks_by_uid_with_lenient_numbers() {
        let a = parse_mediainfo_json(JSON).unwrap();
        assert_eq!(a.general.format.as_deref(), Some("Matroska"));
        assert_eq!(a.general.duration_seconds, Some(7200.5));
        // The Text track without a UniqueID and the Menu are skipped.
        assert_eq!(a.tracks.len(), 4);

        let v = a.track(5712197752799084509).unwrap();
        assert_eq!(v.kind, Some(TrackType::Video));
        assert_eq!(v.width, Some(3840));
        assert_eq!(v.bit_depth, Some(10));
        assert_eq!(v.frame_rate, Some(23.976));
        assert!(v.has_mastering_display);
        assert_eq!(v.hdr_format_profile.as_deref(), Some("dvhe.08.06 /"));
        assert_eq!(v.encoded_library_name.as_deref(), Some("x265"));

        let atmos = a.track(12032068110368226109).unwrap();
        assert_eq!(atmos.format_additional_features.as_deref(), Some("JOC"));
        assert_eq!(atmos.channels, Some(6));

        let dts = a.track(42).unwrap();
        assert_eq!(dts.channels, Some(8), "'8 / 6' parses as its leading number");

        let sub = a.track(8546186038945566966).unwrap();
        assert_eq!(sub.kind, Some(TrackType::Subtitles));
        assert_eq!(sub.element_count, Some(1234));
    }

    #[test]
    fn parses_text_report_into_sections_and_hides_absolute_path() {
        let text = "General\n\
            Unique ID                                : 1234\n\
            Complete name                            : /data/library/secret/dir/Movie.mkv\n\
            Format                                   : Matroska\n\
            \n\
            Video\n\
            Width                                    : 3 840 pixels\n\
            \n\
            Audio #1\n\
            Format                                   : E-AC-3 JOC\n\
            \n\
            Menu\n\
            00:00:00.000                             : :Chapter 1\n";
        let sections = parse_mediainfo_text(text, "Movie.mkv");
        let titles: Vec<_> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["General", "Video", "Audio #1", "Menu"]);

        let complete = sections[0].rows.iter().find(|r| r.key == "Complete name").unwrap();
        assert_eq!(complete.value, "Movie.mkv");
        assert!(!sections.iter().flat_map(|s| &s.rows).any(|r| r.value.contains("/data/")));

        assert_eq!(sections[1].rows[0].value, "3 840 pixels");
        assert_eq!(sections[3].rows[0].key, "00:00:00.000");
        assert_eq!(sections[3].rows[0].value, ":Chapter 1");
    }

    #[tokio::test]
    async fn analyze_joins_all_three_outputs() {
        let runner = crate::process::MockProcessRunner {
            identify_stdout: r#"{"container":{},"tracks":[]}"#.into(),
            mediainfo_json: JSON.into(),
            mediainfo_text: "General\nFormat : Matroska\n".into(),
            ..Default::default()
        };
        let result = analyze(&runner, Path::new("/lib/Some.Movie.2020.mkv")).await.unwrap();
        assert_eq!(result.file_name, "Some.Movie.2020.mkv");
        assert_eq!(result.media.tracks.len(), 4);
        assert_eq!(result.sections[0].title, "General");
        assert_eq!(runner.calls.lock().unwrap().len(), 3);
    }
}
