//! Applies a [`Profile`] to an analyzed file: decides which tracks to keep,
//! their order, names and flags. The result is a *proposal* the user can
//! still adjust in the UI before saving.

use crate::config::{AudioRule, LabelOverrides, Profile, SubtitleKind, SubtitleProfile, ORIGINAL_PLACEHOLDER};
use crate::labels::{audio_attrs, video_attrs};
use crate::lang;
use crate::template::{render, RenderMode, Tokens};
use mkv_types::{FileAnalysis, MediaTrack, TrackEdit, TrackFlags, TrackInfo, TrackType};
use regex::Regex;

pub const VIDEO_TOKENS: &[&str] = &["Resolution", "DynamicRange", "Codec", "BitDepth", "3D", "Language"];
pub const AUDIO_TOKENS: &[&str] = &["Language", "Format", "Channels", "Atmos", "Commentary", "Description"];
pub const SUBTITLE_TOKENS: &[&str] = &["Language", "Kind", "Format"];

/// One track's proposed fate.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackProposal {
    pub keep: bool,
    /// The proposed final state (for dropped tracks: unchanged source state).
    pub edit: TrackEdit,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileOutcome {
    /// Kept tracks in final output order, followed by dropped tracks in
    /// source order.
    pub tracks: Vec<TrackProposal>,
    pub warnings: Vec<String>,
}

impl ProfileOutcome {
    pub fn kept(&self) -> impl Iterator<Item = &TrackEdit> {
        self.tracks.iter().filter(|t| t.keep).map(|t| &t.edit)
    }
}

/// Best guess for the content's original language: an audio track already
/// flagged "original", else the first audio track with a real language.
pub fn guess_original_language(analysis: &FileAnalysis) -> Option<String> {
    let audio = || analysis.probe.tracks.iter().filter(|t| t.track_type == TrackType::Audio);
    audio()
        .find(|t| t.flags.original)
        .or_else(|| audio().find(|t| track_language(t, analysis).is_some()))
        .and_then(|t| track_language(t, analysis))
}

/// Canonical language of a track, preferring mkvmerge's value and falling
/// back to MediaInfo's; `None` when undetermined.
fn track_language(track: &TrackInfo, analysis: &FileAnalysis) -> Option<String> {
    let from_mkv = track.language.as_deref().filter(|l| !lang::is_undetermined(l));
    let from_media = analysis
        .media
        .track(track.uid)
        .and_then(|m| m.language.as_deref())
        .filter(|l| !lang::is_undetermined(l));
    from_mkv.or(from_media).map(lang::canonical)
}

fn regex(pattern: &str) -> Option<Regex> {
    // Patterns are validated when the config is saved; a bad one here just
    // disables that heuristic instead of failing the whole profile.
    Regex::new(pattern).ok()
}

fn matches(re: &Option<Regex>, text: Option<&str>) -> bool {
    matches!((re, text), (Some(re), Some(t)) if re.is_match(t))
}

fn name(template: &str, tokens: &Tokens) -> Option<String> {
    render(template, tokens, RenderMode::TrackName)
        .ok()
        .map(|r| r.text)
        .filter(|s| !s.is_empty())
}

fn base_edit(track: &TrackInfo) -> TrackEdit {
    TrackEdit {
        id: track.id,
        uid: track.uid,
        track_type: track.track_type,
        name: track.name.clone(),
        language: track.language.clone(),
        flags: track.flags,
    }
}

