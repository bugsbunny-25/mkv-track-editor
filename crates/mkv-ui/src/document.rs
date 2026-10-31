//! One open file ("tab"): its analysis, the proposed track edits, naming
//! metadata and job state. Pure logic — no Slint types — so it's shared
//! verbatim by desktop and wasm and unit-testable on its own.

use crate::backend::SourceRef;
use crate::mux::{self, Mux};
use mkv_profiles::labels::{audio_attrs, video_attrs, DynamicRange};
use mkv_profiles::lang;
use mkv_profiles::{
    apply_profile, guess_original_language, render_file_name, summarize, track_name, AppConfig, MediaSummary,
    NamingMetadata,
};
use mkv_types::{
    is_image_codec, AttachmentInfo, FileAnalysis, JobId, MuxTrack, TrackEdit, TrackFlags, TrackType,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub keep: bool,
    pub edit: TrackEdit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Idle,
    /// Waiting in a "Save all" queue.
    Queued,
    Running { job: Option<JobId>, percent: u8 },
    /// `note`: what happened afterwards, e.g. deleting a mux's sources.
    Done { output: String, note: String },
    Failed { message: String },
}

impl JobState {
    pub fn is_busy(&self) -> bool {
        matches!(self, JobState::Queued | JobState::Running { .. })
    }
}

/// What a header fact describes — the UI picks a colour per kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    Video,
    /// HDR / Dolby Vision (not emitted for SDR).
    HighDynamicRange,
    Audio,
    Subtitle,
    /// Codec, size, duration.
    Detail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub kind: FactKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Default,
    Forced,
    Original,
    HearingImpaired,
    VisualImpaired,
    Commentary,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub id: u64,
    /// For a mux, its first file.
    pub source: SourceRef,
    /// Set when the document combines several files into one output.
    pub mux: Option<Mux>,
    pub analysis: FileAnalysis,
    pub profile: String,
    pub original_language: Option<String>,
    pub rows: Vec<Row>,
    pub warnings: Vec<String>,
    pub metadata: NamingMetadata,
    pub output_name: String,
    /// The user typed their own output name; stop auto-updating it.
    pub output_overridden: bool,
    pub job: JobState,
    pub info_section: usize,
    pub info_filter: String,
}

impl Document {
    /// Opens a file and immediately applies the first profile, so the tab
    /// starts out showing the proposed result (which the user can reset).
    pub fn open(id: u64, source: SourceRef, analysis: FileAnalysis, config: &AppConfig) -> Self {
        Self::new(id, source, None, analysis, config)
    }

    /// Opens several files (in `sources` order, analyses alike) as one
    /// document whose output combines them. The video comes from the file
    /// of the default audio track the profile picks, and the naming fields
    /// are prefilled from that file's name.
    pub fn open_mux(id: u64, sources: Vec<SourceRef>, analyses: Vec<FileAnalysis>, config: &AppConfig) -> Self {
        let (analysis, mux) = mux::combine(sources, analyses);
        let source = mux.sources[0].clone();
        let mut doc = Self::new(id, source, Some(mux), analysis, config);
        if let Some(video) = doc.video_input().and_then(|i| doc.mux.as_ref()?.sources.get(i).cloned()) {
            doc.metadata = NamingMetadata::from_file_name(video.file_name(), &config.naming);
            doc.refresh_output_name(config);
        }
        doc
    }

    fn new(id: u64, source: SourceRef, mux: Option<Mux>, analysis: FileAnalysis, config: &AppConfig) -> Self {
        let metadata = NamingMetadata::from_file_name(source.file_name(), &config.naming);
        let mut doc = Self {
            id,
            mux,
            original_language: guess_original_language(&analysis),
            profile: config.profiles.first().map(|p| p.name.clone()).unwrap_or_default(),
            rows: Vec::new(),
            warnings: Vec::new(),
            metadata,
            output_name: String::new(),
            output_overridden: false,
            job: JobState::Idle,
            info_section: 0,
            info_filter: String::new(),
            source,
            analysis,
        };
        doc.rows = doc.original_rows();
        doc.apply_profile(config);
        doc
    }

    /// Tab title: the file name, or for a mux its first file's name and
    /// how many more it combines.
    pub fn title(&self) -> String {
        match &self.mux {
            Some(m) if m.sources.len() > 1 => format!("{} + {} more", self.source.file_name(), m.sources.len() - 1),
            _ => self.source.file_name().to_string(),
        }
    }

    /// Every file the document reads.
    pub fn sources(&self) -> Vec<&SourceRef> {
        match &self.mux {
            Some(m) => m.sources.iter().collect(),
            None => vec![&self.source],
        }
    }

    /// The file's tracks exactly as they are on disk.
    pub fn original_rows(&self) -> Vec<Row> {
        self.analysis
            .probe
            .tracks
            .iter()
            .map(|t| Row {
                keep: true,
                edit: TrackEdit {
                    id: t.id,
                    uid: t.uid,
                    track_type: t.track_type,
                    name: t.name.clone(),
                    language: t.language.clone(),
                    flags: t.flags,
                },
            })
            .collect()
    }

