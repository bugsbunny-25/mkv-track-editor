//! Radarr/Sonarr-compatible naming templates.
//!
//! Grammar (mirrors Radarr's `FileNameBuilder`):
//! - `{prefix Token Name:format suffix}` — the prefix may contain `- ._[(`,
//!   the suffix `- ._)]`. Both are emitted only when the token has a value,
//!   so `{[Quality Full]}` renders `[Bluray-1080p]` or nothing at all.
//! - Token words may be separated by space, `.`, `_` or `-`; a non-space
//!   separator also replaces spaces in the value (`{Movie.CleanTitle}` →
//!   `The.Matrix`).
//! - An ALL-CAPS token name upper-cases the value, all-lowercase lower-cases
//!   it; mixed case leaves it as-is.
//! - `:00` zero-pads numeric values.
//!
//! Extension: literal `[...]`, `(...)` and `{...}` groups *outside* tokens
//! are optional — if every token inside a group renders empty, the whole
//! group is dropped. That's what makes `[tmdbid-{TmdbId}]` vanish when no
//! TMDb ID is known, and `{edition-{Edition Tags}}` (Plex style) work.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<prefix>[- ._\[(]*)(?P<name>[A-Za-z0-9]+(?:[- ._][A-Za-z0-9]+)*?)(?::(?P<format>[A-Za-z0-9|+-]+))?(?P<suffix>[- ._)\]]*)$",
    )
    .expect("token regex is valid")
});

/// Normalizes a token name for lookup: `Movie.CleanTitle` / `movie cleantitle`
/// → `moviecleantitle`.
pub fn token_key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Token values keyed by normalized name. A key that's present with an
/// empty value is a *known* token that happens to be empty (its prefix and
/// suffix are dropped); a missing key is an unknown token.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens(HashMap<String, String>);

