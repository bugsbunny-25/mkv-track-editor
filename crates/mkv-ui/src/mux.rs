//! Several files opened as one document, to be combined into a single
//! output. Their analyses are folded into one [`FileAnalysis`] with tracks
//! and attachments renumbered, so profiles, naming and every view work on a
//! mux exactly as on one file; [`Mux`] maps the numbers back for saving.

use crate::backend::SourceRef;
use mkv_types::{FileAnalysis, InfoSection, MediaAnalysis, ProbeResult};

/// Where a track or attachment of the combined analysis comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    /// Index into [`Mux::sources`].
    pub input: usize,
    /// mkvmerge's id for it in that file.
    pub id: u64,
    pub uid: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mux {
    pub sources: Vec<SourceRef>,
    /// Indexed by a combined track's id.
    pub tracks: Vec<Origin>,
    /// Indexed by a combined attachment's id.
    pub attachments: Vec<Origin>,
    /// The file whose video is kept, chosen by the user. `None` follows the
    /// file of the default audio track.
    pub video_from: Option<usize>,
    /// Delete the sources that went into the output once it's saved.
    pub delete_sources: bool,
}

impl Mux {
    pub fn track(&self, combined_id: u64) -> Option<Origin> {
        self.tracks.get(combined_id as usize).copied()
    }

    /// "File 2", as tracks and pickers name a source.
    pub fn label(input: usize) -> String {
        format!("File {}", input + 1)
    }
}

/// Folds the analyses (in `sources` order) into one. Combined track and
/// attachment ids are their positions; UIDs are those positions plus one,
/// so two files with the same track UIDs (copies of one release) can't
/// collide, and MediaInfo's tracks are renumbered to match. Container facts
/// come from the first file; its size is left out, since the output's
/// size isn't known yet.
pub fn combine(sources: Vec<SourceRef>, analyses: Vec<FileAnalysis>) -> (FileAnalysis, Mux) {
    let first = analyses.first();
    let mut combined = FileAnalysis {
        file_name: first.map(|a| a.file_name.clone()).unwrap_or_default(),
        probe: ProbeResult {
            container: first.map(|a| a.probe.container.clone()).unwrap_or_default(),
            tracks: Vec::new(),
            attachments: Vec::new(),
        },
        media: MediaAnalysis::default(),
        sections: Vec::new(),
    };
    if let Some(first) = first {
        combined.media.general = first.media.general.clone();
        combined.media.general.file_size = None;
    }
    let mut mux = Mux {
        sources,
        tracks: Vec::new(),
        attachments: Vec::new(),
        video_from: None,
        delete_sources: false,
    };

    for (input, analysis) in analyses.into_iter().enumerate() {
        for mut track in analysis.probe.tracks {
            let id = mux.tracks.len() as u64;
            mux.tracks.push(Origin { input, id: track.id, uid: track.uid });
            if let Some(media) = analysis.media.track(track.uid) {
                combined.media.tracks.push(mkv_types::MediaTrack { uid: id + 1, ..media.clone() });
            }
            track.id = id;
            track.uid = id + 1;
            combined.probe.tracks.push(track);
        }
        for mut attachment in analysis.probe.attachments {
            let id = mux.attachments.len() as u64;
            mux.attachments.push(Origin { input, id: attachment.id, uid: attachment.uid });
            attachment.id = id;
            attachment.uid = id + 1;
            combined.probe.attachments.push(attachment);
        }
        combined.sections.extend(analysis.sections.into_iter().map(|s| InfoSection {
            title: format!("{} · {}", Mux::label(input), s.title),
            rows: s.rows,
        }));
    }
    (combined, mux)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkv_types::{AttachmentInfo, ContainerInfo, InfoRow, MediaTrack, TrackFlags, TrackInfo, TrackType};

    fn analysis(name: &str, uids: &[u64]) -> FileAnalysis {
        FileAnalysis {
            file_name: name.into(),
            probe: ProbeResult {
                container: ContainerInfo::default(),
                tracks: uids
                    .iter()
                    .enumerate()
                    .map(|(i, uid)| TrackInfo {
                        id: i as u64,
                        uid: *uid,
                        track_type: if i == 0 { TrackType::Video } else { TrackType::Audio },
                        codec: "codec".into(),
                        language: None,
                        name: None,
                        flags: TrackFlags::default(),
                    })
                    .collect(),
                attachments: vec![AttachmentInfo {
                    id: 1,
                    uid: 500,
                    file_name: "cover.jpg".into(),
                    mime_type: "image/jpeg".into(),
                    size: 1,
                }],
            },
            media: MediaAnalysis {
                tracks: uids.iter().map(|uid| MediaTrack { uid: *uid, channels: Some(*uid as u32), ..Default::default() }).collect(),
                ..Default::default()
            },
            sections: vec![InfoSection { title: "General".into(), rows: vec![InfoRow { key: "k".into(), value: "v".into() }] }],
        }
    }

    #[test]
    fn renumbers_tracks_and_keeps_media_joined() {
        let sources = vec![SourceRef::LocalPath("/a.mkv".into()), SourceRef::LocalPath("/b.mkv".into())];
        // The same UIDs in both files, as in two copies of one release.
        let (combined, mux) = combine(sources, vec![analysis("a.mkv", &[7, 8]), analysis("b.mkv", &[7, 8, 9])]);

        let ids: Vec<u64> = combined.probe.tracks.iter().map(|t| t.id).collect();
        assert_eq!(ids, [0, 1, 2, 3, 4]);
        assert_eq!(mux.track(3), Some(Origin { input: 1, id: 1, uid: 8 }));
        assert_eq!(mux.track(5), None);

        // MediaInfo still describes the right track after renumbering.
        let uid_of_b9 = combined.probe.tracks[4].uid;
        assert_eq!(combined.media.track(uid_of_b9).and_then(|m| m.channels), Some(9));

        assert_eq!(combined.probe.attachments.len(), 2);
        assert_eq!(mux.attachments[1], Origin { input: 1, id: 1, uid: 500 });
        let titles: Vec<&str> = combined.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["File 1 · General", "File 2 · General"]);
        assert_eq!(combined.file_name, "a.mkv");
    }
}