    /// A mux always has something to save: the combined file.
    pub fn has_changes(&self) -> bool {
        self.mux.is_some() || self.rows != self.original_rows() || !self.removed_attachments().is_empty()
    }

    /// Rows that differ from the file on disk (dropped, edited or moved),
    /// plus attachments being removed.
    pub fn change_count(&self) -> usize {
        let original = self.original_rows();
        let rows = self.rows.iter().zip(&original).filter(|(row, orig)| row != orig).count();
        rows + self.removed_attachments().len()
    }

    /// Attachments left out of the saved file: every image (cover art).
    /// Fonts and other attachments are kept — styled subtitles need them.
    pub fn removed_attachments(&self) -> Vec<AttachmentInfo> {
        self.analysis.probe.attachments.iter().filter(|a| a.is_image()).cloned().collect()
    }

    /// Banner lines for the attachments being removed.
    fn attachment_notices(&self) -> Vec<String> {
        self.removed_attachments()
            .iter()
            .map(|a| format!("The image attachment \u{201c}{}\u{201d} will be removed.", a.file_name))
            .collect()
    }

    /// Title for the document header: "Title (Year)" from the naming
    /// metadata, falling back to the file name.
    pub fn display_title(&self) -> String {
        let title = self.metadata.title.trim();
        match (title.is_empty(), self.metadata.year) {
            (true, _) => self.source.file_name().trim_end_matches(".mkv").to_string(),
            (false, Some(year)) => format!("{title} ({year})"),
            (false, None) => title.to_string(),
        }
    }

    /// At-a-glance media facts for the header, reflecting the tracks being
    /// kept: resolution, dynamic range and codec of the first video track,
    /// the primary audio format, subtitle count, then size and duration.
    pub fn header_facts(&self) -> Vec<Fact> {
        let mut facts = Vec::new();
        let mut push = |kind, text: String| {
            if !text.is_empty() {
                facts.push(Fact { kind, text });
            }
        };
        let codec_of = |uid: u64| {
            self.analysis.probe.tracks.iter().find(|t| t.uid == uid).map(|t| t.codec.as_str()).unwrap_or_default()
        };
        // Cover-art tracks never describe the file's video.
        let is_image = |r: &&Row| is_image_codec(codec_of(r.edit.uid));
        let first = |t: TrackType| {
            let of_type = || self.rows.iter().filter(move |r| r.edit.track_type == t);
            of_type()
                .find(|r| r.keep && !is_image(r))
                .or_else(|| of_type().find(|r| !is_image(r)))
                .or_else(|| of_type().next())
        };

        if let Some(row) = first(TrackType::Video) {
            let v = video_attrs(self.analysis.media.track(row.edit.uid), codec_of(row.edit.uid));
            push(FactKind::Video, v.resolution.clone().unwrap_or_default());
            if v.dynamic_range != DynamicRange::SDR {
                push(FactKind::HighDynamicRange, v.dynamic_range.label());
            }
            push(FactKind::Detail, v.codec);
        }
        if let Some(row) = first(TrackType::Audio) {
            let a = audio_attrs(self.analysis.media.track(row.edit.uid), codec_of(row.edit.uid));
            let mut text = a.format;
            if let Some(ch) = a.channels {
                text = format!("{text} {ch}");
            }
            if a.atmos {
                text.push_str(" Atmos");
            }
            push(FactKind::Audio, text);
        }
        let subtitles = self.rows.iter().filter(|r| r.keep && r.edit.track_type == TrackType::Subtitles).count();
        if subtitles > 0 {
            push(FactKind::Subtitle, format!("{subtitles} subtitle{}", if subtitles == 1 { "" } else { "s" }));
        }

        let general = &self.analysis.media.general;
        push(FactKind::Detail, general.file_size.map(format_size).unwrap_or_default());
        let seconds = general
            .duration_seconds
            .or_else(|| self.analysis.probe.container.duration_ns.map(|ns| ns as f64 / 1e9));
        push(FactKind::Detail, seconds.map(format_duration).unwrap_or_default());
        facts
    }

    pub fn apply_profile(&mut self, config: &AppConfig) {
        let Some(profile) = config.profile(&self.profile).or_else(|| config.profiles.first()) else {
            return;
        };
        self.profile = profile.name.clone();
        let outcome = apply_profile(&self.analysis, profile, &config.labels, self.original_language.as_deref());
        self.warnings = outcome.warnings;
        self.warnings.extend(self.attachment_notices());
        self.rows = outcome
            .tracks
            .into_iter()
            .map(|t| Row { keep: t.keep, edit: t.edit })
            .collect();
        self.follow_video_source();
        self.refresh_output_name(config);
    }