impl Tokens {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) -> &mut Self {
        self.0.insert(token_key(name), value.into());
        self
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(&token_key(name)).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColonReplacement {
    Delete,
    Dash,
    SpaceDash,
    SpaceDashSpace,
    /// `: ` → ` - `, any other `:` → `-` (Radarr's "Smart Replace").
    #[default]
    Smart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    /// Matroska track names: only whitespace/empty-bracket clean-up.
    TrackName,
    /// File names: additionally replaces characters that are illegal on
    /// common filesystems and caps the length.
    FileName { colon: ColonReplacement, extension: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// Token names that weren't recognized (rendered as empty).
    pub unknown_tokens: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("unclosed `{{` at position {0}")]
    Unclosed(usize),
    #[error("`{{{0}}}` is not a valid token")]
    InvalidToken(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Casing {
    AsIs,
    Upper,
    Lower,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenSpec {
    prefix: String,
    name: String,
    separator: Option<char>,
    format: Option<String>,
    suffix: String,
    casing: Casing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(char),
    Token(TokenSpec),
}

fn parse_token(content: &str) -> Option<TokenSpec> {
    let caps = TOKEN_RE.captures(content)?;
    let name = caps["name"].to_string();
    let separator = name.chars().find(|c| matches!(c, ' ' | '.' | '_' | '-'));
    let letters: Vec<char> = name.chars().filter(char::is_ascii_alphabetic).collect();
    let casing = if !letters.is_empty() && letters.iter().all(char::is_ascii_uppercase) {
        Casing::Upper
    } else if !letters.is_empty() && letters.iter().all(char::is_ascii_lowercase) {
        Casing::Lower
    } else {
        Casing::AsIs
    };
    Some(TokenSpec {
        prefix: caps["prefix"].to_string(),
        separator,
        format: caps.name("format").map(|m| m.as_str().to_string()),
        suffix: caps["suffix"].to_string(),
        casing,
        name,
    })
}

fn parse(template: &str) -> Result<Vec<Segment>, TemplateError> {
    let chars: Vec<char> = template.chars().collect();
    let mut segments = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            // `{` starts a token only if the next brace is a `}`; if another
            // `{` comes first, this one is a literal optional-group opener
            // (e.g. Plex's `{edition-{Edition Tags}}`).
            let next_brace = chars[i + 1..].iter().position(|c| *c == '{' || *c == '}');
            match next_brace {
                Some(offset) if chars[i + 1 + offset] == '}' => {
                    let content: String = chars[i + 1..i + 1 + offset].iter().collect();
                    let spec = parse_token(&content).ok_or_else(|| TemplateError::InvalidToken(content.clone()))?;
                    segments.push(Segment::Token(spec));
                    i += offset + 2;
                    continue;
                }
                Some(_) => {}
                None => return Err(TemplateError::Unclosed(i)),
            }
        }
        segments.push(Segment::Literal(chars[i]));
        i += 1;
    }
    Ok(segments)
}

/// Returns a description of the first syntax error, if any.
pub fn validate(template: &str) -> Result<(), TemplateError> {
    parse(template).map(|_| ())
}

/// Every token name referenced by `template` (in order, as written).
pub fn referenced_tokens(template: &str) -> Result<Vec<String>, TemplateError> {
    Ok(parse(template)?
        .into_iter()
        .filter_map(|s| match s {
            Segment::Token(t) => Some(t.name),
            Segment::Literal(_) => None,
        })
        .collect())
}

fn format_value(spec: &TokenSpec, raw: &str) -> String {
    let mut value = raw.trim().to_string();
    let numeric_format = spec
        .format
        .as_deref()
        .is_some_and(|f| !f.is_empty() && f.chars().all(|c| c == '0'));
    if let Some(format) = spec.format.as_deref().filter(|_| numeric_format) {
        // Zero-pad every number in the value, so an episode range
        // "1-E2" becomes "01-E02".
        static NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("valid"));
        value = NUM
            .replace_all(&value, |c: &regex::Captures| format!("{:0>width$}", &c[0], width = format.len()))
            .into_owned();
    }
    if let Some(sep) = spec.separator.filter(|s| *s != ' ') {
        value = value.split_whitespace().collect::<Vec<_>>().join(&sep.to_string());
    }
    // Sonarr's canonical `{season:00}`/`{episode:00}` are lower-case names
    // for numbers; lower-casing a multi-episode "01-E03" would be wrong.
    let numeric_token = numeric_format || matches!(token_key(&spec.name).as_str(), "season" | "episode");
    if numeric_token {
        return value;
    }
    match spec.casing {
        Casing::Upper => value.to_uppercase(),
        Casing::Lower => value.to_lowercase(),
        Casing::AsIs => value,
    }
}

struct Group {
    start: usize,
    has_token: bool,
    filled: bool,
}

fn opener_for(c: char) -> Option<char> {
    match c {
        ']' => Some('['),
        ')' => Some('('),
        '}' => Some('{'),
        _ => None,
    }
}

pub fn render(template: &str, tokens: &Tokens, mode: RenderMode) -> Result<Rendered, TemplateError> {
    let segments = parse(template)?;
    let mut out = String::new();
    let mut groups: Vec<(char, Group)> = Vec::new();
    let mut unknown = Vec::new();

    for segment in segments {
        match segment {
            Segment::Literal(c @ ('[' | '(' | '{')) => {
                groups.push((c, Group { start: out.len(), has_token: false, filled: false }));
                out.push(c);
            }
            Segment::Literal(c) if opener_for(c).is_some() => {
                let matches_top = groups.last().is_some_and(|(open, _)| Some(*open) == opener_for(c));
                if !matches_top {
                    out.push(c);
                    continue;
                }
                let (_, group) = groups.pop().expect("checked non-empty");
                if group.has_token && !group.filled {
                    out.truncate(group.start);
                } else {
                    out.push(c);
                }
            }
            Segment::Literal(c) => out.push(c),
            Segment::Token(spec) => {
                let value = match tokens.get(&spec.name) {
                    Some(v) => format_value(&spec, v),
                    None => {
                        unknown.push(spec.name.clone());
                        String::new()
                    }
                };
                for (_, g) in groups.iter_mut() {
                    g.has_token = true;
                    if !value.is_empty() {
                        g.filled = true;
                    }
                }
                if !value.is_empty() {
                    out.push_str(&spec.prefix);
                    out.push_str(&value);
                    out.push_str(&spec.suffix);
                }
            }
        }
    }

    let text = match mode {
        RenderMode::TrackName => tidy(&out),
        RenderMode::FileName { colon, extension } => sanitize_file_name(&tidy(&out), colon, extension),
    };
    Ok(Rendered { text, unknown_tokens: unknown })
}

static EMPTY_BRACKETS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\s*\)|\[\s*\]|\{\s*\}").expect("valid"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid"));
static REPEATED_DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s-(?:\s-)+\s").expect("valid"));

