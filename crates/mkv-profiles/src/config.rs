//! User-editable configuration: named track profiles and file-naming
//! templates. Serialized as TOML on disk and JSON over the API; every field
//! has a default so partial/older config files still load.

use crate::template::{self, ColonReplacement};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub profiles: Vec<Profile>,
    pub naming: NamingConfig,
    pub labels: LabelOverrides,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            profiles: vec![Profile::default()],
            naming: NamingConfig::default(),
            labels: LabelOverrides::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub video: VideoProfile,
    pub audio: AudioProfile,
    pub subtitle: SubtitleProfile,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "Default".into(),
            video: VideoProfile::default(),
            audio: AudioProfile::default(),
            subtitle: SubtitleProfile::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoProfile {
    /// Tokens: Resolution, DynamicRange, Codec, BitDepth, 3D, Language.
    pub name_format: String,
}

impl Default for VideoProfile {
    fn default() -> Self {
        Self {
            name_format: "{Resolution} {DynamicRange} {Codec}".into(),
        }
    }
}

/// "When the original language is one of `when_original`, keep audio in the
/// languages listed in `keep`, in that order." `"*"` matches any original
/// language; `"{original}"` in `keep` stands for the picked original language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioRule {
    pub when_original: Vec<String>,
    pub keep: Vec<String>,
}

pub const ORIGINAL_PLACEHOLDER: &str = "{original}";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioProfile {
    /// Tokens: Language, Format, Channels, Atmos, Commentary, Description.
    pub name_format: String,
    /// First matching rule wins.
    pub rules: Vec<AudioRule>,
    /// Keep commentary tracks in kept languages (named + flagged, never default).
    pub keep_commentary: bool,
    /// Keep audio-description tracks in kept languages (visual-impaired flag).
    pub keep_descriptions: bool,
    /// Track-title patterns, used in addition to the Matroska flags.
    pub commentary_pattern: String,
    pub description_pattern: String,
}

