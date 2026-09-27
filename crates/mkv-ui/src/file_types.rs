//! Which files the app can open. Everything is edited with MKVToolNix
//! (`mkvpropedit` only understands Matroska), so only Matroska video is
//! accepted; other video containers are recognised just to explain why
//! they're turned away.

/// Extensions of files the app opens (lowercase, no dot). Shared by the
/// desktop Open dialog filter and drag-and-drop.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["mkv"];

/// Video containers we recognise but can't edit.
const OTHER_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "avi", "wmv", "webm", "flv", "ts", "m2ts", "mts", "mpg", "mpeg", "vob", "ogv", "3gp", "mk3d",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// Matroska video — can be opened.
    Supported,
    /// A video, but not in a container we can edit.
    OtherVideo,
    NotVideo,
}

impl FileKind {
    pub fn is_supported(self) -> bool {
        self == FileKind::Supported
    }
}

/// Classifies a path by its extension (case-insensitive).
pub fn classify(path: &str) -> FileKind {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return FileKind::NotVideo;
    };
    if stem.is_empty() {
        // Dotfiles like ".mkv" have no extension.
        return FileKind::NotVideo;
    }
    let ext = ext.to_ascii_lowercase();
    if SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
        FileKind::Supported
    } else if OTHER_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        FileKind::OtherVideo
    } else {
        FileKind::NotVideo
    }
}

/// One message for a batch of files that were turned away, naming at most a
/// few of them.
pub fn rejection_message(rejected: &[(String, FileKind)]) -> Option<String> {
    const MAX_NAMED: usize = 3;
    match rejected {
        [] => None,
        [(name, FileKind::OtherVideo)] => {
            Some(format!("“{name}” isn't an MKV file. Only Matroska (.mkv) video can be edited."))
        }
        [(name, _)] => Some(format!("“{name}” isn't a video file. Drop MKV files to open them.")),
        many => {
            let names: Vec<&str> = many.iter().take(MAX_NAMED).map(|(n, _)| n.as_str()).collect();
            let more = match many.len().saturating_sub(MAX_NAMED) {
                0 => String::new(),
                n => format!(" and {n} more"),
            };
            Some(format!(
                "Skipped {} files that aren't MKV video: {}{more}.",
                many.len(),
                names.join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_matroska_is_supported() {
        assert_eq!(classify("/movies/Dune.Part.Two.2024.mkv"), FileKind::Supported);
        assert_eq!(classify("C:\\Movies\\LOUD.MKV"), FileKind::Supported);
        assert_eq!(classify("clip.mp4"), FileKind::OtherVideo);
        assert_eq!(classify("/a/b/episode.M2TS"), FileKind::OtherVideo);
        assert_eq!(classify("notes.txt"), FileKind::NotVideo);
        assert_eq!(classify("/movies/Season 1"), FileKind::NotVideo, "folders have no extension");
        assert_eq!(classify("/movies/.mkv"), FileKind::NotVideo, "dotfile, not an extension");
    }

    #[test]
    fn rejection_messages() {
        assert_eq!(rejection_message(&[]), None);
        let one_video = rejection_message(&[("a.mp4".into(), FileKind::OtherVideo)]).unwrap();
        assert!(one_video.contains("isn't an MKV file"), "{one_video}");
        let one_other = rejection_message(&[("a.txt".into(), FileKind::NotVideo)]).unwrap();
        assert!(one_other.contains("isn't a video file"), "{one_other}");
        let many: Vec<(String, FileKind)> =
            ["a.mp4", "b.txt", "c.jpg", "d.avi"].iter().map(|n| (n.to_string(), classify(n))).collect();
        assert_eq!(
            rejection_message(&many).unwrap(),
            "Skipped 4 files that aren't MKV video: a.mp4, b.txt, c.jpg and 1 more."
        );
    }
}