    /// The file as it is on disk; for a mux, every file's tracks, but
    /// still one file's video.
    pub fn reset(&mut self, config: &AppConfig) {
        self.rows = self.original_rows();
        self.warnings = self.attachment_notices();
        self.follow_video_source();
        self.refresh_output_name(config);
    }

    /// For a mux: the file a row's track comes from.
    pub fn input_of(&self, row: usize) -> Option<usize> {
        let mux = self.mux.as_ref()?;
        Some(mux.track(self.rows.get(row)?.edit.id)?.input)
    }

    fn is_image_row(&self, row: &Row) -> bool {
        self.analysis.probe.tracks.iter().any(|t| t.uid == row.edit.uid && t.is_image())
    }

    /// For a mux: the file whose video is kept — the user's choice, else
    /// the file of the kept default audio track (or of the first kept audio
    /// track, or the first file with a video).
    pub fn video_input(&self) -> Option<usize> {
        let mux = self.mux.as_ref()?;
        if let Some(input) = mux.video_from {
            return Some(input);
        }
        let of_type = |t: TrackType| (0..self.rows.len()).filter(move |&i| self.rows[i].edit.track_type == t);
        let audio = || of_type(TrackType::Audio).filter(|&i| self.rows[i].keep);
        audio()
            .find(|&i| self.rows[i].edit.flags.default)
            .or_else(|| audio().next())
            .or_else(|| of_type(TrackType::Video).find(|&i| !self.is_image_row(&self.rows[i])))
            .and_then(|i| self.input_of(i))
            .or(Some(0))
    }

    /// For a mux: keeps the video of [`Self::video_input`] only (default
    /// on its first track) and drops the other files' video. Cover-art
    /// tracks are left as the profile proposed.
    fn follow_video_source(&mut self) {
        let Some(target) = self.video_input() else { return };
        let mut first = true;
        for i in 0..self.rows.len() {
            if self.rows[i].edit.track_type != TrackType::Video || self.is_image_row(&self.rows[i]) {
                continue;
            }
            let keep = self.input_of(i) == Some(target);
            let row = &mut self.rows[i];
            row.keep = keep;
            row.edit.flags.default = keep && first;
            first &= !keep;
        }
    }

    /// For a mux: choose the file whose video is kept (`None` follows
    /// the default audio track again).
    pub fn set_video_from(&mut self, input: Option<usize>, config: &AppConfig) {
        if let Some(mux) = &mut self.mux {
            mux.video_from = input.filter(|&i| i < mux.sources.len());
            self.follow_video_source();
            self.refresh_output_name(config);
        }
    }

    /// While the video follows the default audio, re-picks it after an
    /// audio row changed.
    fn audio_changed(&mut self, row: usize) {
        let follows = self.mux.as_ref().is_some_and(|m| m.video_from.is_none());
        if follows && self.rows.get(row).is_some_and(|r| r.edit.track_type == TrackType::Audio) {
            self.follow_video_source();
        }
    }

    /// For a mux: the sources that go into the output, in mkvmerge input
    /// order (the video's file first, since chapters and the title come from
    /// the first input), each with its attachments to leave out, and the kept
    /// tracks renumbered to those inputs. Files none of whose tracks are kept
    /// aren't inputs at all.
    pub fn mux_job(&self) -> Option<(Vec<(SourceRef, Vec<AttachmentInfo>)>, Vec<MuxTrack>)> {
        let mux = self.mux.as_ref()?;
        let kept: Vec<usize> = (0..self.rows.len()).filter(|&i| self.rows[i].keep).collect();
        let primary = kept
            .iter()
            .find(|&&i| self.rows[i].edit.track_type == TrackType::Video && !self.is_image_row(&self.rows[i]))
            .or(kept.first())
            .and_then(|&i| self.input_of(i))?;

        let mut order = vec![primary];
        for i in &kept {
            let input = self.input_of(*i)?;
            if !order.contains(&input) {
                order.push(input);
            }
        }
        // Keep the user's file order after the primary.
        order[1..].sort_unstable();
        let position = |input: usize| order.iter().position(|&o| o == input);

        let removed = self.removed_attachments();
        let inputs = order
            .iter()
            .map(|&input| {
                let attachments = removed
                    .iter()
                    .filter_map(|a| {
                        let origin = mux.attachments.get(a.id as usize)?;
                        (origin.input == input).then(|| AttachmentInfo { id: origin.id, uid: origin.uid, ..a.clone() })
                    })
                    .collect();
                (mux.sources[input].clone(), attachments)
            })
            .collect();
        let tracks = kept
            .iter()
            .map(|&i| {
                let origin = mux.track(self.rows[i].edit.id)?;
                let edit = TrackEdit { id: origin.id, uid: origin.uid, ..self.rows[i].edit.clone() };
                Some(MuxTrack { input: position(origin.input)?, edit })
            })
            .collect::<Option<Vec<_>>>()?;
        Some((inputs, tracks))
    }

