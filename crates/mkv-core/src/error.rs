use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("could not launch `{binary}`: {source}")]
    MissingBinary {
        binary: String,
        #[source]
        source: std::io::Error,
    },

    #[error("`{binary} --version` exited with a non-zero status")]
    BinaryVersionCheckFailed { binary: String },

    #[error("failed to launch `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },

    #[error("`{program}` exited with an error status ({status}): {stderr}")]
    ProcessFailed {
        program: String,
        status: i32,
        stderr: String,
    },

    #[error("failed to parse mkvmerge JSON identify output: {0}")]
    ProbeParse(#[from] serde_json::Error),

    #[error("remux plan is invalid: {0}")]
    InvalidPlan(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
