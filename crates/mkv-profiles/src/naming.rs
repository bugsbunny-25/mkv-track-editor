//! Radarr/Sonarr-style output file names: the per-file metadata the user
//! can edit, the token set built from it plus the (post-profile) media info,
//! and rendering through the user's configured format.

use crate::config::NamingConfig;
use crate::labels::{audio_attrs, video_attrs, AudioAttrs, VideoAttrs};
use crate::lang;
use crate::release::{parse_release_name, MediaKind, QualitySource, Revision};
use crate::template::{render, RenderMode, Rendered, TemplateError, Tokens};
use mkv_types::{is_image_codec, FileAnalysis, TrackEdit, TrackType};
use regex::Regex;
use serde::{Deserialize, Serialize};

const MEDIAINFO_TOKEN_NAMES: [&str; 13] = [
    "MediaInfo 3D",
    "MediaInfo Simple",
    "MediaInfo Full",
    "MediaInfo VideoCodec",
    "MediaInfo VideoBitDepth",
    "MediaInfo VideoDynamicRange",
    "MediaInfo VideoDynamicRangeType",
    "MediaInfo AudioCodec",
    "MediaInfo AudioChannels",
    "MediaInfo AudioLanguages",
    "MediaInfo AudioLanguagesAll",
    "MediaInfo SubtitleLanguages",
    "MediaInfo SubtitleLanguagesAll",
];

pub const MOVIE_TOKENS: &[&str] = &[
    "Movie Title", "Movie CleanTitle", "Movie TitleThe", "Movie CleanTitleThe", "Movie OriginalTitle",
    "Release Year", "Movie Year", "TmdbId", "ImdbId", "Edition Tags", "Custom Formats", "Quality Full",
    "Quality Title", "Release Group", "Original Title", "Original Filename",
    "MediaInfo 3D", "MediaInfo Simple", "MediaInfo Full", "MediaInfo VideoCodec", "MediaInfo VideoBitDepth",
    "MediaInfo VideoDynamicRange", "MediaInfo VideoDynamicRangeType", "MediaInfo AudioCodec",
    "MediaInfo AudioChannels", "MediaInfo AudioLanguages", "MediaInfo AudioLanguagesAll",
    "MediaInfo SubtitleLanguages", "MediaInfo SubtitleLanguagesAll",
];

pub const EPISODE_TOKENS: &[&str] = &[
    "Series Title", "Series CleanTitle", "Series TitleYear", "Series CleanTitleYear", "Series TitleThe",
    "Series Year", "season", "episode", "Episode Title", "Episode CleanTitle", "TvdbId", "TmdbId", "ImdbId",
    "Custom Formats", "Quality Full", "Quality Title", "Release Group", "Original Title", "Original Filename",
    "MediaInfo 3D", "MediaInfo Simple", "MediaInfo Full", "MediaInfo VideoCodec", "MediaInfo VideoBitDepth",
    "MediaInfo VideoDynamicRange", "MediaInfo VideoDynamicRangeType", "MediaInfo AudioCodec",
    "MediaInfo AudioChannels", "MediaInfo AudioLanguages", "MediaInfo AudioLanguagesAll",
    "MediaInfo SubtitleLanguages", "MediaInfo SubtitleLanguagesAll",
];

/// Everything about a file's identity that feeds the name. Prefilled from
/// the release-name parser; the user edits it (and supplies the TMDb ID).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NamingMetadata {
    pub kind: MediaKind,
    /// Movie title or series title.
    pub title: String,
    /// Movie release year or series year.
    pub year: Option<u16>,
    pub tmdb_id: String,
    pub imdb_id: String,
    pub tvdb_id: String,
    pub edition: String,
    /// Space-separated custom format names, including the streaming
    /// service's tag (e.g. `Hybrid AMZN`).
    pub custom_formats: String,
    pub source: QualitySource,
    pub revision: Revision,
    pub release_group: String,
    pub season: Option<u32>,
    pub episodes: Vec<u32>,
    pub episode_title: String,
    /// Source file name without extension (`{Original Filename}`).
    pub original_file_name: String,
}

