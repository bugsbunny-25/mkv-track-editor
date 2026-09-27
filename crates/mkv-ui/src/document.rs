//! One open file ("tab"): its analysis, the proposed track edits, naming
//! metadata and job state. Pure logic — no Slint types — so it's shared
//! verbatim by desktop and wasm and unit-testable on its own.

use crate::backend::SourceRef;
use mkv_profiles::labels::{audio_attrs, video_attrs, DynamicRange};
use mkv_profiles::lang;
use mkv_profiles::{
    apply_profile, guess_original_language, render_file_name, summarize, AppConfig, MediaSummary, NamingMetadata,
};
use mkv_types::{FileAnalysis, JobId, TrackEdit, TrackFlags, TrackType};

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
    Done { output: String },
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
    pub source: SourceRef,
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
        let metadata = NamingMetadata::from_file_name(source.file_name(), &config.naming);
        let mut doc = Self {
            id,
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

    pub fn title(&self) -> &str {
        self.source.file_name()
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

    pub fn has_changes(&self) -> bool {
        self.rows != self.original_rows()
    }

    /// Rows that differ from the file on disk: dropped, edited or moved.
    pub fn change_count(&self) -> usize {
        let original = self.original_rows();
        self.rows.iter().zip(&original).filter(|(row, orig)| row != orig).count()
    }

    /// Title for the document header: "Title (Year)" from the naming
    /// metadata, falling back to the file name.
    pub fn display_title(&self) -> String {
        let title = self.metadata.title.trim();
        match (title.is_empty(), self.metadata.year) {
            (true, _) => self.title().trim_end_matches(".mkv").to_string(),
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
        let first = |t: TrackType| {
            self.rows
                .iter()
                .find(|r| r.keep && r.edit.track_type == t)
                .or_else(|| self.rows.iter().find(|r| r.edit.track_type == t))
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
        self.rows = outcome
            .tracks
            .into_iter()
            .map(|t| Row { keep: t.keep, edit: t.edit })
            .collect();
        self.refresh_output_name(config);
    }

    pub fn reset(&mut self, config: &AppConfig) {
        self.rows = self.original_rows();
        self.warnings.clear();
        self.refresh_output_name(config);
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

    pub fn set_keep(&mut self, row: usize, keep: bool, config: &AppConfig) {
        if let Some(r) = self.rows.get_mut(row) {
            r.keep = keep;
        }
        self.refresh_output_name(config);
    }

    pub fn set_name(&mut self, row: usize, name: &str) {
        if let Some(r) = self.rows.get_mut(row) {
            r.edit.name = (!name.trim().is_empty()).then(|| name.to_string());
        }
    }

    pub fn set_language(&mut self, row: usize, language: &str, config: &AppConfig) {
        if let Some(r) = self.rows.get_mut(row) {
            r.edit.language = (!language.trim().is_empty()).then(|| language.trim().to_string());
        }
        self.refresh_output_name(config);
    }

    /// Setting "default" on one track clears it on the other tracks of the
    /// same type — players pick the first default anyway, so more than one
    /// is almost always a mistake.
    pub fn set_flag(&mut self, row: usize, flag: Flag, value: bool) {
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
        doc.set_flag(1, Flag::Default, true);
        doc.set_flag(2, Flag::Default, true);
        assert!(!doc.rows[1].edit.flags.default && doc.rows[2].edit.flags.default);
        doc.set_flag(4, Flag::Default, true);
        assert!(doc.rows[2].edit.flags.default, "other types unaffected");
        doc.set_flag(4, Flag::HearingImpaired, true);
        assert!(doc.rows[4].edit.flags.hearing_impaired);
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
