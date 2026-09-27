//! Turns raw MediaInfo fields into two vocabularies:
//! - the user's track-name labels ("DV8 HDR10", "DD+", "7.1"), and
//! - Radarr/Sonarr's `{MediaInfo ...}` token values ("DV HDR10", "EAC3 Atmos",
//!   "x265"), kept exactly as Radarr formats them so generated file names stay
//!   parseable by Radarr/Sonarr.
//!
//! mkvmerge's codec string is used as a fallback when MediaInfo has no entry
//! for a track.

use mkv_types::MediaTrack;

fn lower(s: &Option<String>) -> String {
    s.as_deref().unwrap_or("").to_ascii_lowercase()
}

/// The non-Dolby-Vision part of a dynamic range: either the whole thing
/// (plain HDR10, SDR, ...) or a Dolby Vision stream's fallback layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrBase {
    Sdr,
    Hdr10,
    Hdr10Plus,
    Hlg,
    /// PQ transfer without HDR10 static metadata.
    Pq,
}

impl HdrBase {
    pub fn label(self) -> &'static str {
        match self {
            HdrBase::Sdr => "SDR",
            HdrBase::Hdr10 => "HDR10",
            HdrBase::Hdr10Plus => "HDR10+",
            HdrBase::Hlg => "HLG",
            HdrBase::Pq => "PQ",
        }
    }

    fn radarr(self) -> &'static str {
        match self {
            HdrBase::Sdr => "",
            HdrBase::Hdr10 => "HDR10",
            HdrBase::Hdr10Plus => "HDR10Plus",
            HdrBase::Hlg => "HLG",
            HdrBase::Pq => "PQ",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicRange {
    /// Dolby Vision profile number, if the stream carries DV.
    pub dolby_vision: Option<u8>,
    /// Fallback / base layer. `None` only for DV without a compatible
    /// fallback (profile 5, IPTPQc2).
    pub base: Option<HdrBase>,
}

impl DynamicRange {
    pub const SDR: Self = Self { dolby_vision: None, base: Some(HdrBase::Sdr) };

    /// User style: "SDR", "HDR10+", "DV5", "DV8 HDR10", "DV8 HLG".
    pub fn label(&self) -> String {
        match (self.dolby_vision, self.base) {
            (Some(p), Some(base)) => format!("DV{p} {}", base.label()),
            (Some(p), None) => format!("DV{p}"),
            (None, Some(base)) => base.label().to_string(),
            (None, None) => HdrBase::Sdr.label().to_string(),
        }
    }

    /// Radarr's `{MediaInfo VideoDynamicRangeType}`: "DV HDR10", "HDR10Plus", "".
    pub fn radarr_type(&self) -> String {
        match (self.dolby_vision, self.base) {
            (Some(_), None) => "DV".into(),
            // Plain SDR is "" in Radarr, but DV with an SDR fallback is "DV SDR".
            (Some(_), Some(HdrBase::Sdr)) => "DV SDR".into(),
            (Some(_), Some(base)) => format!("DV {}", base.radarr()),
            (None, base) => base.map(HdrBase::radarr).unwrap_or("").into(),
        }
    }

    /// Radarr's `{MediaInfo VideoDynamicRange}`: "HDR" or "".
    pub fn radarr_range(&self) -> &'static str {
        if self.dolby_vision.is_none() && matches!(self.base, Some(HdrBase::Sdr) | None) {
            ""
        } else {
            "HDR"
        }
    }
}

/// Parses the DV profile out of MediaInfo's `HDR_Format_Profile`, e.g.
/// "dvhe.08.06", "dvhe.07.06 / ", "dav1.10.06", "dvh1.08".
fn dv_profile(profile: &str) -> Option<u8> {
    profile
        .split(|c: char| c == '/' || c.is_whitespace())
        .find(|tok| tok.starts_with("dv") || tok.starts_with("da"))
        .and_then(|tok| tok.split('.').nth(1))
        .and_then(|p| p.parse().ok())
}