impl Default for AudioProfile {
    fn default() -> Self {
        let rule = |when: &[&str], keep: &[&str]| AudioRule {
            when_original: when.iter().map(|s| s.to_string()).collect(),
            keep: keep.iter().map(|s| s.to_string()).collect(),
        };
        Self {
            name_format: "{Language} {Format} {Channels}{ Atmos}{ Commentary}{ Description}".into(),
            rules: vec![
                rule(&["en", "hi", "te"], &[ORIGINAL_PLACEHOLDER]),
                rule(&["ta", "ml", "kn"], &["te", ORIGINAL_PLACEHOLDER, "hi"]),
                rule(&["*"], &[ORIGINAL_PLACEHOLDER, "en"]),
            ],
            keep_commentary: true,
            keep_descriptions: true,
            commentary_pattern: r"(?i)\bcommentary\b".into(),
            description_pattern: r"(?i)\b(audio[ ._-]?description|descriptive|described)\b|\bAD\b".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SubtitleKind {
    Forced,
    Full,
    #[serde(rename = "SDH")]
    Sdh,
    #[serde(rename = "CC")]
    Cc,
    Commentary,
    #[serde(rename = "Signs & Songs")]
    SignsSongs,
}

impl SubtitleKind {
    pub const ALL: [SubtitleKind; 6] = [
        SubtitleKind::Forced,
        SubtitleKind::Full,
        SubtitleKind::Sdh,
        SubtitleKind::Cc,
        SubtitleKind::Commentary,
        SubtitleKind::SignsSongs,
    ];

    /// Default `{Kind}` token value (full subtitles have none).
    pub fn label(self) -> &'static str {
        match self {
            SubtitleKind::Forced => "Forced",
            SubtitleKind::Full => "",
            SubtitleKind::Sdh => "SDH",
            SubtitleKind::Cc => "CC",
            SubtitleKind::Commentary => "Commentary",
            SubtitleKind::SignsSongs => "Signs & Songs",
        }
    }

    /// Stable key, as used in config (`order`, label overrides).
    pub fn key(self) -> &'static str {
        match self {
            SubtitleKind::Full => "Full",
            other => other.label(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleProfile {
    pub keep_languages: Vec<String>,
    /// Tokens: Language, Kind, Format.
    pub name_format: String,
    pub order: Vec<SubtitleKind>,
    /// Set the default flag on the first Forced subtitle when the picked
    /// original language differs from the subtitle's language.
    pub default_forced_when_foreign: bool,
    /// Treat a subtitle with far fewer events than another of the same
    /// language as Forced (when no flag/title says otherwise).
    pub detect_forced_by_element_count: bool,
    pub forced_pattern: String,
    pub sdh_pattern: String,
    pub cc_pattern: String,
    pub commentary_pattern: String,
    pub signs_pattern: String,
}

impl Default for SubtitleProfile {
    fn default() -> Self {
        Self {
            keep_languages: vec!["en".into()],
            name_format: "{Language}{ Kind}".into(),
            order: vec![
                SubtitleKind::Forced,
                SubtitleKind::Full,
                SubtitleKind::Sdh,
                SubtitleKind::Cc,
                SubtitleKind::SignsSongs,
                SubtitleKind::Commentary,
            ],
            default_forced_when_foreign: true,
            detect_forced_by_element_count: true,
            forced_pattern: r"(?i)\bforced\b|\bforeign\b".into(),
            sdh_pattern: r"(?i)\bsdh\b|hearing[ ._-]?impaired|\bHI\b".into(),
            cc_pattern: r"(?i)\bcc\b|closed[ ._-]?captions?".into(),
            commentary_pattern: r"(?i)\bcommentary\b".into(),
            signs_pattern: r"(?i)\bsigns?\b(\s*(&|and)\s*songs?)?".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomFormat {
    pub name: String,
    /// Regex matched against the original file name.
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamingConfig {
    pub movie_format: String,
    pub episode_format: String,
    pub colon_replacement: ColonReplacement,
    pub custom_formats: Vec<CustomFormat>,
}

pub const DEFAULT_MOVIE_FORMAT: &str = "{Movie CleanTitle} {(Release Year)} [tmdbid-{TmdbId}] - {Edition Tags }{[Custom Formats]}{[Quality Full]}{[MediaInfo 3D]}{[MediaInfo VideoDynamicRangeType]}{[Mediainfo AudioCodec}{ Mediainfo AudioChannels]}{[Mediainfo VideoCodec]}{-Release Group}";
pub const DEFAULT_EPISODE_FORMAT: &str = "{Series TitleYear} - S{season:00}E{episode:00} - {Episode CleanTitle} [{Custom Formats }{Quality Full}]{[MediaInfo VideoDynamicRangeType]}{[Mediainfo AudioCodec}{ Mediainfo AudioChannels]}{[MediaInfo VideoCodec]}{-Release Group}";

impl Default for NamingConfig {
    fn default() -> Self {
        let cf = |name: &str, pattern: &str| CustomFormat {
            name: name.into(),
            pattern: pattern.into(),
        };
        Self {
            movie_format: DEFAULT_MOVIE_FORMAT.into(),
            episode_format: DEFAULT_EPISODE_FORMAT.into(),
            colon_replacement: ColonReplacement::Smart,
            custom_formats: vec![
                cf("IMAX Enhanced", r"(?i)\bimax[ ._-]?enhanced\b"),
                cf("Hybrid", r"(?i)\bhybrid\b"),
            ],
        }
    }
}

/// Optional replacements for generated labels, keyed by the default label
/// (e.g. `dynamic_range."DV8 HDR10" = "DoVi P8"`, `audio_format."DD+" = "EAC3"`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabelOverrides {
    pub dynamic_range: BTreeMap<String, String>,
    pub video_codec: BTreeMap<String, String>,
    pub audio_format: BTreeMap<String, String>,
    pub subtitle_kind: BTreeMap<String, String>,
}

impl LabelOverrides {
    pub fn apply(map: &BTreeMap<String, String>, label: &str) -> String {
        map.get(label).cloned().unwrap_or_else(|| label.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid config file: {0}")]
    Parse(String),
    #[error("{}", .0.join("; "))]
    Invalid(Vec<String>),
}

impl AppConfig {
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: AppConfig = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::Parse(e.to_string()))
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Checks everything that would otherwise only fail later, mid-use:
    /// template syntax, unknown tokens, regexes, and rule shape.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut problems = Vec::new();

        if self.profiles.is_empty() {
            problems.push("at least one profile is required".to_string());
        }
        let mut names = std::collections::HashSet::new();
        for profile in &self.profiles {
            let p = &profile.name;
            if p.trim().is_empty() {
                problems.push("profile names must not be empty".into());
            } else if !names.insert(p.as_str()) {
                problems.push(format!("duplicate profile name `{p}`"));
            }

            check_template(&mut problems, &format!("{p}: video name format"), &profile.video.name_format,
                crate::apply::VIDEO_TOKENS);
            check_template(&mut problems, &format!("{p}: audio name format"), &profile.audio.name_format,
                crate::apply::AUDIO_TOKENS);
            check_template(&mut problems, &format!("{p}: subtitle name format"), &profile.subtitle.name_format,
                crate::apply::SUBTITLE_TOKENS);

            if profile.audio.rules.is_empty() {
                problems.push(format!("{p}: at least one audio rule is required"));
            }
            for (i, rule) in profile.audio.rules.iter().enumerate() {
                if rule.when_original.is_empty() || rule.keep.is_empty() {
                    problems.push(format!("{p}: audio rule #{} needs both `when_original` and `keep`", i + 1));
                }
            }
            for (label, pattern) in [
                ("audio commentary pattern", &profile.audio.commentary_pattern),
                ("audio description pattern", &profile.audio.description_pattern),
                ("subtitle forced pattern", &profile.subtitle.forced_pattern),
                ("subtitle SDH pattern", &profile.subtitle.sdh_pattern),
                ("subtitle CC pattern", &profile.subtitle.cc_pattern),
                ("subtitle commentary pattern", &profile.subtitle.commentary_pattern),
                ("subtitle signs pattern", &profile.subtitle.signs_pattern),
            ] {
                if let Err(e) = Regex::new(pattern) {
                    problems.push(format!("{p}: {label}: {e}"));
                }
            }
        }

        check_template(&mut problems, "movie format", &self.naming.movie_format, crate::naming::MOVIE_TOKENS);
        check_template(&mut problems, "episode format", &self.naming.episode_format, crate::naming::EPISODE_TOKENS);
        for cf in &self.naming.custom_formats {
            if cf.name.trim().is_empty() {
                problems.push("custom format names must not be empty".into());
            }
            if let Err(e) = Regex::new(&cf.pattern) {
                problems.push(format!("custom format `{}`: {e}", cf.name));
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Invalid(problems))
        }
    }
}

fn check_template(problems: &mut Vec<String>, what: &str, tpl: &str, known: &[&str]) {
    match template::referenced_tokens(tpl) {
        Err(e) => problems.push(format!("{what}: {e}")),
        Ok(tokens) => {
            let known: Vec<String> = known.iter().map(|k| template::token_key(k)).collect();
            for token in tokens {
                if !known.contains(&template::token_key(&token)) {
                    problems.push(format!("{what}: unknown token `{{{token}}}`"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_round_trips_through_toml() {
        let config = AppConfig::default();
        config.validate().unwrap();
        let text = config.to_toml().unwrap();
        let back = AppConfig::from_toml(&text).unwrap();
        assert_eq!(back, config);
        assert!(text.contains("when_original"));
        assert!(text.contains("{original}"));
    }

    #[test]
    fn partial_config_fills_defaults() {
        let config = AppConfig::from_toml(
            r#"
            [naming]
            colon_replacement = "space-dash-space"
            "#,
        )
        .unwrap();
        assert_eq!(config.naming.colon_replacement, ColonReplacement::SpaceDashSpace);
        assert_eq!(config.naming.movie_format, DEFAULT_MOVIE_FORMAT);
        assert_eq!(config.profiles.len(), 1);
    }

    #[test]
    fn subtitle_kinds_serialize_by_display_name() {
        let config = AppConfig::default();
        let text = config.to_toml().unwrap();
        assert!(text.contains("\"SDH\""));
        assert!(text.contains("\"Signs & Songs\""));
    }

    #[test]
    fn validation_catches_mistakes() {
        let mut config = AppConfig::default();
        config.profiles.push(Profile::default()); // duplicate name
        config.profiles[0].video.name_format = "{Resolution} {Bogus}".into();
        config.profiles[0].subtitle.sdh_pattern = "(unclosed".into();
        config.profiles[0].audio.rules.push(AudioRule { when_original: vec![], keep: vec![] });
        config.naming.movie_format = "{Movie Title".into();
        let ConfigError::Invalid(problems) = config.validate().unwrap_err() else {
            panic!("expected Invalid");
        };
        let all = problems.join("\n");
        assert!(all.contains("duplicate profile name"), "{all}");
        assert!(all.contains("unknown token `{Bogus}`"), "{all}");
        assert!(all.contains("subtitle SDH pattern"), "{all}");
        assert!(all.contains("audio rule #4"), "{all}");
        assert!(all.contains("movie format: unclosed"), "{all}");
    }
}