    pub fn kept(&self) -> Vec<TrackEdit> {
        self.rows.iter().filter(|r| r.keep).map(|r| r.edit.clone()).collect()
    }

    pub fn summary(&self) -> MediaSummary {
        summarize(&self.analysis, &self.kept())
    }

    /// The name the configured format produces, plus any unknown tokens.
    pub fn auto_output_name(&self, config: &AppConfig) -> (String, Vec<String>) {
        match render_file_name(&config.naming, &self.metadata, &self.summary()) {
            Ok(r) => (r.text, r.unknown_tokens),
            Err(e) => (format!("{}.mkv", self.metadata.original_file_name), vec![e.to_string()]),
        }
    }

    pub fn refresh_output_name(&mut self, config: &AppConfig) {
        if !self.output_overridden {
            self.output_name = self.auto_output_name(config).0;
        }
    }

    /// Only adjacent rows of the same track type can swap, which keeps the
    /// video → audio → subtitle grouping intact.
    pub fn can_move(&self, from: usize, to: usize) -> bool {
        match (self.rows.get(from), self.rows.get(to)) {
            (Some(a), Some(b)) => from.abs_diff(to) == 1 && a.edit.track_type == b.edit.track_type,
            _ => false,
        }
    }

    pub fn move_row(&mut self, from: usize, to: usize, config: &AppConfig) {
        if self.can_move(from, to) {
            self.rows.swap(from, to);
            self.refresh_output_name(config);
        }
    }

    /// In a mux, keeping a video track pins the video to its file; the
    /// video no longer follows the default audio.
    pub fn set_keep(&mut self, row: usize, keep: bool, config: &AppConfig) {
        if let Some(r) = self.rows.get_mut(row) {
            r.keep = keep;
        }
        let pinned = self.input_of(row).filter(|_| {
            keep && self.rows[row].edit.track_type == TrackType::Video && !self.is_image_row(&self.rows[row])
        });
        if let (Some(input), Some(mux)) = (pinned, &mut self.mux) {
            mux.video_from = Some(input);
        }
        self.audio_changed(row);
        self.refresh_output_name(config);
    }

    pub fn set_name(&mut self, row: usize, name: &str) {
        if let Some(r) = self.rows.get_mut(row) {
            r.edit.name = (!name.trim().is_empty()).then(|| name.to_string());
        }
    }

    /// The current profile's name for a row in its current state, or `None`
    /// when the profile doesn't name it.
    fn auto_name(&self, row: usize, config: &AppConfig) -> Option<String> {
        let profile = config.profile(&self.profile).or_else(|| config.profiles.first())?;
        track_name(&self.analysis, profile, &config.labels, &self.rows.get(row)?.edit)
    }

    /// Runs `edit` on a row; if the row's name was the profile's
    /// auto-generated one (not typed by the user), renames it to match the
    /// edited language and flags — e.g. "English" → "English SDH".
    fn edit_named(&mut self, row: usize, config: &AppConfig, edit: impl FnOnce(&mut Self)) {
        let untouched = self.rows.get(row).is_some_and(|r| r.edit.name.is_some())
            && self.rows[row].edit.name == self.auto_name(row, config);
        edit(self);
        if untouched {
            if let Some(name) = self.auto_name(row, config) {
                self.rows[row].edit.name = Some(name);
            }
        }
    }

    pub fn set_language(&mut self, row: usize, language: &str, config: &AppConfig) {
        self.edit_named(row, config, |doc| {
            if let Some(r) = doc.rows.get_mut(row) {
                r.edit.language = (!language.trim().is_empty()).then(|| language.trim().to_string());
            }
        });
        self.refresh_output_name(config);
    }

    /// Setting "default" on one track clears it on the other tracks of the
    /// same type — players pick the first default anyway, so more than one
    /// is almost always a mistake. An auto-generated name follows the flags.
    pub fn set_flag(&mut self, row: usize, flag: Flag, value: bool, config: &AppConfig) {
        self.edit_named(row, config, |doc| doc.apply_flag(row, flag, value));
        if flag == Flag::Default {
            self.audio_changed(row);
        }
        self.refresh_output_name(config);
    }

    fn apply_flag(&mut self, row: usize, flag: Flag, value: bool) {
        let Some(track_type) = self.rows.get(row).map(|r| r.edit.track_type) else { return };
        if flag == Flag::Default && value {
            for (i, r) in self.rows.iter_mut().enumerate() {
                if i != row && r.edit.track_type == track_type {
                    r.edit.flags.default = false;
                }
            }
        }
        let f: &mut TrackFlags = &mut self.rows[row].edit.flags;
        match flag {
            Flag::Default => f.default = value,
            Flag::Forced => f.forced = value,
            Flag::Original => f.original = value,
            Flag::HearingImpaired => f.hearing_impaired = value,
            Flag::VisualImpaired => f.visual_impaired = value,
            Flag::Commentary => f.commentary = value,
        }
    }