impl NamingMetadata {
    /// Prefills from the source file name.
    pub fn from_file_name(file_name: &str, naming: &NamingConfig) -> Self {
        let info = parse_release_name(file_name);
        let stem = file_name
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(file_name)
            .to_string();
        Self {
            kind: info.kind,
            title: info.title,
            year: info.year,
            edition: info.editions.join(" "),
            custom_formats: detect_custom_formats(naming, file_name).join(" "),
            source: info.source,
            revision: info.revision,
            release_group: info.release_group.unwrap_or_default(),
            season: info.season,
            episodes: info.episodes,
            episode_title: info.episode_title,
            original_file_name: stem,
            ..Default::default()
        }
    }
}

/// The custom formats whose patterns match `file_name`, followed by the
/// streaming service's tag (as Radarr lists a service's custom format).
pub fn detect_custom_formats(naming: &NamingConfig, file_name: &str) -> Vec<String> {
    let mut found: Vec<String> = naming
        .custom_formats
        .iter()
        .filter(|cf| Regex::new(&cf.pattern).is_ok_and(|re| re.is_match(file_name)))
        .map(|cf| cf.name.clone())
        .collect();
    if let Some(tag) = detect_streaming_service(naming, file_name) {
        if !found.contains(&tag) {
            found.push(tag);
        }
    }
    found
}

/// The tag of the first streaming-service rule that matches `file_name`.
pub fn detect_streaming_service(naming: &NamingConfig, file_name: &str) -> Option<String> {
    naming
        .streaming_services
        .iter()
        .find(|s| s.regex().is_ok_and(|re| re.is_match(file_name).unwrap_or(false)))
        .map(|s| s.tag.trim().to_string())
}

/// Media facts for the `{MediaInfo ...}` tokens, computed from the tracks
/// that will actually be in the output (after profile / manual edits).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaSummary {
    pub video: Option<VideoAttrs>,
    /// First kept audio track — Radarr's "primary" audio stream.
    pub audio: Option<AudioAttrs>,
    pub audio_languages: Vec<String>,
    pub subtitle_languages: Vec<String>,
}

pub fn summarize(analysis: &FileAnalysis, kept: &[TrackEdit]) -> MediaSummary {
    let codec_of = |uid: u64| {
        analysis
            .probe
            .tracks
            .iter()
            .find(|t| t.uid == uid)
            .map(|t| t.codec.clone())
            .unwrap_or_default()
    };
    let first = |t: TrackType| kept.iter().find(|e| e.track_type == t);
    // A kept cover-art track never describes the video.
    let first_video = kept
        .iter()
        .find(|e| e.track_type == TrackType::Video && !is_image_codec(&codec_of(e.uid)))
        .or_else(|| first(TrackType::Video));
    let languages = |t: TrackType| {
        let mut out: Vec<String> = Vec::new();
        for edit in kept.iter().filter(|e| e.track_type == t) {
            if let Some(code) = edit.language.as_deref().filter(|l| !lang::is_undetermined(l)) {
                let code = lang::canonical(code);
                if !out.contains(&code) {
                    out.push(code);
                }
            }
        }
        out
    };
    MediaSummary {
        video: first_video.map(|e| video_attrs(analysis.media.track(e.uid), &codec_of(e.uid))),
        audio: first(TrackType::Audio).map(|e| audio_attrs(analysis.media.track(e.uid), &codec_of(e.uid))),
        audio_languages: languages(TrackType::Audio),
        subtitle_languages: languages(TrackType::Subtitles),
    }
}

