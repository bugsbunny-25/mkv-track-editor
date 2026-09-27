//! Best-effort parser for scene/P2P-style release names
//! (`The.Matrix.1999.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.7.1.Atmos-FraMeSToR`).
//! Its output only *prefills* the editable naming form, so it favors
//! predictable behavior over cleverness.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    #[default]
    Movie,
    Episode,
}

/// Radarr/Sonarr quality sources.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualitySource {
    #[default]
    Unknown,
    Remux,
    Bluray,
    #[serde(rename = "WEBDL")]
    WebDl,
    #[serde(rename = "WEBRip")]
    WebRip,
    #[serde(rename = "HDTV")]
    Hdtv,
    #[serde(rename = "DVD")]
    Dvd,
    #[serde(rename = "SDTV")]
    Sdtv,
}

impl QualitySource {
    pub const ALL: [QualitySource; 8] = [
        QualitySource::Unknown,
        QualitySource::Remux,
        QualitySource::Bluray,
        QualitySource::WebDl,
        QualitySource::WebRip,
        QualitySource::Hdtv,
        QualitySource::Dvd,
        QualitySource::Sdtv,
    ];

    pub fn label(self) -> &'static str {
        match self {
            QualitySource::Unknown => "Unknown",
            QualitySource::Remux => "Remux",
            QualitySource::Bluray => "Bluray",
            QualitySource::WebDl => "WEBDL",
            QualitySource::WebRip => "WEBRip",
            QualitySource::Hdtv => "HDTV",
            QualitySource::Dvd => "DVD",
            QualitySource::Sdtv => "SDTV",
        }
    }

    /// Radarr's quality title, e.g. "Remux-2160p", "WEBDL-1080p", "DVD".
    pub fn quality_title(self, resolution: Option<&str>) -> String {
        match (self, resolution) {
            (QualitySource::Unknown, _) => String::new(),
            (QualitySource::Dvd | QualitySource::Sdtv, _) => self.label().to_string(),
            (source, Some(res)) if !res.is_empty() => format!("{}-{res}", source.label()),
            (source, _) => source.label().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Revision {
    #[default]
    None,
    Proper,
    Repack,
}

impl Revision {
    pub fn label(self) -> &'static str {
        match self {
            Revision::None => "",
            Revision::Proper => "Proper",
            Revision::Repack => "Repack",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub kind: MediaKind,
    pub title: String,
    pub year: Option<u16>,
    pub season: Option<u32>,
    pub episodes: Vec<u32>,
    /// Text between the episode marker and the quality tags, e.g. "The
    /// Convict" in `The.Office.US.S03E10.The.Convict.720p...`.
    pub episode_title: String,
    pub source: QualitySource,
    pub resolution: Option<String>,
    pub revision: Revision,
    pub editions: Vec<String>,
    pub release_group: Option<String>,
}

static EPISODE_SXXEYY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[ ._\-\[(])S(\d{1,3})[ ._-]?E(\d{1,4})((?:[ ._-]?-?[ ._-]?E\d{1,4})*)").expect("valid")
});
static EPISODE_NXNN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[ ._\-])(\d{1,2})x(\d{2,3})(?:[ ._\-]|$)").expect("valid"));
static EXTRA_EPISODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)E(\d{1,4})").expect("valid"));
static YEAR_DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:19|20)\d{2}").expect("valid"));

/// Byte ranges of plausible years delimited by separators/brackets. Done by
/// hand because `regex` has no lookaround, and a consuming delimiter would
/// hide the second year in "1917.2019".
fn years(text: &str) -> Vec<(usize, usize)> {
    let before_ok = |i: usize| i == 0 || text[..i].ends_with([' ', '.', '_', '(', '[']);
    let after_ok = |i: usize| i == text.len() || text[i..].starts_with([' ', '.', '_', ')', ']']);
    YEAR_DIGITS
        .find_iter(text)
        .filter(|m| before_ok(m.start()) && after_ok(m.end()))
        .map(|m| (m.start(), m.end()))
        .collect()
}
/// Strong markers of where the title ends. Ambiguous words that also
/// appear in titles ("web", "dv", "hdr", "uhd") are deliberately excluded.
static TITLE_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[ ._\-\[(])(?:2160p|1080[pi]|720p|576p|480p|4k|remux|blu-?ray|bdrip|brrip|bdremux|web-?dl|webrip|hdtv|dvdrip|x26[45]|h\.?26[45]|hevc|proper|repack)(?:[ ._\-\])]|$)",
    )
    .expect("valid")
});
static TRAILING_TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\s*\[[^\]]*\])+$").expect("valid"));
static GROUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-([A-Za-z0-9][A-Za-z0-9_]*)$").expect("valid"));
static LEADING_GROUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[([^\]]+)\]\s*").expect("valid"));
static SPACED_ACRONYM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:[A-Za-z] ){2,}[A-Za-z]\b").expect("valid"));