    /// Candidate original languages: the audio languages present in the
    /// file first (most likely picks), then every other known language.
    pub fn language_options(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in self.analysis.probe.tracks.iter().filter(|t| t.track_type == TrackType::Audio) {
            if let Some(code) = t.language.as_deref().filter(|l| !lang::is_undetermined(l)) {
                let code = lang::canonical(code);
                if !out.contains(&code) {
                    out.push(code);
                }
            }
        }
        if let Some(orig) = &self.original_language {
            if !out.contains(orig) {
                out.insert(0, orig.clone());
            }
        }
        let present = out.len();
        let mut rest: Vec<&lang::Language> = lang::LANGUAGES
            .iter()
            .filter(|l| !lang::is_undetermined(l.code()) && !out.iter().any(|c| c == l.code()))
            .collect();
        rest.sort_by_key(|l| l.name);
        out.extend(rest.into_iter().map(|l| l.code().to_string()));
        debug_assert!(present <= out.len());
        out
    }

    /// Rows of the selected media-info section, filtered case-insensitively
    /// by key or value. With a filter, all sections are searched.
    pub fn info_rows(&self) -> Vec<(String, String)> {
        let filter_owned = self.info_filter.trim().to_lowercase();
        let filter = filter_owned.as_str();
        if filter.is_empty() {
            return self
                .analysis
                .sections
                .get(self.info_section)
                .map(|s| s.rows.iter().map(|r| (r.key.clone(), r.value.clone())).collect())
                .unwrap_or_default();
        }
        self.analysis
            .sections
            .iter()
            .flat_map(|s| {
                s.rows.iter().filter_map(move |r| {
                    let hit = r.key.to_lowercase().contains(filter) || r.value.to_lowercase().contains(filter);
                    hit.then(|| (format!("{} › {}", s.title, r.key), r.value.clone()))
                })
            })
            .collect()
    }

    /// Checks that would otherwise only fail once the job is running.
    pub fn validate_for_save(&self) -> Result<(), String> {
        if !self.rows.iter().any(|r| r.keep) {
            return Err("Keep at least one track.".into());
        }
        let name = self.output_name.trim();
        if name.is_empty() {
            return Err("The output file name is empty.".into());
        }
        if !name.to_ascii_lowercase().ends_with(".mkv") {
            return Err("The output file name must end with .mkv.".into());
        }
        Ok(())
    }
}