pub fn apply_profile(
    analysis: &FileAnalysis,
    profile: &Profile,
    labels: &LabelOverrides,
    original_language: Option<&str>,
) -> ProfileOutcome {
    let original = original_language
        .filter(|l| !lang::is_undetermined(l))
        .map(lang::canonical);
    let mut outcome = ProfileOutcome::default();
    let mut kept: Vec<TrackEdit> = Vec::new();

    let of_type = |t: TrackType| analysis.probe.tracks.iter().filter(move |x| x.track_type == t);

    // --- Video: keep all, rename, default on the first. -------------------
    for (i, track) in of_type(TrackType::Video).enumerate() {
        let media = analysis.media.track(track.uid);
        let attrs = video_attrs(media, &track.codec);
        let mut tokens = Tokens::new();
        tokens
            .set("Resolution", attrs.resolution.clone().unwrap_or_default())
            .set("DynamicRange", LabelOverrides::apply(&labels.dynamic_range, &attrs.dynamic_range.label()))
            .set("Codec", LabelOverrides::apply(&labels.video_codec, &attrs.codec))
            .set("BitDepth", attrs.bit_depth.map(|b| format!("{b}bit")).unwrap_or_default())
            .set("3D", if attrs.is_3d { "3D" } else { "" })
            .set("Language", track_language(track, analysis).map(|l| lang::display_name(&l)).unwrap_or_default());
        let mut edit = base_edit(track);
        edit.name = name(&profile.video.name_format, &tokens);
        edit.flags = TrackFlags { default: i == 0, ..TrackFlags::default() };
        kept.push(edit);
    }

    // --- Audio: language rules. -------------------------------------------
    kept.extend(apply_audio(analysis, profile, labels, original.as_deref(), &mut outcome.warnings));

    // --- Subtitles: keep languages, classify, order. ----------------------
    kept.extend(apply_subtitles(analysis, &profile.subtitle, labels, original.as_deref(), &mut outcome.warnings));

    let kept_uids: Vec<u64> = kept.iter().map(|e| e.uid).collect();
    outcome.tracks = kept
        .into_iter()
        .map(|edit| TrackProposal { keep: true, edit })
        .chain(
            analysis
                .probe
                .tracks
                .iter()
                .filter(|t| !kept_uids.contains(&t.uid))
                .map(|t| TrackProposal { keep: false, edit: base_edit(t) }),
        )
        .collect();
    outcome
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AudioRole {
    Main,
    Description,
    Commentary,
}

fn audio_role(track: &TrackInfo, commentary: &Option<Regex>, description: &Option<Regex>) -> AudioRole {
    let title = track.name.as_deref();
    if track.flags.commentary || matches(commentary, title) {
        AudioRole::Commentary
    } else if track.flags.visual_impaired || matches(description, title) {
        AudioRole::Description
    } else {
        AudioRole::Main
    }
}

fn resolve_rule<'a>(rules: &'a [AudioRule], original: &str) -> Option<&'a AudioRule> {
    rules.iter().find(|r| {
        r.when_original
            .iter()
            .any(|w| w.trim() == "*" || lang::same(w, original))
    })
}