fn is(re: &str, text: &str) -> bool {
    Regex::new(re).map(|r| r.is_match(text)).unwrap_or(false)
}

fn strip_extension(file_name: &str) -> &str {
    match file_name.rsplit_once('.') {
        Some((stem, ext)) if (2..=4).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) => stem,
        _ => file_name,
    }
}

fn clean_title(raw: &str) -> String {
    let spaced = if raw.contains(' ') {
        raw.replace('_', " ")
    } else {
        raw.replace(['.', '_'], " ")
    };
    // "S W A T" → "S.W.A.T"
    let joined = SPACED_ACRONYM.replace_all(&spaced, |c: &regex::Captures| c[0].replace(' ', "."));
    joined
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '(' | '[' | '.'))
        .to_string()
}

pub fn parse_release_name(file_name: &str) -> ReleaseInfo {
    let stem = strip_extension(file_name.trim());
    let mut info = ReleaseInfo::default();

    // Anime-style "[Group] Title - 01" puts the group first.
    let mut body = stem.to_string();
    if let Some(c) = LEADING_GROUP.captures(stem) {
        info.release_group = Some(c[1].trim().to_string());
        body = stem[c.get(0).map_or(0, |m| m.end())..].to_string();
    }
    let body = TRAILING_TAGS.replace(&body, "").into_owned();

    if info.release_group.is_none() {
        if let Some(c) = GROUP.captures(&body) {
            let group = c[1].to_string();
            let not_a_group = is(r"(?i)^(dl|hd|rip|x26[45]|h26[45]|hevc|avc|\d+p?|web)$", &group);
            if !not_a_group {
                info.release_group = Some(group);
            }
        }
    }

    let title_end = TITLE_END.find(&body).map(|m| m.start()).unwrap_or(body.len());
    // Where the title (and year / episode marker) stops; editions are only
    // searched after it so a movie *called* "Final Cut" isn't an edition.
    let after_title;

    if let Some(c) = EPISODE_SXXEYY.captures(&body) {
        let whole = c.get(0).expect("group 0 always matches");
        info.kind = MediaKind::Episode;
        info.season = c[1].parse().ok();
        info.episodes = std::iter::once(&c[2])
            .filter_map(|e| e.parse().ok())
            .chain(EXTRA_EPISODE.captures_iter(&c[3]).filter_map(|e| e[1].parse().ok()))
            .collect();
        set_series_title(&mut info, &body[..whole.start()]);
        after_title = whole.end();
    } else if let Some(c) = EPISODE_NXNN.captures(&body) {
        let whole = c.get(0).expect("group 0 always matches");
        info.kind = MediaKind::Episode;
        info.season = c[1].parse().ok();
        info.episodes = c[2].parse().ok().into_iter().collect();
        set_series_title(&mut info, &body[..whole.start()]);
        after_title = whole.end();
    } else {
        // Movie: the last year before the first quality marker (so
        // "Blade.Runner.2049.2017" → "Blade Runner 2049" (2017), and
        // "1917.2019" → "1917" (2019)). A year at position 0 is the title.
        let year = years(&body[..title_end]).into_iter().rfind(|(start, _)| *start > 0);
        match year {
            Some((start, end)) => {
                info.year = body[start..end].parse().ok();
                info.title = clean_title(&body[..start]);
                after_title = end;
            }
            None => {
                info.title = clean_title(&body[..title_end]);
                after_title = title_end;
            }
        }
    }

    if info.kind == MediaKind::Episode && after_title < title_end {
        info.episode_title = clean_title(&body[after_title..title_end]);
    }

    let rest = &body[after_title.min(body.len())..];
    let whole = body.as_str();

    info.resolution = if is(r"(?i)\b(2160p|4k)\b", whole) {
        Some("2160p".into())
    } else if is(r"(?i)\b1080[pi]\b", whole) {
        Some("1080p".into())
    } else if is(r"(?i)\b720p\b", whole) {
        Some("720p".into())
    } else if is(r"(?i)\b576p\b", whole) {
        Some("576p".into())
    } else if is(r"(?i)\b480p\b", whole) {
        Some("480p".into())
    } else if is(r"(?i)\buhd\b", whole) {
        Some("2160p".into())
    } else {
        None
    };

    info.source = if is(r"(?i)\b(bd)?remux\b", whole) {
        QualitySource::Remux
    } else if is(r"(?i)\bweb[ ._-]?rip\b", whole) {
        QualitySource::WebRip
    } else if is(r"(?i)\bweb[ ._-]?dl\b|\bweb\b|\bamzn\b|\bnf\b|\bdsnp\b|\batvp\b|\bhmax\b", whole) {
        QualitySource::WebDl
    } else if is(r"(?i)\bblu[ ._-]?ray\b|\bbd(rip|mv)?\b|\bbrrip\b", whole) {
        QualitySource::Bluray
    } else if is(r"(?i)\b(hdtv|pdtv|dsr)\b", whole) {
        QualitySource::Hdtv
    } else if is(r"(?i)\bdvd(rip|9|5|r)?\b", whole) {
        QualitySource::Dvd
    } else if is(r"(?i)\bsdtv\b", whole) {
        QualitySource::Sdtv
    } else {
        QualitySource::Unknown
    };

    info.revision = if is(r"(?i)\b(repack|rerip)\d?\b", whole) {
        Revision::Repack
    } else if is(r"(?i)\bproper\b|\bv2\b", whole) {
        Revision::Proper
    } else {
        Revision::None
    };

    info.editions = parse_editions(rest);
    info
}