/// Radarr's `CleanTitle`: `&` → "and", strips punctuation that's awkward in
/// file names (`:`, `'`, `?`, brackets, ...) and diacritics.
pub fn clean_title(title: &str) -> String {
    let replaced = title.replace('&', "and").replace(['/', '\\'], " ");
    let stripped: String = remove_diacritics(&replaced)
        .chars()
        .filter(|c| !matches!(c, ':' | '\'' | '`' | '?' | ',' | '!' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ';' | '*' | '<' | '>' | '|' | '~' | '@' | '$' | '%' | '^' | '=' | '’'))
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "The Matrix" → "Matrix, The".
pub fn title_the(title: &str) -> String {
    for article in ["The ", "A ", "An "] {
        if let Some(rest) = title.strip_prefix(article) {
            return format!("{rest}, {}", article.trim());
        }
    }
    title.to_string()
}

fn remove_diacritics(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' => 'A',
            'ç' | 'ć' | 'č' => 'c',
            'Ç' | 'Ć' | 'Č' => 'C',
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' => 'e',
            'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ę' => 'E',
            'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
            'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' => 'I',
            'ñ' | 'ń' => 'n',
            'Ñ' | 'Ń' => 'N',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' => 'O',
            'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
            'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' => 'U',
            'ý' | 'ÿ' => 'y',
            'Ý' => 'Y',
            'š' | 'ś' => 's',
            'Š' | 'Ś' => 'S',
            'ž' | 'ź' | 'ż' => 'z',
            'Ž' | 'Ź' | 'Ż' => 'Z',
            'ł' => 'l',
            'Ł' => 'L',
            other => other,
        })
        .collect()
}

/// `[EN+HI]`, or empty when the only language is English (Radarr's rule
/// for `{MediaInfo AudioLanguages}`); `all` keeps English-only too.
fn languages_token(codes: &[String], all: bool) -> String {
    if codes.is_empty() || (!all && codes.len() == 1 && codes[0] == "en") {
        return String::new();
    }
    let upper: Vec<String> = codes.iter().map(|c| c.to_uppercase()).collect();
    format!("[{}]", upper.join("+"))
}

pub fn file_tokens(meta: &NamingMetadata, summary: &MediaSummary) -> Tokens {
    let mut t = Tokens::new();
    let year = meta.year.map(|y| y.to_string()).unwrap_or_default();
    let clean = clean_title(&meta.title);
    let resolution = summary.video.as_ref().and_then(|v| v.resolution.clone());
    let quality_title = meta.source.quality_title(resolution.as_deref());
    let quality_full = if quality_title.is_empty() {
        String::new()
    } else {
        format!("{quality_title} {}", meta.revision.label()).trim().to_string()
    };

    let video_codec = summary.video.as_ref().map(|v| v.radarr_codec.clone()).unwrap_or_default();
    let audio_codec = summary.audio.as_ref().map(|a| a.radarr_codec.clone()).unwrap_or_default();
    let audio_languages = languages_token(&summary.audio_languages, false);
    let simple = format!("{video_codec} {audio_codec}").trim().to_string();

    // Movie
    t.set("Movie Title", meta.title.clone())
        .set("Movie CleanTitle", clean.clone())
        .set("Movie TitleThe", title_the(&meta.title))
        .set("Movie CleanTitleThe", title_the(&clean))
        .set("Movie OriginalTitle", meta.title.clone())
        .set("Release Year", year.clone())
        .set("Movie Year", year.clone())
        .set("Edition Tags", meta.edition.clone());
    // Series / episode
    let already_has_year = !year.is_empty() && meta.title.trim_end_matches(')').ends_with(&year);
    let title_year = if year.is_empty() || already_has_year {
        meta.title.clone()
    } else {
        format!("{} ({year})", meta.title)
    };
    let clean_title_year = if year.is_empty() || already_has_year {
        clean.clone()
    } else {
        format!("{clean} {year}")
    };
    let episode = match meta.episodes.as_slice() {
        [] => String::new(),
        [single] => single.to_string(),
        [first, .., last] => format!("{first}-E{last}"),
    };
    t.set("Series Title", meta.title.clone())
        .set("Series CleanTitle", clean.clone())
        .set("Series TitleYear", title_year)
        .set("Series CleanTitleYear", clean_title_year)
        .set("Series TitleThe", title_the(&meta.title))
        .set("Series Year", year)
        .set("season", meta.season.map(|s| s.to_string()).unwrap_or_default())
        .set("episode", episode)
        .set("Episode Title", meta.episode_title.clone())
        .set("Episode CleanTitle", clean_title(&meta.episode_title));
    // Shared
    t.set("TmdbId", meta.tmdb_id.trim())
        .set("ImdbId", meta.imdb_id.trim())
        .set("TvdbId", meta.tvdb_id.trim())
        .set("Custom Formats", meta.custom_formats.clone())
        .set("Quality Full", quality_full)
        .set("Quality Title", quality_title)
        .set("Release Group", meta.release_group.clone())
        .set("Original Title", meta.original_file_name.clone())
        .set("Original Filename", meta.original_file_name.clone());
    // MediaInfo
    let video = summary.video.as_ref();
    let audio = summary.audio.as_ref();
    t.set("MediaInfo 3D", if video.is_some_and(|v| v.is_3d) { "3D" } else { "" })
        .set("MediaInfo Simple", simple.clone())
        .set("MediaInfo Full", format!("{simple} {audio_languages}").trim().to_string())
        .set("MediaInfo VideoCodec", video_codec)
        .set("MediaInfo VideoBitDepth", video.and_then(|v| v.bit_depth).map(|b| b.to_string()).unwrap_or_default())
        .set("MediaInfo VideoDynamicRange", video.map(|v| v.dynamic_range.radarr_range()).unwrap_or(""))
        .set("MediaInfo VideoDynamicRangeType", video.map(|v| v.dynamic_range.radarr_type()).unwrap_or_default())
        .set("MediaInfo AudioCodec", audio_codec)
        .set("MediaInfo AudioChannels", audio.and_then(|a| a.channels.clone()).unwrap_or_default())
        .set("MediaInfo AudioLanguages", audio_languages)
        .set("MediaInfo AudioLanguagesAll", languages_token(&summary.audio_languages, true))
        .set("MediaInfo SubtitleLanguages", languages_token(&summary.subtitle_languages, false))
        .set("MediaInfo SubtitleLanguagesAll", languages_token(&summary.subtitle_languages, true));
    debug_assert!(MEDIAINFO_TOKEN_NAMES.iter().all(|n| t.get(n).is_some()));
    t
}