fn apply_audio(
    analysis: &FileAnalysis,
    profile: &Profile,
    labels: &LabelOverrides,
    original: Option<&str>,
    warnings: &mut Vec<String>,
) -> Vec<TrackEdit> {
    let audio_cfg = &profile.audio;
    let commentary_re = regex(&audio_cfg.commentary_pattern);
    let description_re = regex(&audio_cfg.description_pattern);

    let tracks: Vec<(&TrackInfo, Option<String>, AudioRole)> = analysis
        .probe
        .tracks
        .iter()
        .filter(|t| t.track_type == TrackType::Audio)
        .map(|t| (t, track_language(t, analysis), audio_role(t, &commentary_re, &description_re)))
        .collect();
    if tracks.is_empty() {
        return Vec::new();
    }

    let allowed_role = |role: AudioRole| match role {
        AudioRole::Main => true,
        AudioRole::Commentary => audio_cfg.keep_commentary,
        AudioRole::Description => audio_cfg.keep_descriptions,
    };

    let mut selected: Vec<(&TrackInfo, Option<String>, AudioRole)> = Vec::new();
    match original {
        None => {
            warnings.push("No original language selected — keeping all audio tracks.".into());
            selected = tracks.clone();
        }
        Some(original) => {
            if !tracks.iter().any(|(_, l, _)| l.as_deref() == Some(original)) {
                warnings.push(format!(
                    "No {} audio track found (the selected original language).",
                    lang::display_name(original)
                ));
            }
            match resolve_rule(&audio_cfg.rules, original) {
                None => {
                    warnings.push("No audio rule matches the original language — keeping all audio tracks.".into());
                    selected = tracks.clone();
                }
                Some(rule) => {
                    let mut wanted: Vec<String> = Vec::new();
                    for entry in &rule.keep {
                        let code = if entry.trim() == ORIGINAL_PLACEHOLDER {
                            original.to_string()
                        } else {
                            lang::canonical(entry)
                        };
                        if !wanted.contains(&code) {
                            wanted.push(code);
                        }
                    }
                    for code in &wanted {
                        let mut group: Vec<_> = tracks
                            .iter()
                            .filter(|(_, l, role)| l.as_deref() == Some(code) && allowed_role(*role))
                            .cloned()
                            .collect();
                        // Main audio first, then descriptions, then commentary;
                        // stable, so source order is kept within each role.
                        group.sort_by_key(|(_, _, role)| *role);
                        selected.extend(group);
                    }
                    if selected.is_empty() {
                        let names: Vec<String> = wanted.iter().map(|c| lang::display_name(c)).collect();
                        warnings.push(format!(
                            "None of the profile's audio languages ({}) are present — keeping all audio tracks.",
                            names.join(", ")
                        ));
                        selected = tracks.clone();
                    }
                }
            }
        }
    }

    let first_main = selected.iter().position(|(_, _, role)| *role == AudioRole::Main);
    selected
        .into_iter()
        .enumerate()
        .map(|(i, (track, language, role))| {
            let attrs = audio_attrs(analysis.media.track(track.uid), &track.codec);
            let mut tokens = Tokens::new();
            tokens
                .set("Language", language.as_deref().map(lang::display_name).unwrap_or_default())
                .set("Format", LabelOverrides::apply(&labels.audio_format, &attrs.format))
                .set("Channels", attrs.channels.clone().unwrap_or_default())
                .set("Atmos", if attrs.atmos { "Atmos" } else { "" })
                .set("Commentary", if role == AudioRole::Commentary { "Commentary" } else { "" })
                .set("Description", if role == AudioRole::Description { "Audio Description" } else { "" });
            let mut edit = base_edit(track);
            edit.name = name(&audio_cfg.name_format, &tokens);
            edit.flags = TrackFlags {
                default: Some(i) == first_main,
                forced: false,
                original: original.is_some() && language.as_deref() == original,
                hearing_impaired: false,
                visual_impaired: role == AudioRole::Description,
                commentary: role == AudioRole::Commentary,
            };
            edit
        })
        .collect()
}

struct SubtitlePatterns {
    forced: Option<Regex>,
    sdh: Option<Regex>,
    cc: Option<Regex>,
    commentary: Option<Regex>,
    signs: Option<Regex>,
}

/// Classifies from flags first, then the track title. The element-count
/// heuristic for forced subtitles runs afterwards, across tracks.
fn subtitle_kind(track: &TrackInfo, p: &SubtitlePatterns) -> SubtitleKind {
    let title = track.name.as_deref();
    if track.flags.commentary || matches(&p.commentary, title) {
        SubtitleKind::Commentary
    } else if matches(&p.signs, title) {
        SubtitleKind::SignsSongs
    } else if track.flags.forced || matches(&p.forced, title) {
        SubtitleKind::Forced
    } else if matches(&p.cc, title) {
        SubtitleKind::Cc
    } else if track.flags.hearing_impaired || matches(&p.sdh, title) {
        SubtitleKind::Sdh
    } else {
        SubtitleKind::Full
    }
}

fn subtitle_format(media: Option<&MediaTrack>, mkv_codec: &str) -> String {
    let raw = media.and_then(|m| m.format.clone()).unwrap_or_else(|| mkv_codec.to_string());
    let lower = raw.to_ascii_lowercase();
    if lower.contains("utf-8") || lower.contains("srt") || lower.contains("subrip") {
        "SRT".into()
    } else if lower.contains("pgs") {
        "PGS".into()
    } else if lower.contains("ass") {
        "ASS".into()
    } else if lower.contains("ssa") {
        "SSA".into()
    } else if lower.contains("vobsub") {
        "VobSub".into()
    } else if lower.contains("timed text") || lower.contains("ttml") {
        "TTML".into()
    } else {
        raw
    }
}