fn set_series_title(info: &mut ReleaseInfo, raw_title: &str) {
    let title = clean_title(raw_title);
    // "Doctor Who 2005" / "Doctor Who (2005)" → title + series year.
    static TRAILING_YEAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(.+?)[ (]+((?:19|20)\d{2})\)?$").expect("valid"));
    match TRAILING_YEAR.captures(&title) {
        Some(c) => {
            info.title = c[1].trim().to_string();
            info.year = c[2].parse().ok();
        }
        None => info.title = title,
    }
}

fn parse_editions(text: &str) -> Vec<String> {
    const SEP: &str = r"[ ._-]?";
    let editions: &[(&str, &str)] = &[
        ("4K Remaster", &format!(r"(?i)\b4k{SEP}remaster(ed)?\b")),
        ("Director's Cut", &format!(r"(?i)\bdirector'?s{SEP}cut\b")),
        ("Extended", &format!(r"(?i)\bextended({SEP}(cut|edition))?\b")),
        ("Ultimate Edition", &format!(r"(?i)\bultimate({SEP}(cut|edition))\b")),
        ("Theatrical", &format!(r"(?i)\btheatrical({SEP}cut)?\b")),
        ("Unrated", r"(?i)\bunrated\b"),
        ("Uncut", r"(?i)\buncut\b"),
        ("Special Edition", &format!(r"(?i)\bspecial{SEP}edition\b")),
        ("Collector's Edition", &format!(r"(?i)\bcollector'?s{SEP}edition\b")),
        ("Anniversary Edition", &format!(r"(?i)\b(\d+(th)?{SEP})?anniversary{SEP}edition\b")),
        ("Criterion", &format!(r"(?i)\bcriterion({SEP}collection)?\b")),
        ("Final Cut", &format!(r"(?i)\bfinal{SEP}cut\b")),
        ("Open Matte", &format!(r"(?i)\bopen{SEP}matte\b")),
        ("Redux", r"(?i)\bredux\b"),
    ];
    let mut found: Vec<String> = editions
        .iter()
        .filter(|(_, re)| is(re, text))
        .map(|(name, _)| name.to_string())
        .collect();

    if found.iter().any(|e| e == "4K Remaster") {
        found.retain(|e| e != "Remastered");
    } else if is(r"(?i)\bremaster(ed)?\b", text) {
        found.push("Remastered".into());
    }
    // `regex` has no lookahead: "IMAX" is an edition unless it's part of
    // "IMAX Enhanced" (which is a custom format, not an edition).
    let imax = Regex::new(&format!(r"(?i)\bimax({SEP}enhanced)?\b")).expect("valid");
    if imax.captures_iter(text).any(|c| c.get(1).is_none()) {
        found.push("IMAX".into());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> ReleaseInfo {
        parse_release_name(name)
    }

    #[test]
    fn movies() {
        let r = p("The.Matrix.1999.2160p.UHD.BluRay.REMUX.DV.HDR.HEVC.TrueHD.7.1.Atmos-FraMeSToR.mkv");
        assert_eq!(r.kind, MediaKind::Movie);
        assert_eq!(r.title, "The Matrix");
        assert_eq!(r.year, Some(1999));
        assert_eq!(r.resolution.as_deref(), Some("2160p"));
        assert_eq!(r.source, QualitySource::Remux);
        assert_eq!(r.release_group.as_deref(), Some("FraMeSToR"));

        let r = p("Blade.Runner.2049.2017.1080p.BluRay.x264-SPARKS.mkv");
        assert_eq!((r.title.as_str(), r.year), ("Blade Runner 2049", Some(2017)));
        assert_eq!(r.source, QualitySource::Bluray);

        let r = p("1917.2019.2160p.WEB-DL.DDP5.1.HDR.HEVC-EVO.mkv");
        assert_eq!((r.title.as_str(), r.year), ("1917", Some(2019)));
        assert_eq!(r.source, QualitySource::WebDl);
        assert_eq!(r.release_group.as_deref(), Some("EVO"));

        let r = p("Charlotte's.Web.2006.720p.WEBRip.x264-GalaxyRG.mkv");
        assert_eq!((r.title.as_str(), r.year), ("Charlotte's Web", Some(2006)));
        assert_eq!(r.source, QualitySource::WebRip);

        let r = p("Aliens (1986) Special Edition 1080p BluRay REPACK x265 10bit-Tigole.mkv");
        assert_eq!((r.title.as_str(), r.year), ("Aliens", Some(1986)));
        assert_eq!(r.editions, ["Special Edition"]);
        assert_eq!(r.revision, Revision::Repack);
        assert_eq!(r.release_group.as_deref(), Some("Tigole"));

        let r = p("Apocalypse.Now.1979.Final.Cut.IMAX.Enhanced.2160p.UHD.BluRay.REMUX-GRP.mkv");
        assert_eq!(r.editions, ["Final Cut"], "IMAX Enhanced is a custom format, not an edition");

        let r = p("Avatar.2009.Extended.Collectors.Edition.IMAX.1080p.BluRay.x264.PROPER-GRP.mkv");
        assert!(r.editions.contains(&"Extended".to_string()));
        assert!(r.editions.contains(&"IMAX".to_string()));
        assert_eq!(r.revision, Revision::Proper);

        let r = p("Baahubali.2.The.Conclusion.2017.Telugu.2160p.WEB-DL.AMZN.DDP5.1.HEVC.mkv");
        assert_eq!((r.title.as_str(), r.year), ("Baahubali 2 The Conclusion", Some(2017)));
        assert_eq!(r.release_group, None, "no -GROUP suffix");

        let r = p("S.W.A.T.2003.1080p.BluRay.x264-HANDJOB.mkv");
        assert_eq!(r.title, "S.W.A.T");

        let r = p("Some Home Movie.mkv");
        assert_eq!((r.title.as_str(), r.year, r.source), ("Some Home Movie", None, QualitySource::Unknown));

        let r = p("The.Movie.2020.1080p.WEB-DL.mkv");
        assert_eq!(r.release_group, None, "`-DL` is not a group");
    }

    #[test]
    fn episodes() {
        let r = p("Doctor.Who.2005.S01E02.The.End.of.the.World.1080p.BluRay.x264-SHORTBREHD.mkv");
        assert_eq!(r.kind, MediaKind::Episode);
        assert_eq!((r.title.as_str(), r.year), ("Doctor Who", Some(2005)));
        assert_eq!((r.season, r.episodes.as_slice()), (Some(1), &[2][..]));
        assert_eq!(r.episode_title, "The End of the World");

        let r = p("The.Office.US.S03E10E11.720p.WEB-DL.mkv");
        assert_eq!(r.title, "The Office US");
        assert_eq!(r.episodes, [10, 11]);
        assert_eq!(r.episode_title, "", "no title between marker and tags");

        let r = p("Mirzapur S02E01-E02 1080p AMZN WEB-DL.mkv");
        assert_eq!(r.title, "Mirzapur");
        assert_eq!(r.episodes, [1, 2]);
        assert_eq!(r.source, QualitySource::WebDl);

        let r = p("Friends.10x05.HDTV.XviD-LOL.avi");
        assert_eq!((r.title.as_str(), r.season, r.episodes.as_slice()), ("Friends", Some(10), &[5][..]));
        assert_eq!(r.source, QualitySource::Hdtv);

        let r = p("[SubsPlease] Frieren S01E05 (1080p) [ABCD1234].mkv");
        assert_eq!(r.release_group.as_deref(), Some("SubsPlease"));
        assert_eq!(r.title, "Frieren");
    }

    #[test]
    fn quality_titles() {
        assert_eq!(QualitySource::Remux.quality_title(Some("2160p")), "Remux-2160p");
        assert_eq!(QualitySource::WebDl.quality_title(Some("1080p")), "WEBDL-1080p");
        assert_eq!(QualitySource::Dvd.quality_title(Some("480p")), "DVD");
        assert_eq!(QualitySource::Unknown.quality_title(Some("1080p")), "");
    }
}