/// "58.2 GB", "700 MB" (decimal units, like file managers).
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// "2h 46m", "42m", "45s".
fn format_duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m"),
        _ => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkv_types::{ContainerInfo, InfoRow, InfoSection, MediaAnalysis, MediaTrack, ProbeResult, TrackInfo};

    fn track(id: u64, t: TrackType, lang: &str, codec: &str) -> TrackInfo {
        TrackInfo {
            id,
            uid: 100 + id,
            track_type: t,
            codec: codec.into(),
            language: Some(lang.into()),
            name: None,
            flags: TrackFlags::default(),
        }
    }

    fn analysis() -> FileAnalysis {
        FileAnalysis {
            file_name: "Movie.2020.1080p.BluRay.x264-GRP.mkv".into(),
            probe: ProbeResult {
                container: ContainerInfo::default(),
                tracks: vec![
                    track(0, TrackType::Video, "und", "AVC/H.264/MPEG-4p10"),
                    track(1, TrackType::Audio, "tel", "E-AC-3"),
                    track(2, TrackType::Audio, "hin", "AC-3"),
                    track(3, TrackType::Audio, "eng", "AAC"),
                    track(4, TrackType::Subtitles, "eng", "SubRip/SRT"),
                    track(5, TrackType::Subtitles, "fre", "SubRip/SRT"),
                ],
                attachments: vec![],
            },
            media: MediaAnalysis {
                tracks: vec![
                    MediaTrack {
                        uid: 100,
                        format: Some("AVC".into()),
                        width: Some(1920),
                        height: Some(1080),
                        ..Default::default()
                    },
                    MediaTrack {
                        uid: 101,
                        format: Some("E-AC-3".into()),
                        channels: Some(6),
                        channel_layout: Some("L R C LFE Ls Rs".into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            sections: vec![
                InfoSection {
                    title: "General".into(),
                    rows: vec![InfoRow { key: "Format".into(), value: "Matroska".into() }],
                },
                InfoSection {
                    title: "Audio #1".into(),
                    rows: vec![InfoRow { key: "Format".into(), value: "E-AC-3".into() }],
                },
            ],
        }
    }

    fn open() -> (Document, AppConfig) {
        let config = AppConfig::default();
        let source = SourceRef::LocalPath("/lib/Movie.2020.1080p.BluRay.x264-GRP.mkv".into());
        (Document::open(1, source, analysis(), &config), config)
    }

    #[test]
    fn opening_applies_the_default_profile_and_names_the_output() {
        let (doc, _) = open();
        assert_eq!(doc.original_language.as_deref(), Some("te"));
        let kept: Vec<u64> = doc.kept().iter().map(|e| e.id).collect();
        assert_eq!(kept, [0, 1, 4], "Telugu original → only Telugu audio, English subs");
        assert!(doc.has_changes());
        assert_eq!(doc.output_name, "Movie (2020) - [Bluray-1080p][EAC3 5.1][h264]-GRP.mkv");
    }

    #[test]
    fn changing_original_language_and_reapplying() {
        let (mut doc, config) = open();
        doc.original_language = Some("fr".into());
        doc.apply_profile(&config);
        let kept_audio: Vec<u64> = doc.kept().iter().filter(|e| e.track_type == TrackType::Audio).map(|e| e.id).collect();
        assert_eq!(kept_audio, [3], "no French audio → English only");
        assert!(doc.warnings.iter().any(|w| w.contains("No French audio")));
    }

    #[test]
    fn reset_restores_the_file_as_is() {
        let (mut doc, config) = open();
        doc.reset(&config);
        assert!(!doc.has_changes());
        assert_eq!(doc.kept().len(), 6);
    }

    #[test]
    fn moves_are_limited_to_neighbours_of_the_same_type() {
        let (mut doc, config) = open();
        doc.reset(&config);
        assert!(!doc.can_move(0, 1), "video can't swap with audio");
        assert!(doc.can_move(1, 2));
        doc.move_row(1, 2, &config);
        assert_eq!(doc.rows[1].edit.id, 2);
        assert!(!doc.can_move(1, 3), "only adjacent rows");
    }

    #[test]
    fn default_flag_is_exclusive_per_track_type() {
        let (mut doc, config) = open();
        doc.reset(&config);
        doc.set_flag(1, Flag::Default, true, &config);
        doc.set_flag(2, Flag::Default, true, &config);
        assert!(!doc.rows[1].edit.flags.default && doc.rows[2].edit.flags.default);
        doc.set_flag(4, Flag::Default, true, &config);
        assert!(doc.rows[2].edit.flags.default, "other types unaffected");
        doc.set_flag(4, Flag::HearingImpaired, true, &config);
        assert!(doc.rows[4].edit.flags.hearing_impaired);
    }

    /// Index of the kept row with this mkvmerge track id.
    fn row_of(doc: &Document, id: u64) -> usize {
        doc.rows.iter().position(|r| r.edit.id == id).unwrap()
    }

    #[test]
    fn auto_names_follow_flags_and_language() {
        let (mut doc, config) = open();
        let sub = row_of(&doc, 4);
        assert_eq!(doc.rows[sub].edit.name.as_deref(), Some("English"));

        doc.set_flag(sub, Flag::HearingImpaired, true, &config);
        assert_eq!(doc.rows[sub].edit.name.as_deref(), Some("English SDH"));
        doc.set_flag(sub, Flag::Forced, true, &config);
        assert_eq!(doc.rows[sub].edit.name.as_deref(), Some("English Forced"));
        doc.set_flag(sub, Flag::Forced, false, &config);
        doc.set_flag(sub, Flag::HearingImpaired, false, &config);
        assert_eq!(doc.rows[sub].edit.name.as_deref(), Some("English"));

        doc.set_language(sub, "fr", &config);
        assert_eq!(doc.rows[sub].edit.name.as_deref(), Some("French"));

        let audio = row_of(&doc, 1);
        doc.set_flag(audio, Flag::Commentary, true, &config);
        assert!(doc.rows[audio].edit.name.as_deref().unwrap().ends_with(" Commentary"));
    }

    #[test]
    fn typed_names_are_left_alone() {
        let (mut doc, config) = open();
        let sub = row_of(&doc, 4);
        doc.set_name(sub, "My subs");
        doc.set_flag(sub, Flag::HearingImpaired, true, &config);
        assert_eq!(doc.rows[sub].edit.name.as_deref(), Some("My subs"));

        // A file's own names (after reset) aren't the profile's either.
        doc.reset(&config);
        doc.set_flag(sub, Flag::HearingImpaired, true, &config);
        assert_eq!(doc.rows[sub].edit.name, None);
    }

    fn with_cover_art() -> FileAnalysis {
        let mut a = analysis();
        // ffmpeg stores cover art as a one-frame MJPEG video track, and some
        // files mark it default.
        let mut cover = track(6, TrackType::Video, "und", "V_MJPEG");
        cover.flags.default = true;
        a.probe.tracks.insert(0, cover);
        let attachment = |id: u64, file_name: &str, mime_type: &str| AttachmentInfo {
            id,
            uid: 900 + id,
            file_name: file_name.into(),
            mime_type: mime_type.into(),
            size: 1,
        };
        a.probe.attachments = vec![attachment(1, "cover.jpg", "image/jpeg"), attachment(2, "font.ttf", "font/ttf")];
        a
    }

    #[test]
    fn cover_art_tracks_and_image_attachments_are_removed() {
        let config = AppConfig::default();
        let source = SourceRef::LocalPath("/lib/Movie.2020.1080p.BluRay.x264-GRP.mkv".into());
        let mut doc = Document::open(1, source, with_cover_art(), &config);

        let cover = &doc.rows[row_of(&doc, 6)];
        assert!(!cover.keep, "cover art track is dropped");
        let video = doc.kept().into_iter().find(|e| e.track_type == TrackType::Video).unwrap();
        assert_eq!(video.id, 0);
        assert!(video.flags.default);
        assert_eq!(doc.header_facts()[0].text, "1080p");

        let removed: Vec<String> = doc.removed_attachments().into_iter().map(|a| a.file_name).collect();
        assert_eq!(removed, ["cover.jpg"], "fonts are kept");
        assert!(doc.warnings.iter().any(|w| w.contains("cover.jpg")));

        // Resetting the tracks still removes the image, so there's a change.
        doc.reset(&config);
        assert!(doc.has_changes());
        assert_eq!(doc.change_count(), 1);
        assert_eq!(doc.header_facts()[0].text, "1080p", "even when the cover track is kept");
    }

    #[test]
    fn output_name_follows_edits_until_overridden() {
        let (mut doc, config) = open();
        doc.metadata.tmdb_id = "42".into();
        doc.refresh_output_name(&config);
        assert!(doc.output_name.contains("[tmdbid-42]"), "{}", doc.output_name);

        doc.output_name = "custom.mkv".into();
        doc.output_overridden = true;
        doc.set_keep(1, false, &config);
        assert_eq!(doc.output_name, "custom.mkv");
    }

    #[test]
    fn language_options_put_present_languages_first() {
        let (doc, _) = open();
        let options = doc.language_options();
        assert_eq!(&options[..3], ["te", "hi", "en"]);
        assert!(options.contains(&"ja".to_string()));
        assert!(!options.contains(&"und".to_string()));
    }

    #[test]
    fn media_info_filtering() {
        let (mut doc, _) = open();
        assert_eq!(doc.info_rows(), [("Format".to_string(), "Matroska".to_string())]);
        doc.info_filter = "e-ac".into();
        assert_eq!(doc.info_rows(), [("Audio #1 › Format".to_string(), "E-AC-3".to_string())]);
    }

    #[test]
    fn change_count_tracks_edits() {
        let (mut doc, config) = open();
        doc.reset(&config);
        assert_eq!(doc.change_count(), 0);
        doc.set_keep(5, false, &config);
        doc.set_name(0, "Main");
        assert_eq!(doc.change_count(), 2);
        doc.move_row(1, 2, &config);
        assert_eq!(doc.change_count(), 4, "a swap changes both rows");
    }

    #[test]
    fn header_shows_title_and_kept_media_facts() {
        let (mut doc, config) = open();
        assert_eq!(doc.display_title(), "Movie (2020)");
        let facts = doc.header_facts();
        let texts: Vec<&str> = facts.iter().map(|f| f.text.as_str()).collect();
        assert_eq!(texts, ["1080p", "AVC", "DD+ 5.1", "1 subtitle"], "SDR is omitted");
        assert_eq!(facts[0].kind, FactKind::Video);

        doc.reset(&config);
        let subs = doc.header_facts().into_iter().find(|f| f.kind == FactKind::Subtitle).unwrap();
        assert_eq!(subs.text, "2 subtitles");
    }

    /// File 1: 2160p video, English audio and subs. File 2: 1080p video,
    /// Telugu audio flagged original, Hindi audio and a cover image. Both
    /// number their tracks from 0 with the same UIDs.
    fn open_mux() -> (Document, AppConfig) {
        let config = AppConfig::default();
        let mut uhd = analysis();
        uhd.file_name = "Movie.2020.2160p.WEB-DL.DDP5.1.H.265-UHD.mkv".into();
        uhd.probe.tracks = vec![
            track(0, TrackType::Video, "und", "HEVC/H.265/MPEG-H"),
            track(1, TrackType::Audio, "eng", "E-AC-3"),
            track(2, TrackType::Subtitles, "eng", "SubRip/SRT"),
        ];
        uhd.media.tracks[0].width = Some(3840);
        uhd.media.tracks[0].height = Some(2160);
        let mut telugu = analysis();
        telugu.probe.tracks = vec![
            track(0, TrackType::Video, "und", "AVC/H.264/MPEG-4p10"),
            track(1, TrackType::Audio, "tel", "E-AC-3"),
            track(2, TrackType::Audio, "hin", "AC-3"),
        ];
        telugu.probe.tracks[1].flags.original = true;
        telugu.probe.attachments = vec![AttachmentInfo {
            id: 1,
            uid: 900,
            file_name: "cover.jpg".into(),
            mime_type: "image/jpeg".into(),
            size: 1,
        }];
        let sources = vec![
            SourceRef::LocalPath("/lib/Movie.2020.2160p.WEB-DL.DDP5.1.H.265-UHD.mkv".into()),
            SourceRef::LocalPath("/lib/Movie.2020.1080p.BluRay.x264-GRP.mkv".into()),
        ];
        (Document::open_mux(1, sources, vec![uhd, telugu], &config), config)
    }

    /// (file, id in that file) of the kept tracks, in order.
    fn kept_origins(doc: &Document) -> Vec<(usize, u64)> {
        let mux = doc.mux.as_ref().unwrap();
        doc.kept().iter().map(|e| mux.track(e.id).map(|o| (o.input, o.id)).unwrap()).collect()
    }

    #[test]
    fn mux_video_follows_the_default_audio() {
        let (doc, _) = open_mux();
        assert_eq!(doc.original_language.as_deref(), Some("te"));
        // Telugu rule keeps only Telugu audio (file 2), so file 2's video.
        assert_eq!(kept_origins(&doc), [(1, 0), (1, 1), (0, 2)]);
        assert_eq!(doc.video_input(), Some(1));
        assert!(doc.kept()[0].flags.default);
        assert_eq!(doc.title(), "Movie.2020.2160p.WEB-DL.DDP5.1.H.265-UHD.mkv + 1 more");
        assert!(doc.has_changes());
        // Naming comes from the file the video is taken from.
        assert_eq!(doc.metadata.release_group, "GRP");
    }

    #[test]
    fn mux_video_choice_sticks_until_auto() {
        let (mut doc, config) = open_mux();
        doc.set_video_from(Some(0), &config);
        assert_eq!(kept_origins(&doc)[0], (0, 0), "2160p video from file 1");
        assert!(!doc.rows[row_of(&doc, 3)].keep, "file 2's video is dropped");

        // Re-applying the profile keeps the user's choice.
        doc.apply_profile(&config);
        assert_eq!(kept_origins(&doc)[0], (0, 0));

        doc.set_video_from(None, &config);
        assert_eq!(kept_origins(&doc)[0], (1, 0));
    }

    #[test]
    fn mux_video_follows_a_manual_default_audio_change() {
        let (mut doc, config) = open_mux();
        let english = row_of(&doc, 1);
        doc.set_keep(english, true, &config);
        doc.set_flag(english, Flag::Default, true, &config);
        assert_eq!(doc.video_input(), Some(0));
        assert_eq!(kept_origins(&doc)[0], (0, 0));

        // Keeping a video by hand pins it.
        let video_2 = row_of(&doc, 3);
        doc.set_keep(video_2, true, &config);
        assert_eq!(doc.mux.as_ref().unwrap().video_from, Some(1));
        doc.set_flag(english, Flag::Default, true, &config);
        assert_eq!(doc.video_input(), Some(1));
    }

    #[test]
    fn mux_job_maps_tracks_back_to_their_files() {
        let (mut doc, config) = open_mux();
        doc.set_video_from(Some(0), &config);
        let (inputs, tracks) = doc.mux_job().unwrap();

        // The video's file is the first input.
        let files: Vec<&str> = inputs.iter().map(|(s, _)| s.file_name()).collect();
        assert_eq!(files, ["Movie.2020.2160p.WEB-DL.DDP5.1.H.265-UHD.mkv", "Movie.2020.1080p.BluRay.x264-GRP.mkv"]);
        let order: Vec<(usize, u64, u64)> = tracks.iter().map(|t| (t.input, t.edit.id, t.edit.uid)).collect();
        assert_eq!(order, [(0, 0, 100), (1, 1, 101), (0, 2, 102)], "original ids and UIDs");
        assert_eq!(inputs[1].1.iter().map(|a| (a.id, a.uid)).collect::<Vec<_>>(), [(1, 900)]);

        // A file none of whose tracks are kept isn't an input; the video's
        // file still comes first.
        doc.set_video_from(Some(1), &config);
        let subtitles = row_of(&doc, 2);
        doc.set_keep(subtitles, false, &config);
        let (inputs, tracks) = doc.mux_job().unwrap();
        assert_eq!(inputs.len(), 1);
        assert!(tracks.iter().all(|t| t.input == 0));
    }

    #[test]
    fn sizes_and_durations_are_human_readable() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(58_213_000_000), "58.2 GB");
        assert_eq!(format_size(700_000_000), "700 MB");
        assert_eq!(format_duration(9960.0), "2h 46m");
        assert_eq!(format_duration(2520.0), "42m");
        assert_eq!(format_duration(45.2), "45s");
    }

    #[test]
    fn save_validation() {
        let (mut doc, config) = open();
        assert!(doc.validate_for_save().is_ok());
        doc.output_name = "x.mp4".into();
        assert!(doc.validate_for_save().is_err());
        doc.output_name = "x.mkv".into();
        for i in 0..doc.rows.len() {
            doc.set_keep(i, false, &config);
        }
        assert!(doc.validate_for_save().is_err());
    }
}
