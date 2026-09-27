//! Profiles, labels and Radarr/Sonarr-compatible naming. Pure logic with no
//! I/O, so it runs identically in the native binaries and the wasm UI.

pub mod apply;
pub mod config;
pub mod labels;
pub mod lang;
pub mod naming;
pub mod release;
pub mod template;

pub use apply::{apply_profile, guess_original_language, track_name, ProfileOutcome, TrackProposal};
pub use config::{AppConfig, ConfigError, Profile, SubtitleKind};
pub use naming::{render_file_name, summarize, MediaSummary, NamingMetadata};
pub use release::{MediaKind, QualitySource, Revision};