fn apply_subtitles(
    analysis: &FileAnalysis,
    cfg: &SubtitleProfile,
    labels: &LabelOverrides,
    original: Option<&str>,
    warnings: &mut Vec<String>,
) -> Vec<TrackEdit> {
    let patterns = SubtitlePatterns {
        forced: regex(&cfg.forced_pattern),
        sdh: regex(&cfg.sdh_pattern),
        cc: regex(&cfg.cc_pattern),
        commentary: regex(&cfg.commentary_pattern),
        signs: regex(&cfg.signs_pattern),
    };
    let all_subs: Vec<&TrackInfo> = analysis
        .probe
        .tracks
        .iter()
        .filter(|t| t.track_type == TrackType::Subtitles)
        .collect();

    let keep_langs: Vec<String> = cfg.keep_languages.iter().map(|l| lang::canonical(l)).collect();
    let mut kept: Vec<(&TrackInfo, String, SubtitleKind)> = all_subs
        .iter()
        .filter_map(|t| {
            let language = track_language(t, analysis)?;
            keep_langs
                .contains(&language)
                .then(|| (*t, language, subtitle_kind(t, &patterns)))
        })
        .collect();

    if !all_subs.is_empty() && kept.is_empty() {
        let names: Vec<String> = keep_langs.iter().map(|c| lang::display_name(c)).collect();
        warnings.push(format!(
            "No {} subtitles found — all subtitle tracks will be removed.",
            names.join("/")
        ));
    }

    if cfg.detect_forced_by_element_count {
        for language in &keep_langs {
            let counts: Vec<(usize, u64)> = kept
                .iter()
                .enumerate()
                .filter(|(_, (_, l, kind))| l == language && *kind == SubtitleKind::Full)
                .filter_map(|(i, (t, _, _))| Some((i, analysis.media.track(t.uid)?.element_count?)))
                .collect();
            let max = counts.iter().map(|(_, c)| *c).max().unwrap_or(0);
            if counts.len() >= 2 && max > 0 {
                for (i, count) in counts {
                    if (count as f64) < max as f64 * 0.25 {
                        kept[i].2 = SubtitleKind::Forced;
                    }
                }
            }
        }
    }

    let rank = |kind: SubtitleKind| cfg.order.iter().position(|k| *k == kind).unwrap_or(cfg.order.len());
    kept.sort_by_key(|(_, _, kind)| rank(*kind)); // stable: source order within a kind

    let mut default_given = false;
    kept.into_iter()
        .map(|(track, language, kind)| {
            let mut tokens = Tokens::new();
            tokens
                .set("Language", lang::display_name(&language))
                .set("Kind", LabelOverrides::apply(&labels.subtitle_kind, kind.label()))
                .set("Format", subtitle_format(analysis.media.track(track.uid), &track.codec));
            let foreign = original.is_some_and(|o| o != language);
            let default = kind == SubtitleKind::Forced && cfg.default_forced_when_foreign && foreign && !default_given;
            default_given |= default;

            let mut edit = base_edit(track);
            edit.name = name(&cfg.name_format, &tokens);
            edit.flags = TrackFlags {
                default,
                forced: kind == SubtitleKind::Forced,
                original: original == Some(language.as_str()),
                hearing_impaired: matches!(kind, SubtitleKind::Sdh | SubtitleKind::Cc),
                visual_impaired: false,
                commentary: kind == SubtitleKind::Commentary,
            };
            edit
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use mkv_types::{ContainerInfo, MediaAnalysis, ProbeResult};

    struct Fixture {
        tracks: Vec<TrackInfo>,
        media: Vec<MediaTrack>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut f = Fixture { tracks: vec![], media: vec![] };
            f.video();
            f
        }

        fn push(&mut self, t: TrackType, codec: &str, lang: &str, name: Option<&str>, flags: TrackFlags, media: MediaTrack) -> &mut Self {
            let id = self.tracks.len() as u64;
            let uid = 1000 + id;
            self.tracks.push(TrackInfo {
                id,
                uid,
                track_type: t,
                codec: codec.into(),
                language: Some(lang.into()),
                name: name.map(Into::into),
                flags,
            });
            self.media.push(MediaTrack { uid, kind: Some(t), ..media });
            self
        }

        fn video(&mut self) -> &mut Self {
            self.push(TrackType::Video, "HEVC", "und", None, TrackFlags::default(), MediaTrack {
                format: Some("HEVC".into()),
                width: Some(3840),
                height: Some(2160),
                hdr_format: Some("Dolby Vision / SMPTE ST 2086".into()),
                hdr_format_profile: Some("dvhe.08.06".into()),
                hdr_format_compatibility: Some("HDR10 / HDR10".into()),
                transfer_characteristics: Some("PQ".into()),
                has_mastering_display: true,
                ..Default::default()
            })
        }

        fn audio(&mut self, lang: &str, format: &str, commercial: &str, features: &str, channels: u32) -> &mut Self {
            self.audio_named(lang, format, commercial, features, channels, None, TrackFlags::default())
        }

        #[allow(clippy::too_many_arguments)] // test fixture builder
        fn audio_named(&mut self, lang: &str, format: &str, commercial: &str, features: &str, channels: u32, name: Option<&str>, flags: TrackFlags) -> &mut Self {
            let layout = match channels {
                8 => "L R C LFE Ls Rs Lb Rb",
                6 => "L R C LFE Ls Rs",
                _ => "L R",
            };
            self.push(TrackType::Audio, format, lang, name, flags, MediaTrack {
                format: Some(format.into()),
                format_commercial: (!commercial.is_empty()).then(|| commercial.into()),
                format_additional_features: (!features.is_empty()).then(|| features.into()),
                channels: Some(channels),
                channel_layout: Some(layout.into()),
                ..Default::default()
            })
        }

        fn sub(&mut self, lang: &str, name: Option<&str>, flags: TrackFlags, elements: u64) -> &mut Self {
            self.push(TrackType::Subtitles, "SubRip/SRT", lang, name, flags, MediaTrack {
                format: Some("UTF-8".into()),
                element_count: Some(elements),
                ..Default::default()
            })
        }

        fn analysis(&self) -> FileAnalysis {
            FileAnalysis {
                file_name: "x.mkv".into(),
                probe: ProbeResult { container: ContainerInfo::default(), tracks: self.tracks.clone() },
                media: MediaAnalysis { tracks: self.media.clone(), ..Default::default() },
                sections: vec![],
            }
        }
    }

    fn run(f: &Fixture, original: Option<&str>) -> ProfileOutcome {
        let config = AppConfig::default();
        apply_profile(&f.analysis(), &config.profiles[0], &config.labels, original)
    }

    fn kept_names(o: &ProfileOutcome, t: TrackType) -> Vec<String> {
        o.kept().filter(|e| e.track_type == t).map(|e| e.name.clone().unwrap_or_default()).collect()
    }

    fn kept_default(o: &ProfileOutcome, t: TrackType) -> Vec<bool> {
        o.kept().filter(|e| e.track_type == t).map(|e| e.flags.default).collect()
    }

    #[test]
    fn video_is_named_resolution_range_codec() {
        let f = Fixture::new();
        let o = run(&f, Some("en"));
        assert_eq!(kept_names(&o, TrackType::Video), ["2160p DV8 HDR10 HEVC"]);
        assert_eq!(kept_default(&o, TrackType::Video), [true]);
    }

    fn multilingual() -> Fixture {
        let mut f = Fixture::new();
        f.audio("eng", "E-AC-3", "Dolby Digital Plus with Dolby Atmos", "JOC", 6)
            .audio("hin", "AC-3", "", "", 6)
            .audio("tel", "DTS", "DTS-HD Master Audio", "XLL", 8)
            .audio("tam", "MLP FBA", "Dolby TrueHD with Dolby Atmos", "16-ch", 8)
            .audio("fre", "AAC", "", "", 2);
        f
    }

    #[test]
    fn english_hindi_telugu_originals_keep_only_that_language() {
        let f = multilingual();
        for (original, expected) in [
            ("en", "English DD+ 5.1 Atmos"),
            ("hi", "Hindi DD 5.1"),
            ("te", "Telugu DTS-HD MA 7.1"),
        ] {
            let o = run(&f, Some(original));
            assert_eq!(kept_names(&o, TrackType::Audio), [expected], "original {original}");
            let a = o.kept().find(|e| e.track_type == TrackType::Audio).unwrap();
            assert!(a.flags.default && a.flags.original, "original {original}");
        }
    }

    #[test]
    fn tamil_malayalam_kannada_prefer_telugu_then_original_then_hindi() {
        let f = multilingual();
        let o = run(&f, Some("ta"));
        assert_eq!(
            kept_names(&o, TrackType::Audio),
            ["Telugu DTS-HD MA 7.1", "Tamil Dolby TrueHD 7.1 Atmos", "Hindi DD 5.1"]
        );
        assert_eq!(kept_default(&o, TrackType::Audio), [true, false, false]);
        let original_flags: Vec<bool> = o.kept().filter(|e| e.track_type == TrackType::Audio).map(|e| e.flags.original).collect();
        assert_eq!(original_flags, [false, true, false]);

        // Malayalam original not present, no Telugu: Hindi only, with a warning.
        let mut f = Fixture::new();
        f.audio("hin", "AC-3", "", "", 6).audio("eng", "AC-3", "", "", 2);
        let o = run(&f, Some("mal"));
        assert_eq!(kept_names(&o, TrackType::Audio), ["Hindi DD 5.1"]);
        assert_eq!(kept_default(&o, TrackType::Audio), [true]);
        assert!(o.warnings.iter().any(|w| w.contains("No Malayalam audio")), "{:?}", o.warnings);
    }

    #[test]
    fn other_originals_keep_original_plus_english() {
        let f = multilingual();
        let o = run(&f, Some("fr"));
        assert_eq!(kept_names(&o, TrackType::Audio), ["French AAC 2.0", "English DD+ 5.1 Atmos"]);
        assert_eq!(kept_default(&o, TrackType::Audio), [true, false]);
        let dropped: Vec<u64> = o.tracks.iter().filter(|t| !t.keep).map(|t| t.edit.id).collect();
        assert_eq!(dropped, [2, 3, 4], "hindi, telugu and tamil dropped");
    }

    #[test]
    fn nothing_matching_keeps_everything_with_warning() {
        let mut f = Fixture::new();
        f.audio("ger", "AC-3", "", "", 6);
        let o = run(&f, Some("te"));
        assert_eq!(kept_names(&o, TrackType::Audio), ["German DD 5.1"]);
        assert!(o.warnings.iter().any(|w| w.contains("keeping all audio")), "{:?}", o.warnings);

        let o = run(&f, None);
        assert_eq!(kept_names(&o, TrackType::Audio).len(), 1);
        assert!(o.warnings.iter().any(|w| w.contains("No original language")));
    }

    #[test]
    fn commentary_and_description_are_kept_flagged_and_never_default() {
        let mut f = Fixture::new();
        f.audio_named("eng", "AC-3", "", "", 2, Some("Director's Commentary"), TrackFlags::default())
            .audio("eng", "E-AC-3", "", "", 6)
            .audio_named("eng", "AC-3", "", "", 2, None, TrackFlags { visual_impaired: true, ..Default::default() });
        let o = run(&f, Some("en"));
        assert_eq!(
            kept_names(&o, TrackType::Audio),
            ["English DD+ 5.1", "English DD 2.0 Audio Description", "English DD 2.0 Commentary"]
        );
        let flags: Vec<TrackFlags> = o.kept().filter(|e| e.track_type == TrackType::Audio).map(|e| e.flags).collect();
        assert!(flags[0].default && !flags[0].commentary);
        assert!(!flags[1].default && flags[1].visual_impaired);
        assert!(!flags[2].default && flags[2].commentary);
    }

    #[test]
    fn subtitles_keep_english_classify_flag_and_order() {
        let mut f = Fixture::new();
        f.audio("jpn", "AAC", "", "", 2)
            .sub("eng", Some("English SDH"), TrackFlags::default(), 1500)
            .sub("fre", None, TrackFlags::default(), 1400)
            .sub("eng", None, TrackFlags::default(), 1450)
            .sub("eng", Some("Signs & Songs"), TrackFlags::default(), 40)
            .sub("eng", None, TrackFlags::default(), 30) // few events → forced
            .sub("eng", Some("Closed Captions"), TrackFlags::default(), 1500)
            .sub("eng", Some("Commentary"), TrackFlags::default(), 900);
        let o = run(&f, Some("ja"));
        assert_eq!(
            kept_names(&o, TrackType::Subtitles),
            ["English Forced", "English", "English SDH", "English CC", "English Signs & Songs", "English Commentary"]
        );
        let subs: Vec<TrackFlags> = o.kept().filter(|e| e.track_type == TrackType::Subtitles).map(|e| e.flags).collect();
        assert!(subs[0].forced && subs[0].default, "forced default when original is foreign");
        assert!(!subs[1].default && !subs[1].forced && !subs[1].hearing_impaired);
        assert!(subs[2].hearing_impaired && subs[3].hearing_impaired);
        assert!(subs[5].commentary);
        assert!(subs.iter().all(|s| !s.original), "subtitles aren't in the original language");
        assert!(o.tracks.iter().any(|t| !t.keep && t.edit.language.as_deref() == Some("fre")));
    }

    #[test]
    fn subtitle_flags_from_existing_matroska_flags() {
        let mut f = Fixture::new();
        f.audio("eng", "AAC", "", "", 2)
            .sub("eng", None, TrackFlags { hearing_impaired: true, ..Default::default() }, 1000)
            .sub("eng", None, TrackFlags { forced: true, ..Default::default() }, 1000);
        let o = run(&f, Some("en"));
        assert_eq!(kept_names(&o, TrackType::Subtitles), ["English Forced", "English SDH"]);
        let forced = o.kept().find(|e| e.name.as_deref() == Some("English Forced")).unwrap();
        assert!(!forced.flags.default, "original is English → not foreign → no default");
        assert!(forced.flags.original);
    }

    #[test]
    fn no_english_subtitles_warns() {
        let mut f = Fixture::new();
        f.audio("eng", "AAC", "", "", 2).sub("spa", None, TrackFlags::default(), 100);
        let o = run(&f, Some("en"));
        assert!(kept_names(&o, TrackType::Subtitles).is_empty());
        assert!(o.warnings.iter().any(|w| w.contains("No English subtitles")));
    }

    #[test]
    fn guesses_original_language() {
        let mut f = Fixture::new();
        f.audio("und", "AAC", "", "", 2)
            .audio("tel", "AAC", "", "", 2)
            .audio_named("hin", "AAC", "", "", 2, None, TrackFlags { original: true, ..Default::default() });
        assert_eq!(guess_original_language(&f.analysis()).as_deref(), Some("hi"));
        f.tracks[3].flags.original = false;
        assert_eq!(guess_original_language(&f.analysis()).as_deref(), Some("te"));
    }

    #[test]
    fn label_overrides_apply() {
        let f = multilingual();
        let mut config = AppConfig::default();
        config.labels.audio_format.insert("DD+".into(), "EAC3".into());
        config.labels.dynamic_range.insert("DV8 HDR10".into(), "DoVi P8".into());
        let o = apply_profile(&f.analysis(), &config.profiles[0], &config.labels, Some("en"));
        assert_eq!(kept_names(&o, TrackType::Audio), ["English EAC3 5.1 Atmos"]);
        assert_eq!(kept_names(&o, TrackType::Video), ["2160p DoVi P8 HEVC"]);
    }
}
