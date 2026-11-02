use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VolumeError {
    #[error("unknown root id `{0}`")]
    UnknownRoot(String),

    #[error("path escapes the configured root")]
    PathEscape,

    #[error("path must be relative, not absolute")]
    AbsolutePath,

    #[error("path has no valid file name")]
    InvalidComponent,

    #[error("not a file")]
    NotAFile,

    #[error("io error accessing `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid volume configuration: {0}")]
    InvalidConfig(String),
}