pub fn detect_dynamic_range(t: &MediaTrack) -> DynamicRange {
    let hdr = lower(&t.hdr_format);
    let compat = lower(&t.hdr_format_compatibility);
    let transfer = lower(&t.transfer_characteristics);

    let hdr10plus = hdr.contains("2094 app 4") || compat.contains("hdr10+");
    let hlg = transfer.contains("hlg") || transfer.contains("arib std-b67") || compat.contains("hlg");
    let pq = transfer.contains("pq") || transfer.contains("2084");
    let static_hdr10 = hdr.contains("2086") || t.has_mastering_display || compat.contains("hdr10");

    if hdr.contains("dolby vision") {
        let profile = dv_profile(&lower(&t.hdr_format_profile));
        let base = if profile == Some(5) {
            // Profile 5 is IPTPQc2 with no backwards-compatible layer.
            None
        } else if hdr10plus {
            Some(HdrBase::Hdr10Plus)
        } else if hlg {
            Some(HdrBase::Hlg)
        } else if compat.contains("sdr") {
            Some(HdrBase::Sdr)
        } else if static_hdr10 || pq || compat.contains("blu-ray") || profile == Some(7) {
            // Profile 7's base layer is always HDR10 (UHD Blu-ray).
            Some(HdrBase::Hdr10)
        } else {
            None
        };
        return DynamicRange {
            dolby_vision: Some(profile.unwrap_or(0)),
            base,
        };
    }

    let base = if hdr10plus {
        HdrBase::Hdr10Plus
    } else if hlg {
        HdrBase::Hlg
    } else if pq && static_hdr10 {
        HdrBase::Hdr10
    } else if pq {
        HdrBase::Pq
    } else if static_hdr10 {
        // Mastering metadata without an explicit transfer characteristic.
        HdrBase::Hdr10
    } else {
        HdrBase::Sdr
    };
    DynamicRange { dolby_vision: None, base: Some(base) }
}

