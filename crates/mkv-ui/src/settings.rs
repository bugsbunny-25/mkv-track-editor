//! Conversions between `AppConfig` and the Settings tab's compact text
//! fields (one audio rule / custom format per line), plus the live preview.
//! Pure logic, unit-tested without Slint.

use crate::document::Document;
use mkv_profiles::config::{AudioRule, CustomFormat, NamingConfig, SubtitleKind};
use mkv_profiles::template::ColonReplacement;
use mkv_profiles::{apply_profile, render_file_name, summarize, AppConfig, Profile};
use mkv_types::TrackType;

pub const COLON_OPTIONS: [(ColonReplacement, &str); 5] = [
    (ColonReplacement::Smart, "Smart (\": \" → \" - \", \":\" → \"-\")"),
    (ColonReplacement::Delete, "Delete"),
    (ColonReplacement::Dash, "Replace with dash"),
    (ColonReplacement::SpaceDash, "Replace with space dash"),
    (ColonReplacement::SpaceDashSpace, "Replace with space dash space"),
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileForm {
    pub name: String,
    pub video_format: String,
    pub audio_format: String,
    pub audio_rules: String,
    pub keep_commentary: bool,
    pub keep_descriptions: bool,
    pub subtitle_languages: String,
    pub subtitle_format: String,
    pub subtitle_order: String,
    pub default_forced_when_foreign: bool,
    pub detect_forced_by_element_count: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NamingForm {
    pub movie_format: String,
    pub episode_format: String,
    pub colon_index: usize,
    pub custom_formats: String,
}

fn list_to_text(items: &[String]) -> String {
    items.join(", ")
}

fn text_to_list(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

fn meaningful_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
}

/// `en, hi, te => {original}` per line.
pub fn rules_to_text(rules: &[AudioRule]) -> String {
    rules
        .iter()
        .map(|r| format!("{} => {}", list_to_text(&r.when_original), list_to_text(&r.keep)))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn rules_from_text(text: &str) -> Result<Vec<AudioRule>, String> {
    meaningful_lines(text)
        .map(|(n, line)| {
            let (when, keep) = line
                .split_once("=>")
                .ok_or_else(|| format!("audio rule line {n}: expected `languages => keep order`"))?;
            let rule = AudioRule {
                when_original: text_to_list(when),
                keep: text_to_list(keep),
            };
            if rule.when_original.is_empty() || rule.keep.is_empty() {
                return Err(format!("audio rule line {n}: both sides of `=>` need at least one language"));
            }
            Ok(rule)
        })
        .collect()
}

/// `Name = regex` per line.
pub fn custom_formats_to_text(formats: &[CustomFormat]) -> String {
    formats
        .iter()
        .map(|f| format!("{} = {}", f.name, f.pattern))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn custom_formats_from_text(text: &str) -> Result<Vec<CustomFormat>, String> {
    meaningful_lines(text)
        .map(|(n, line)| {
            let (name, pattern) = line
                .split_once('=')
                .ok_or_else(|| format!("custom format line {n}: expected `Name = regex`"))?;
            Ok(CustomFormat {
                name: name.trim().to_string(),
                pattern: pattern.trim().to_string(),
            })
        })
        .collect()
}

fn kind_from_key(key: &str) -> Option<SubtitleKind> {
    SubtitleKind::ALL
        .into_iter()
        .find(|k| k.key().eq_ignore_ascii_case(key.trim()))
}

pub fn order_to_text(order: &[SubtitleKind]) -> String {
    order.iter().map(|k| k.key()).collect::<Vec<_>>().join(", ")
}

/// Unlisted kinds are appended, so a subtitle is never left without a rank.
pub fn order_from_text(text: &str) -> Result<Vec<SubtitleKind>, String> {
    let mut order = Vec::new();
    for item in text_to_list(text) {
        let kind = kind_from_key(&item).ok_or_else(|| {
            let valid: Vec<&str> = SubtitleKind::ALL.iter().map(|k| k.key()).collect();
            format!("unknown subtitle kind `{item}` (use {})", valid.join(", "))
        })?;
        if !order.contains(&kind) {
            order.push(kind);
        }
    }
    for kind in SubtitleKind::ALL {
        if !order.contains(&kind) {
            order.push(kind);
        }
    }
    Ok(order)
}

pub fn profile_to_form(p: &Profile) -> ProfileForm {
    ProfileForm {
        name: p.name.clone(),
        video_format: p.video.name_format.clone(),
        audio_format: p.audio.name_format.clone(),
        audio_rules: rules_to_text(&p.audio.rules),
        keep_commentary: p.audio.keep_commentary,
        keep_descriptions: p.audio.keep_descriptions,
        subtitle_languages: list_to_text(&p.subtitle.keep_languages),
        subtitle_format: p.subtitle.name_format.clone(),
        subtitle_order: order_to_text(&p.subtitle.order),
        default_forced_when_foreign: p.subtitle.default_forced_when_foreign,
        detect_forced_by_element_count: p.subtitle.detect_forced_by_element_count,
    }
}

/// Applies the form on top of `base`, preserving settings the form doesn't
/// expose (detection regexes).
pub fn form_to_profile(form: &ProfileForm, base: &Profile) -> Result<Profile, String> {
    let mut p = base.clone();
    p.name = form.name.trim().to_string();
    p.video.name_format = form.video_format.clone();
    p.audio.name_format = form.audio_format.clone();
    p.audio.rules = rules_from_text(&form.audio_rules)?;
    p.audio.keep_commentary = form.keep_commentary;
    p.audio.keep_descriptions = form.keep_descriptions;
    p.subtitle.keep_languages = text_to_list(&form.subtitle_languages);
    p.subtitle.name_format = form.subtitle_format.clone();
    p.subtitle.order = order_from_text(&form.subtitle_order)?;
    p.subtitle.default_forced_when_foreign = form.default_forced_when_foreign;
    p.subtitle.detect_forced_by_element_count = form.detect_forced_by_element_count;
    Ok(p)
}

pub fn naming_to_form(n: &NamingConfig) -> NamingForm {
    NamingForm {
        movie_format: n.movie_format.clone(),
        episode_format: n.episode_format.clone(),
        colon_index: COLON_OPTIONS
            .iter()
            .position(|(c, _)| *c == n.colon_replacement)
            .unwrap_or(0),
        custom_formats: custom_formats_to_text(&n.custom_formats),
    }
}

pub fn form_to_naming(form: &NamingForm) -> Result<NamingConfig, String> {
    Ok(NamingConfig {
        movie_format: form.movie_format.clone(),
        episode_format: form.episode_format.clone(),
        colon_replacement: COLON_OPTIONS.get(form.colon_index).map(|(c, _)| *c).unwrap_or_default(),
        custom_formats: custom_formats_from_text(&form.custom_formats)?,
    })
}

/// What the draft settings would do to the active file: its output name and
/// the kept tracks' names/flags.
pub fn preview(doc: &Document, config: &AppConfig, profile_index: usize) -> String {
    let Some(profile) = config.profiles.get(profile_index) else {
        return String::new();
    };
    let outcome = apply_profile(&doc.analysis, profile, &config.labels, doc.original_language.as_deref());
    let kept: Vec<_> = outcome.kept().cloned().collect();
    let summary = summarize(&doc.analysis, &kept);

    let mut lines = Vec::new();
    match render_file_name(&config.naming, &doc.metadata, &summary) {
        Ok(r) => lines.push(r.text),
        Err(e) => lines.push(format!("⚠ {e}")),
    }
    lines.push(String::new());
    for edit in &kept {
        let kind = match edit.track_type {
            TrackType::Video => "Video",
            TrackType::Audio => "Audio",
            TrackType::Subtitles => "Sub",
        };
        let f = edit.flags;
        let flags: Vec<&str> = [
            (f.default, "default"),
            (f.forced, "forced"),
            (f.original, "original"),
            (f.hearing_impaired, "SDH"),
            (f.visual_impaired, "AD"),
            (f.commentary, "commentary"),
        ]
        .into_iter()
        .filter_map(|(on, label)| on.then_some(label))
        .collect();
        let flags = if flags.is_empty() { String::new() } else { format!("  [{}]", flags.join(", ")) };
        lines.push(format!("{kind:<6} {}{flags}", edit.name.as_deref().unwrap_or("—")));
    }
    let dropped = outcome.tracks.iter().filter(|t| !t.keep).count();
    if dropped > 0 {
        lines.push(format!("({dropped} track(s) removed)"));
    }
    for w in &outcome.warnings {
        lines.push(format!("⚠ {w}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_round_trips_through_the_form() {
        let config = AppConfig::default();
        let p = &config.profiles[0];
        let form = profile_to_form(p);
        assert_eq!(
            form.audio_rules,
            "en, hi, te => {original}\nta, ml, kn => te, {original}, hi\n* => {original}, en"
        );
        assert_eq!(form_to_profile(&form, p).unwrap(), *p);

        let naming = naming_to_form(&config.naming);
        assert_eq!(form_to_naming(&naming).unwrap(), config.naming);
    }

    #[test]
    fn rule_parsing_errors_point_at_the_line() {
        let err = rules_from_text("en => {original}\n\n# comment\nbad line").unwrap_err();
        assert!(err.contains("line 4"), "{err}");
        assert!(rules_from_text(" => en").is_err());
    }

    #[test]
    fn subtitle_order_fills_in_missing_kinds() {
        let order = order_from_text("sdh, forced").unwrap();
        assert_eq!(order[..2], [SubtitleKind::Sdh, SubtitleKind::Forced]);
        assert_eq!(order.len(), SubtitleKind::ALL.len());
        assert!(order_from_text("bogus").is_err());
    }

    #[test]
    fn custom_formats_allow_equals_in_the_regex() {
        let cfs = custom_formats_from_text("Weird = (?i)a=b").unwrap();
        assert_eq!(cfs[0].name, "Weird");
        assert_eq!(cfs[0].pattern, "(?i)a=b");
    }
}