/// Renders the output file name (with `.mkv`) using the movie or episode
/// format depending on `meta.kind`.
pub fn render_file_name(
    naming: &NamingConfig,
    meta: &NamingMetadata,
    summary: &MediaSummary,
) -> Result<Rendered, TemplateError> {
    let template = match meta.kind {
        MediaKind::Movie => &naming.movie_format,
        MediaKind::Episode => &naming.episode_format,
    };
    let mut rendered = render(
        template,
        &file_tokens(meta, summary),
        RenderMode::FileName {
            colon: naming.colon_replacement,
            extension: ".mkv",
        },
    )?;
    if rendered.text == ".mkv" {
        // Nothing to name it by; fall back to the original name.
        rendered.text = format!("{}.mkv", meta.original_file_name);
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::{DynamicRange, HdrBase};

    fn summary() -> MediaSummary {
        MediaSummary {
            video: Some(VideoAttrs {
                resolution: Some("2160p".into()),
                dynamic_range: DynamicRange { dolby_vision: Some(8), base: Some(HdrBase::Hdr10) },
                codec: "HEVC".into(),
                radarr_codec: "h265".into(),
                bit_depth: Some(10),
                is_3d: false,
            }),
            audio: Some(AudioAttrs {
                format: "Dolby TrueHD".into(),
                radarr_codec: "TrueHD Atmos".into(),
                atmos: true,
                channels: Some("7.1".into()),
            }),
            audio_languages: vec!["en".into()],
            subtitle_languages: vec!["en".into()],
        }
    }

    #[test]
    fn movie_end_to_end_from_file_name() {
        let naming = NamingConfig::default();
        let mut meta = NamingMetadata::from_file_name(
            "The.Matrix.1999.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.7.1.Atmos-FraMeSToR.mkv",
            &naming,
        );
        assert_eq!(meta.kind, MediaKind::Movie);

        let without_tmdb = render_file_name(&naming, &meta, &summary()).unwrap();
        assert_eq!(
            without_tmdb.text,
            "The Matrix (1999) - [Remux-2160p][DV HDR10][TrueHD Atmos 7.1][h265]-FraMeSToR.mkv"
        );
        assert!(without_tmdb.unknown_tokens.is_empty());

        meta.tmdb_id = "603".into();
        assert_eq!(
            render_file_name(&naming, &meta, &summary()).unwrap().text,
            "The Matrix (1999) [tmdbid-603] - [Remux-2160p][DV HDR10][TrueHD Atmos 7.1][h265]-FraMeSToR.mkv"
        );
    }

    #[test]
    fn movie_with_edition_custom_formats_and_colon() {
        let naming = NamingConfig::default();
        let mut meta = NamingMetadata::from_file_name(
            "Mission.Impossible.Dead.Reckoning.Part.One.2023.IMAX.Enhanced.Hybrid.2160p.WEB-DL.DDP5.1.Atmos.DV.HDR.H.265-FLUX.mkv",
            &naming,
        );
        assert_eq!(meta.custom_formats, "IMAX Enhanced Hybrid");
        meta.title = "Mission: Impossible – Dead Reckoning Part One".into();
        meta.edition = "Director's Cut".into();
        meta.tmdb_id = "575264".into();
        let name = render_file_name(&naming, &meta, &summary()).unwrap().text;
        // CleanTitle drops the colon; Edition Tags are used verbatim.
        assert_eq!(
            name,
            "Mission Impossible – Dead Reckoning Part One (2023) [tmdbid-575264] - Director's Cut [IMAX Enhanced Hybrid][WEBDL-2160p][DV HDR10][TrueHD Atmos 7.1][h265]-FLUX.mkv"
        );
    }

    #[test]
    fn streaming_service_is_added_to_custom_formats() {
        let naming = NamingConfig::default();
        let detect = |name: &str| detect_streaming_service(&naming, name);
        let cases = [
            ("Movie.2023.2160p.AMZN.WEB-DL.DDP5.1.H.265-GRP.mkv", Some("AMZN")),
            ("Show.S01E01.1080p.NF.WEB-DL.mkv", Some("NF")),
            ("Movie.2023.1080p.ATVP.WEB-DL.mkv", Some("ATVP")),
            ("Movie.2023.1080p.ATV.WEB-DL.mkv", Some("ATV")),
            ("Movie.2021.2160p.HMAX.WEB-DL.mkv", Some("HMAX")),
            ("Movie.2021.2160p.HBO.MAX.WEB-DL.mkv", Some("HMAX")),
            ("Movie.2021.1080p.HBO.WEB-DL.mkv", Some("HBO")),
            ("Movie.2024.2160p.MAX.WEB-DL.mkv", Some("MAX")),
            ("Movie.2019.1080p.iT.WEB-DL.mkv", Some("iT")),
            ("Movie.2019.1080p.MA.WEB-DL.mkv", Some("MA")),
            ("Movie.2019.1080p.Hulu.WEB-DL.mkv", Some("Hulu")),
            ("Show.S01E01.1080p.iP.WEB-DL.mkv", Some("IP")),
            ("Show.S01E01.1080p.ITV.WEB-DL.mkv", Some("ITVX")),
            ("Show.S01E01.1080p.ITVX.WEB-DL.mkv", Some("ITVX")),
            ("Show.S01E01.1080p.MY5.WEB-DL.mkv", Some("MY5")),
            ("Show.S01E01.1080p.NOW.WEB-DL.mkv", Some("NOW")),
            ("Apocalypse.Now.1979.2160p.BluRay.mkv", None),
            ("Movie.2024.Telugu.1080p.JHS.WEB-DL.mkv", Some("HTSR")),
            ("Movie.2024.Telugu.1080p.JioHotstar.WEB-DL.mkv", Some("HTSR")),
            ("Movie.2022.Telugu.1080p.HS.WEB-DL.mkv", Some("HTSR")),
            ("Movie.2022.Telugu.1080p.Hotstar.WEB-DL.mkv", Some("HTSR")),
            ("Movie.2024.Telugu.1080p.SLIV.WEB-DL.mkv", Some("SONYLIV")),
            ("Movie.2024.Tamil.1080p.SNXT.WEB-DL.mkv", Some("SUNNXT")),
            ("Movie.2024.Telugu.1080p.AHA.WEB-DL.mkv", Some("AHA")),
            // Already named by us: `[TAG]` / `[... TAG]`.
            ("Movie (2024) - [MAX][WEBDL-2160p].mkv", Some("MAX")),
            ("Movie (2024) - [Hybrid iT][WEBDL-1080p].mkv", Some("iT")),
            // Titles and audio that look like services don't count.
            ("Mad.Max.Fury.Road.2015.2160p.BluRay.mkv", None),
            ("It.2017.2160p.BluRay.mkv", None),
            ("Movie.2019.2160p.BluRay.REMUX.DTS-HD.MA.5.1-GRP.mkv", None),
        ];
        for (name, tag) in cases {
            assert_eq!(detect(name).as_deref(), tag, "{name}");
        }

        let meta = NamingMetadata::from_file_name("Movie.2023.Hybrid.2160p.AMZN.WEB-DL.DV.mkv", &naming);
        assert_eq!(meta.custom_formats, "Hybrid AMZN");
        let name = render_file_name(&naming, &meta, &summary()).unwrap().text;
        assert_eq!(name, "Movie (2023) - [Hybrid AMZN][WEBDL-2160p][DV HDR10][TrueHD Atmos 7.1][h265].mkv");
    }

    #[test]
    fn episode_end_to_end() {
        let naming = NamingConfig::default();
        let mut meta = NamingMetadata::from_file_name(
            "Doctor.Who.2005.S01E02.1080p.BluRay.x264-SHORTBREHD.mkv",
            &naming,
        );
        assert_eq!(meta.kind, MediaKind::Episode);
        meta.episode_title = "The End of the World".into();
        let mut s = summary();
        s.video.as_mut().unwrap().resolution = Some("1080p".into());
        s.video.as_mut().unwrap().dynamic_range = DynamicRange::SDR;
        s.video.as_mut().unwrap().radarr_codec = "x264".into();
        s.audio.as_mut().unwrap().radarr_codec = "DTS".into();
        s.audio.as_mut().unwrap().channels = Some("5.1".into());
        assert_eq!(
            render_file_name(&naming, &meta, &s).unwrap().text,
            "Doctor Who (2005) - S01E02 - The End of the World [Bluray-1080p][DTS 5.1][x264]-SHORTBREHD.mkv"
        );
    }

    #[test]
    fn multi_episode_uses_prefixed_range() {
        let naming = NamingConfig::default();
        let meta = NamingMetadata::from_file_name("Show.S03E10E11.720p.WEB-DL.mkv", &naming);
        let name = render_file_name(&naming, &meta, &MediaSummary::default()).unwrap().text;
        assert!(name.starts_with("Show - S03E10-E11"), "{name}");
    }

    #[test]
    fn clean_title_matches_radarr_style() {
        assert_eq!(clean_title("Mission: Impossible"), "Mission Impossible");
        assert_eq!(clean_title("Don't Look Up"), "Dont Look Up");
        assert_eq!(clean_title("Fast & Furious"), "Fast and Furious");
        assert_eq!(clean_title("Amélie"), "Amelie");
        assert_eq!(clean_title("Spider-Man: No Way Home"), "Spider-Man No Way Home");
        assert_eq!(title_the("The Matrix"), "Matrix, The");
    }

    #[test]
    fn language_tokens() {
        let mut s = summary();
        let t = file_tokens(&NamingMetadata::default(), &s);
        assert_eq!(t.get("MediaInfo AudioLanguages"), Some(""), "English-only is omitted");
        assert_eq!(t.get("MediaInfo AudioLanguagesAll"), Some("[EN]"));
        s.audio_languages = vec!["te".into(), "ta".into(), "hi".into()];
        let t = file_tokens(&NamingMetadata::default(), &s);
        assert_eq!(t.get("MediaInfo AudioLanguages"), Some("[TE+TA+HI]"));
        assert_eq!(t.get("MediaInfo Full"), Some("h265 TrueHD Atmos [TE+TA+HI]"));
    }

    #[test]
    fn falls_back_to_original_name_when_everything_is_empty() {
        let naming = NamingConfig::default();
        let meta = NamingMetadata { original_file_name: "home_video".into(), ..Default::default() };
        assert_eq!(render_file_name(&naming, &meta, &MediaSummary::default()).unwrap().text, "home_video.mkv");
    }
}