/// Radarr-style resolution bucket. Width is checked first so scope releases
/// (1920x800) still count as 1080p.
pub fn resolution(width: Option<u32>, height: Option<u32>) -> Option<String> {
    let (w, h) = (width.unwrap_or(0), height.unwrap_or(0));
    let label = if w >= 7000 || h >= 4000 {
        "4320p"
    } else if w >= 3200 || h >= 2100 {
        "2160p"
    } else if w >= 1800 || h >= 1000 {
        "1080p"
    } else if w >= 1200 || h >= 700 {
        "720p"
    } else if h >= 560 {
        "576p"
    } else if h >= 400 {
        "480p"
    } else if h > 0 {
        return Some(format!("{h}p"));
    } else {
        return None;
    };
    Some(label.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoAttrs {
    pub resolution: Option<String>,
    pub dynamic_range: DynamicRange,
    /// User style: "AVC", "HEVC", "AV1", "VP9", "MPEG-2", "VC-1".
    pub codec: String,
    /// Radarr's `{MediaInfo VideoCodec}`: "x264", "h265", "AV1", ...
    pub radarr_codec: String,
    pub bit_depth: Option<u32>,
    pub is_3d: bool,
}

pub fn video_attrs(media: Option<&MediaTrack>, mkv_codec: &str) -> VideoAttrs {
    let format = media.map(|m| lower(&m.format)).unwrap_or_default();
    let fallback = mkv_codec.to_ascii_lowercase();
    let lib = media.map(|m| lower(&m.encoded_library_name)).unwrap_or_default();
    let is = |needle: &str| format.contains(needle) || fallback.contains(needle);

    let (codec, radarr_codec) = if is("hevc") || is("h.265") {
        ("HEVC", if lib.contains("x265") { "x265" } else { "h265" })
    } else if is("avc") || is("h.264") {
        ("AVC", if lib.contains("x264") { "x264" } else { "h264" })
    } else if is("av1") {
        ("AV1", "AV1")
    } else if is("vp9") {
        ("VP9", "VP9")
    } else if is("vp8") {
        ("VP8", "VP8")
    } else if is("vc-1") || is("vc1") {
        ("VC-1", "VC1")
    } else if is("mpeg-4 visual") || is("mpeg-4p2") || is("xvid") || is("divx") {
        if lib.contains("xvid") {
            ("MPEG-4", "XviD")
        } else {
            ("MPEG-4", "DivX")
        }
    } else if is("mpeg video") || is("mpeg-2") || is("mpeg-1") {
        ("MPEG-2", "MPEG2")
    } else {
        let raw = media
            .and_then(|m| m.format.clone())
            .unwrap_or_else(|| mkv_codec.to_string());
        return VideoAttrs {
            resolution: media.and_then(|m| resolution(m.width, m.height)),
            dynamic_range: media.map(detect_dynamic_range).unwrap_or(DynamicRange::SDR),
            radarr_codec: raw.clone(),
            codec: raw,
            bit_depth: media.and_then(|m| m.bit_depth),
            is_3d: media.and_then(|m| m.multiview_count).unwrap_or(0) > 1,
        };
    };

    VideoAttrs {
        resolution: media.and_then(|m| resolution(m.width, m.height)),
        dynamic_range: media.map(detect_dynamic_range).unwrap_or(DynamicRange::SDR),
        codec: codec.into(),
        radarr_codec: radarr_codec.into(),
        bit_depth: media.and_then(|m| m.bit_depth),
        is_3d: media.and_then(|m| m.multiview_count).unwrap_or(0) > 1,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioAttrs {
    /// User style: "Dolby TrueHD", "DD+", "DD", "DTS-HD MA", "DTS:X", "AAC", ...
    pub format: String,
    /// Radarr's `{MediaInfo AudioCodec}`: "TrueHD Atmos", "EAC3", "DTS-X", ...
    pub radarr_codec: String,
    pub atmos: bool,
    /// "7.1", "5.1", "2.0", ...
    pub channels: Option<String>,
}

pub fn audio_attrs(media: Option<&MediaTrack>, mkv_codec: &str) -> AudioAttrs {
    let format = media.map(|m| lower(&m.format)).unwrap_or_default();
    let commercial = media.map(|m| lower(&m.format_commercial)).unwrap_or_default();
    let features = media.map(|m| lower(&m.format_additional_features)).unwrap_or_default();
    let profile = media.map(|m| lower(&m.format_profile)).unwrap_or_default();
    let codec_id = media.map(|m| lower(&m.codec_id)).unwrap_or_default();
    let fallback = mkv_codec.to_ascii_lowercase();
    let feature_tokens: Vec<&str> = features.split([' ', '/']).filter(|t| !t.is_empty()).collect();
    let has_feature = |f: &str| feature_tokens.contains(&f);

    let atmos_signalled = commercial.contains("atmos") || fallback.contains("atmos");

    let (format_label, radarr, atmos) = if format.contains("mlp fba")
        || format.contains("truehd")
        || codec_id.contains("truehd")
        || fallback.contains("truehd")
    {
        let atmos = atmos_signalled || has_feature("16-ch");
        ("Dolby TrueHD", if atmos { "TrueHD Atmos" } else { "TrueHD" }, atmos)
    } else if format.contains("e-ac-3") || codec_id.contains("eac3") || fallback.contains("e-ac-3") {
        let atmos = atmos_signalled || has_feature("joc");
        ("DD+", if atmos { "EAC3 Atmos" } else { "EAC3" }, atmos)
    } else if format.contains("ac-4") || fallback.contains("ac-4") {
        ("AC-4", "AC4", atmos_signalled)
    } else if format.contains("ac-3") || codec_id.contains("ac3") || fallback == "ac-3" {
        ("DD", "AC3", false)
    } else if format.contains("dts") || fallback.contains("dts") {
        let dts_x = commercial.contains("dts:x") || has_feature("x") || fallback.contains("dts:x");
        let ma = commercial.contains("master audio")
            || has_feature("xll")
            || profile.split([' ', '/']).any(|p| p == "ma")
            || fallback.contains("master audio");
        if dts_x {
            ("DTS:X", "DTS-X", false)
        } else if ma {
            ("DTS-HD MA", "DTS-HD MA", false)
        } else if commercial.contains("high resolution") || has_feature("xbr") || profile.contains("hra") {
            ("DTS-HD HRA", "DTS-HD HRA", false)
        } else if commercial.contains("express") || has_feature("lbr") {
            ("DTS Express", "DTS", false)
        } else if commercial.contains("dts-es") || has_feature("es") || profile.contains("es") {
            ("DTS-ES", "DTS-ES", false)
        } else {
            ("DTS", "DTS", false)
        }
    } else if format.contains("aac") || fallback.contains("aac") {
        ("AAC", "AAC", false)
    } else if format.contains("flac") || fallback.contains("flac") {
        ("FLAC", "FLAC", false)
    } else if format.contains("pcm") || fallback.contains("pcm") {
        ("PCM", "PCM", false)
    } else if format.contains("opus") || fallback.contains("opus") {
        ("Opus", "Opus", false)
    } else if format.contains("vorbis") || fallback.contains("vorbis") {
        ("Vorbis", "Vorbis", false)
    } else if format.contains("alac") || fallback.contains("alac") {
        ("ALAC", "ALAC", false)
    } else if format.contains("mpeg audio") || fallback.contains("mp3") || fallback.contains("mp2") {
        if profile.contains("layer 2") || fallback.contains("mp2") {
            ("MP2", "MP2", false)
        } else {
            ("MP3", "MP3", false)
        }
    } else {
        let raw = media
            .and_then(|m| m.format.clone())
            .unwrap_or_else(|| mkv_codec.to_string());
        return AudioAttrs {
            radarr_codec: raw.clone(),
            format: raw,
            atmos: false,
            channels: media.and_then(channels_label),
        };
    };

    AudioAttrs {
        format: format_label.into(),
        radarr_codec: radarr.into(),
        atmos,
        channels: media.and_then(channels_label),
    }
}

/// 6 channels with an LFE in the layout → "5.1"; 2 without → "2.0".
fn channels_label(m: &MediaTrack) -> Option<String> {
    let count = m.channels?;
    let has_lfe = m
        .channel_layout
        .as_deref()
        .map(|l| l.split_whitespace().any(|c| c.eq_ignore_ascii_case("lfe") || c.eq_ignore_ascii_case("lfe2")))
        // Without a layout, assume the usual surround conventions.
        .unwrap_or(matches!(count, 6 | 8));
    Some(if has_lfe && count > 1 {
        format!("{}.1", count - 1)
    } else {
        format!("{count}.0")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(hdr: &str, profile: &str, compat: &str, transfer: &str, mastering: bool) -> MediaTrack {
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        MediaTrack {
            format: Some("HEVC".into()),
            width: Some(3840),
            height: Some(2160),
            hdr_format: opt(hdr),
            hdr_format_profile: opt(profile),
            hdr_format_compatibility: opt(compat),
            transfer_characteristics: opt(transfer),
            has_mastering_display: mastering,
            ..Default::default()
        }
    }

    fn check(t: MediaTrack, label: &str, radarr: &str) {
        let dr = detect_dynamic_range(&t);
        assert_eq!(dr.label(), label, "label for {t:?}");
        assert_eq!(dr.radarr_type(), radarr, "radarr type for {t:?}");
    }

    #[test]
    fn dynamic_range_matrix() {
        check(video("", "", "", "BT.709", false), "SDR", "");
        check(video("SMPTE ST 2086", "", "HDR10", "PQ", true), "HDR10", "HDR10");
        check(
            video("SMPTE ST 2094 App 4 / SMPTE ST 2086", "", "HDR10+ Profile A / HDR10", "PQ", true),
            "HDR10+",
            "HDR10Plus",
        );
        check(video("", "", "", "HLG", false), "HLG", "HLG");
        check(video("", "", "", "PQ", false), "PQ", "PQ");
        check(video("Dolby Vision", "dvhe.05.06", "", "", false), "DV5", "DV");
        check(
            video("Dolby Vision / SMPTE ST 2086", "dvhe.07.06 / ", "Blu-ray / HDR10", "PQ", true),
            "DV7 HDR10",
            "DV HDR10",
        );
        check(
            video("Dolby Vision / SMPTE ST 2094 App 4 / SMPTE ST 2086", "dvhe.07.06", "Blu-ray / HDR10+ Profile B / HDR10", "PQ", true),
            "DV7 HDR10+",
            "DV HDR10Plus",
        );
        check(
            video("Dolby Vision / SMPTE ST 2086", "dvhe.08.06 / ", "HDR10 / HDR10", "PQ", true),
            "DV8 HDR10",
            "DV HDR10",
        );
        check(
            video("Dolby Vision / SMPTE ST 2094 App 4 / SMPTE ST 2086", "dvhe.08.06", "HDR10 / HDR10+ Profile A / HDR10", "PQ", true),
            "DV8 HDR10+",
            "DV HDR10Plus",
        );
        check(video("Dolby Vision", "dvhe.08.04", "HLG", "HLG", false), "DV8 HLG", "DV HLG");
        check(video("Dolby Vision", "dvhe.08.02", "SDR", "BT.709", false), "DV8 SDR", "DV SDR");
        check(video("Dolby Vision / SMPTE ST 2086", "dav1.10.06", "HDR10", "PQ", true), "DV10 HDR10", "DV HDR10");
    }

    #[test]
    fn dynamic_range_hdr_flag() {
        assert_eq!(DynamicRange::SDR.radarr_range(), "");
        assert_eq!(detect_dynamic_range(&video("Dolby Vision", "dvhe.05", "", "", false)).radarr_range(), "HDR");
        assert_eq!(detect_dynamic_range(&video("", "", "", "HLG", false)).radarr_range(), "HDR");
    }

    #[test]
    fn resolution_buckets() {
        assert_eq!(resolution(Some(3840), Some(2160)).as_deref(), Some("2160p"));
        assert_eq!(resolution(Some(3840), Some(1600)).as_deref(), Some("2160p"));
        assert_eq!(resolution(Some(1920), Some(800)).as_deref(), Some("1080p"));
        assert_eq!(resolution(Some(1280), Some(534)).as_deref(), Some("720p"));
        assert_eq!(resolution(Some(720), Some(576)).as_deref(), Some("576p"));
        assert_eq!(resolution(Some(720), Some(480)).as_deref(), Some("480p"));
        assert_eq!(resolution(None, None), None);
    }

    #[test]
    fn video_codec_labels() {
        let mut t = MediaTrack { format: Some("HEVC".into()), ..Default::default() };
        assert_eq!(video_attrs(Some(&t), "").codec, "HEVC");
        assert_eq!(video_attrs(Some(&t), "").radarr_codec, "h265");
        t.encoded_library_name = Some("x265".into());
        assert_eq!(video_attrs(Some(&t), "").radarr_codec, "x265");

        let avc = MediaTrack { format: Some("AVC".into()), ..Default::default() };
        assert_eq!(video_attrs(Some(&avc), "").codec, "AVC");
        assert_eq!(video_attrs(Some(&avc), "").radarr_codec, "h264");
        assert_eq!(video_attrs(None, "AVC/H.264/MPEG-4p10").codec, "AVC");
        assert_eq!(video_attrs(None, "AV1").codec, "AV1");
        let mpeg2 = MediaTrack { format: Some("MPEG Video".into()), ..Default::default() };
        assert_eq!(video_attrs(Some(&mpeg2), "").radarr_codec, "MPEG2");
        let vc1 = MediaTrack { format: Some("VC-1".into()), ..Default::default() };
        assert_eq!(video_attrs(Some(&vc1), "").codec, "VC-1");

        let three_d = MediaTrack { format: Some("AVC".into()), multiview_count: Some(2), ..Default::default() };
        assert!(video_attrs(Some(&three_d), "").is_3d);
    }

    fn audio(format: &str, commercial: &str, features: &str, channels: u32, layout: &str) -> MediaTrack {
        let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());
        MediaTrack {
            format: opt(format),
            format_commercial: opt(commercial),
            format_additional_features: opt(features),
            channels: Some(channels),
            channel_layout: opt(layout),
            ..Default::default()
        }
    }

    fn check_audio(t: MediaTrack, format: &str, radarr: &str, atmos: bool, channels: &str) {
        let a = audio_attrs(Some(&t), "");
        assert_eq!(
            (a.format.as_str(), a.radarr_codec.as_str(), a.atmos, a.channels.as_deref()),
            (format, radarr, atmos, Some(channels)),
            "{t:?}"
        );
    }

    #[test]
    fn audio_format_matrix() {
        const L71: &str = "L R C LFE Ls Rs Lb Rb";
        const L51: &str = "L R C LFE Ls Rs";
        check_audio(audio("MLP FBA", "Dolby TrueHD with Dolby Atmos", "16-ch", 8, L71), "Dolby TrueHD", "TrueHD Atmos", true, "7.1");
        check_audio(audio("MLP FBA", "Dolby TrueHD", "", 6, L51), "Dolby TrueHD", "TrueHD", false, "5.1");
        check_audio(audio("E-AC-3", "Dolby Digital Plus with Dolby Atmos", "JOC", 6, L51), "DD+", "EAC3 Atmos", true, "5.1");
        check_audio(audio("E-AC-3", "Dolby Digital Plus", "", 6, L51), "DD+", "EAC3", false, "5.1");
        check_audio(audio("AC-3", "Dolby Digital", "", 2, "L R"), "DD", "AC3", false, "2.0");
        check_audio(audio("DTS", "DTS-HD Master Audio with DTS:X", "XLL X", 8, L71), "DTS:X", "DTS-X", false, "7.1");
        check_audio(audio("DTS", "DTS-HD Master Audio", "XLL", 6, L51), "DTS-HD MA", "DTS-HD MA", false, "5.1");
        check_audio(audio("DTS", "DTS-HD High Resolution Audio", "XBR", 6, L51), "DTS-HD HRA", "DTS-HD HRA", false, "5.1");
        check_audio(audio("DTS", "DTS-ES", "ES", 7, "L R C LFE Ls Rs Cb"), "DTS-ES", "DTS-ES", false, "6.1");
        check_audio(audio("DTS", "", "", 6, L51), "DTS", "DTS", false, "5.1");
        check_audio(audio("AAC", "", "LC", 2, "L R"), "AAC", "AAC", false, "2.0");
        check_audio(audio("FLAC", "", "", 1, "M"), "FLAC", "FLAC", false, "1.0");
        check_audio(audio("PCM", "", "", 6, L51), "PCM", "PCM", false, "5.1");
        check_audio(audio("Opus", "", "", 2, "L R"), "Opus", "Opus", false, "2.0");
        check_audio(audio("AC-3", "", "", 3, "L R LFE"), "DD", "AC3", false, "2.1");
    }

    #[test]
    fn audio_falls_back_to_mkvmerge_codec() {
        let a = audio_attrs(None, "TrueHD Atmos");
        assert_eq!((a.format.as_str(), a.radarr_codec.as_str(), a.atmos), ("Dolby TrueHD", "TrueHD Atmos", true));
        assert_eq!(audio_attrs(None, "E-AC-3").format, "DD+");
        assert_eq!(audio_attrs(None, "AC-3").format, "DD");
        assert_eq!(audio_attrs(None, "DTS-HD Master Audio").format, "DTS-HD MA");
        assert_eq!(audio_attrs(None, "Weird").format, "Weird");
    }
}