/// Collapses whitespace, removes empty bracket pairs and dangling
/// separators left behind by empty tokens.
fn tidy(s: &str) -> String {
    let mut s = s.to_string();
    // Removing an empty `[]` can expose another (`[ [] ]`), so iterate.
    loop {
        let next = EMPTY_BRACKETS.replace_all(&s, "").into_owned();
        if next == s {
            break;
        }
        s = next;
    }
    let s = SPACES.replace_all(&s, " ");
    let s = REPEATED_DASH.replace_all(&s, " - ");
    s.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '.' | '_'))
        .to_string()
}

pub fn sanitize_file_name(stem: &str, colon: ColonReplacement, extension: &str) -> String {
    let with_colons = match colon {
        ColonReplacement::Delete => stem.replace(':', ""),
        ColonReplacement::Dash => stem.replace(':', "-"),
        ColonReplacement::SpaceDash => stem.replace(':', " -"),
        ColonReplacement::SpaceDashSpace => stem.replace(':', " - "),
        ColonReplacement::Smart => stem.replace(": ", " - ").replace(':', "-"),
    };
    // Radarr's bad → good character table.
    let mut cleaned = String::with_capacity(with_colons.len());
    for c in with_colons.chars() {
        match c {
            '\\' | '/' => cleaned.push('+'),
            '<' | '>' | '|' | '"' => {}
            '?' => cleaned.push('!'),
            '*' => cleaned.push('-'),
            c if c.is_control() => {}
            c => cleaned.push(c),
        }
    }
    let mut stem = tidy(&cleaned);

    // Filesystems cap names at 255 bytes; truncate on a char boundary.
    let max_stem = 255usize.saturating_sub(extension.len());
    if stem.len() > max_stem {
        let mut cut = max_stem;
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        stem.truncate(cut);
        stem = tidy(&stem);
    }
    // Windows can't have trailing dots or spaces.
    let stem = stem.trim_end_matches(['.', ' ']);
    format!("{stem}{extension}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOVIE: &str = "{Movie CleanTitle} {(Release Year)} [tmdbid-{TmdbId}] - {Edition Tags }{[Custom Formats]}{[Quality Full]}{[MediaInfo 3D]}{[MediaInfo VideoDynamicRangeType]}{[Mediainfo AudioCodec}{ Mediainfo AudioChannels]}{[Mediainfo VideoCodec]}{-Release Group}";
    const EPISODE: &str = "{Series TitleYear} - S{season:00}E{episode:00} - {Episode CleanTitle} [{Custom Formats }{Quality Full}]{[MediaInfo VideoDynamicRangeType]}{[Mediainfo AudioCodec}{ Mediainfo AudioChannels]}{[MediaInfo VideoCodec]}{-Release Group}";
    const FILE: RenderMode = RenderMode::FileName { colon: ColonReplacement::Smart, extension: ".mkv" };

    fn movie_tokens() -> Tokens {
        let mut t = Tokens::new();
        t.set("Movie CleanTitle", "The Matrix")
            .set("Release Year", "1999")
            .set("TmdbId", "603")
            .set("Edition Tags", "")
            .set("Custom Formats", "")
            .set("Quality Full", "Remux-2160p")
            .set("MediaInfo 3D", "")
            .set("MediaInfo VideoDynamicRangeType", "DV HDR10")
            .set("MediaInfo AudioCodec", "TrueHD Atmos")
            .set("MediaInfo AudioChannels", "7.1")
            .set("MediaInfo VideoCodec", "h265")
            .set("Release Group", "FraMeSToR");
        t
    }

    fn file(template: &str, tokens: &Tokens) -> String {
        let r = render(template, tokens, FILE).unwrap();
        assert!(r.unknown_tokens.is_empty(), "unknown: {:?}", r.unknown_tokens);
        r.text
    }

    #[test]
    fn users_movie_format_full() {
        assert_eq!(
            file(MOVIE, &movie_tokens()),
            "The Matrix (1999) [tmdbid-603] - [Remux-2160p][DV HDR10][TrueHD Atmos 7.1][h265]-FraMeSToR.mkv"
        );
    }

    #[test]
    fn users_movie_format_without_tmdb_edition_and_group() {
        let mut t = movie_tokens();
        t.set("TmdbId", "").set("Release Group", "").set("Edition Tags", "Director's Cut");
        t.set("Custom Formats", "IMAX Enhanced Hybrid");
        assert_eq!(
            file(MOVIE, &t),
            "The Matrix (1999) - Director's Cut [IMAX Enhanced Hybrid][Remux-2160p][DV HDR10][TrueHD Atmos 7.1][h265].mkv"
        );
    }

    #[test]
    fn users_movie_format_mostly_empty() {
        let mut t = Tokens::new();
        for name in [
            "TmdbId", "Edition Tags", "Custom Formats", "Quality Full", "MediaInfo 3D",
            "MediaInfo VideoDynamicRangeType", "MediaInfo AudioCodec", "MediaInfo AudioChannels",
            "MediaInfo VideoCodec", "Release Group", "Release Year",
        ] {
            t.set(name, "");
        }
        t.set("Movie CleanTitle", "Some Movie");
        assert_eq!(file(MOVIE, &t), "Some Movie.mkv");
    }

    #[test]
    fn users_episode_format() {
        let mut t = Tokens::new();
        t.set("Series TitleYear", "Doctor Who (2005)")
            .set("season", "1")
            .set("episode", "2")
            .set("Episode CleanTitle", "The End of the World")
            .set("Custom Formats", "")
            .set("Quality Full", "WEBDL-1080p Proper")
            .set("MediaInfo VideoDynamicRangeType", "")
            .set("MediaInfo AudioCodec", "EAC3")
            .set("MediaInfo AudioChannels", "5.1")
            .set("MediaInfo VideoCodec", "h264")
            .set("Release Group", "NTb");
        assert_eq!(
            file(EPISODE, &t),
            "Doctor Who (2005) - S01E02 - The End of the World [WEBDL-1080p Proper][EAC3 5.1][h264]-NTb.mkv"
        );

        // Both tokens in the literal `[...]` empty → the group disappears.
        t.set("Quality Full", "").set("episode", "2-E3");
        assert_eq!(
            file(EPISODE, &t),
            "Doctor Who (2005) - S01E02-E03 - The End of the World [EAC3 5.1][h264]-NTb.mkv"
        );
    }

    #[test]
    fn separators_and_casing() {
        let mut t = Tokens::new();
        t.set("Movie CleanTitle", "The Matrix Reloaded").set("Release Year", "2003");
        let dotted = render("{Movie.CleanTitle}{.Release.Year}", &t, FILE).unwrap().text;
        assert_eq!(dotted, "The.Matrix.Reloaded.2003.mkv");
        let upper = render("{MOVIE CLEANTITLE}", &t, FILE).unwrap().text;
        assert_eq!(upper, "THE MATRIX RELOADED.mkv");
        let lower = render("{movie cleantitle}", &t, FILE).unwrap().text;
        assert_eq!(lower, "the matrix reloaded.mkv");
        let under = render("{Movie_CleanTitle}", &t, RenderMode::TrackName).unwrap().text;
        assert_eq!(under, "The_Matrix_Reloaded");
    }

    #[test]
    fn plex_edition_group() {
        let mut t = Tokens::new();
        t.set("Movie Title", "Blade Runner").set("Edition Tags", "Final Cut");
        assert_eq!(
            render("{Movie Title} {edition-{Edition Tags}}", &t, RenderMode::TrackName).unwrap().text,
            "Blade Runner {edition-Final Cut}"
        );
        t.set("Edition Tags", "");
        assert_eq!(
            render("{Movie Title} {edition-{Edition Tags}}", &t, RenderMode::TrackName).unwrap().text,
            "Blade Runner"
        );
    }

    #[test]
    fn literal_groups_without_tokens_are_kept() {
        let t = Tokens::new();
        assert_eq!(render("[Fixed] Name", &t, RenderMode::TrackName).unwrap().text, "[Fixed] Name");
    }

    #[test]
    fn track_name_templates() {
        let mut t = Tokens::new();
        t.set("Language", "English").set("Format", "DD+").set("Channels", "5.1").set("Atmos", "Atmos");
        let tpl = "{Language} {Format} {Channels}{ Atmos}{ Commentary}";
        t.set("Commentary", "");
        assert_eq!(render(tpl, &t, RenderMode::TrackName).unwrap().text, "English DD+ 5.1 Atmos");
        t.set("Atmos", "").set("Commentary", "Commentary");
        assert_eq!(render(tpl, &t, RenderMode::TrackName).unwrap().text, "English DD+ 5.1 Commentary");

        let mut v = Tokens::new();
        v.set("Resolution", "2160p").set("DynamicRange", "DV8 HDR10").set("Codec", "HEVC");
        assert_eq!(
            render("{Resolution} {DynamicRange} {Codec}", &v, RenderMode::TrackName).unwrap().text,
            "2160p DV8 HDR10 HEVC"
        );
    }

    #[test]
    fn colon_and_illegal_characters() {
        let mut t = Tokens::new();
        t.set("Movie Title", "Mission: Impossible - Dead Reckoning Part One? <Yes>/No*");
        let tpl = "{Movie Title}";
        let smart = render(tpl, &t, FILE).unwrap().text;
        // `*` → `-`, then the dangling trailing dash is tidied away.
        assert_eq!(smart, "Mission - Impossible - Dead Reckoning Part One! Yes+No.mkv");
        let delete = render(tpl, &t, RenderMode::FileName { colon: ColonReplacement::Delete, extension: ".mkv" })
            .unwrap()
            .text;
        assert!(delete.starts_with("Mission Impossible"));
        let dash = render("{Movie Title}", &Tokens::new().set("Movie Title", "10:30").clone(), RenderMode::FileName {
            colon: ColonReplacement::Dash,
            extension: ".mkv",
        })
        .unwrap()
        .text;
        assert_eq!(dash, "10-30.mkv");
    }

    #[test]
    fn caps_length_on_char_boundary() {
        let mut t = Tokens::new();
        t.set("Movie Title", "é".repeat(200));
        let name = render("{Movie Title}", &t, FILE).unwrap().text;
        assert!(name.len() <= 255, "{}", name.len());
        assert!(name.ends_with(".mkv"));
    }

    #[test]
    fn reports_unknown_tokens_and_syntax_errors() {
        let r = render("{Movie Title} {Bogus Token}", &Tokens::new().set("Movie Title", "X").clone(), FILE).unwrap();
        assert_eq!(r.unknown_tokens, vec!["Bogus Token".to_string()]);
        assert_eq!(r.text, "X.mkv");
        assert!(matches!(validate("{Movie Title"), Err(TemplateError::Unclosed(0))));
        assert!(matches!(validate("{!!}"), Err(TemplateError::InvalidToken(_))));
        assert_eq!(
            referenced_tokens("{a} x {[B c]}").unwrap(),
            vec!["a".to_string(), "B c".to_string()]
        );
    }
}
