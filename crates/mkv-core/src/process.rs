use crate::error::CoreError;
use async_trait::async_trait;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Called with a 0-100 percentage each time mkvmerge reports progress.
pub type ProgressSink = Box<dyn Fn(u8) + Send + Sync + 'static>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessOutput {
    pub stdout: String,
    pub stderr: String,
}

/// Paths (or bare names looked up on `PATH`) of the external tools this app
/// shells out to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPaths {
    pub mkvmerge: String,
    pub mkvpropedit: String,
    pub mediainfo: String,
}

impl Default for ToolPaths {
    fn default() -> Self {
        Self {
            mkvmerge: "mkvmerge".into(),
            mkvpropedit: "mkvpropedit".into(),
            mediainfo: "mediainfo".into(),
        }
    }
}

impl ToolPaths {
    /// Honors `MKV_MKVMERGE_BIN`, `MKV_MKVPROPEDIT_BIN`, `MKV_MEDIAINFO_BIN`.
    pub fn from_env() -> Self {
        let default = Self::default();
        let var = |name: &str, fallback: String| std::env::var(name).unwrap_or(fallback);
        Self {
            mkvmerge: var("MKV_MKVMERGE_BIN", default.mkvmerge),
            mkvpropedit: var("MKV_MKVPROPEDIT_BIN", default.mkvpropedit),
            mediainfo: var("MKV_MEDIAINFO_BIN", default.mediainfo),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaInfoOutput {
    /// `--Output=JSON`: machine-readable fields for analysis.
    Json,
    /// Default text report: human-readable values for the media-info viewer.
    Text,
}

/// Abstracts `mkvmerge`/`mkvpropedit`/`mediainfo` execution so command
/// construction and output parsing can be unit-tested without any of the
/// binaries installed.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    /// Runs `mkvmerge -J <path>` and returns raw stdout JSON.
    async fn identify(&self, path: &Path) -> Result<String, CoreError>;

    /// Runs `mkvmerge <args>` (expected to include `--gui-mode`), invoking
    /// `on_progress` for each `#GUI#progress N%` line seen on stdout.
    async fn run_mkvmerge(
        &self,
        args: Vec<String>,
        on_progress: ProgressSink,
    ) -> Result<ProcessOutput, CoreError>;

    /// Runs `mkvpropedit <args>`.
    async fn run_mkvpropedit(&self, args: Vec<String>) -> Result<ProcessOutput, CoreError>;

    /// Runs `mediainfo` on `path` and returns its stdout.
    async fn mediainfo(&self, path: &Path, output: MediaInfoOutput) -> Result<String, CoreError>;
}

/// Parses a `#GUI#progress N%` line as emitted by `mkvmerge --gui-mode`.
pub(crate) fn parse_gui_progress(line: &str) -> Option<u8> {
    let rest = line.strip_prefix("#GUI#progress ")?;
    let rest = rest.strip_suffix('%')?;
    rest.trim().parse::<u8>().ok()
}

/// Real `ProcessRunner` backed by `tokio::process`.
pub struct TokioProcessRunner {
    mkvmerge_bin: String,
    mkvpropedit_bin: String,
    mediainfo_bin: String,
}

impl TokioProcessRunner {
    pub fn new(tools: ToolPaths) -> Self {
        Self {
            mkvmerge_bin: tools.mkvmerge,
            mkvpropedit_bin: tools.mkvpropedit,
            mediainfo_bin: tools.mediainfo,
        }
    }

    /// Exit code 2 means mkvmerge/mkvpropedit failed outright; 0/1 mean
    /// success (1 = success with warnings).
    fn finish(program: &str, status_code: i32, stdout: Vec<u8>, stderr: Vec<u8>) -> Result<ProcessOutput, CoreError> {
        let stdout = String::from_utf8_lossy(&stdout).into_owned();
        let stderr = String::from_utf8_lossy(&stderr).into_owned();
        if status_code == 2 {
            return Err(CoreError::ProcessFailed {
                program: program.to_string(),
                status: status_code,
                stderr,
            });
        }
        Ok(ProcessOutput { stdout, stderr })
    }
}

#[async_trait]
impl ProcessRunner for TokioProcessRunner {
    async fn identify(&self, path: &Path) -> Result<String, CoreError> {
        let output = Command::new(&self.mkvmerge_bin)
            .arg("-J")
            .arg(path)
            .output()
            .await
            .map_err(|source| CoreError::Spawn {
                program: self.mkvmerge_bin.clone(),
                source,
            })?;
        let status_code = output.status.code().unwrap_or(-1);
        let result = Self::finish(&self.mkvmerge_bin, status_code, output.stdout, output.stderr)?;
        Ok(result.stdout)
    }

    async fn run_mkvmerge(
        &self,
        args: Vec<String>,
        on_progress: ProgressSink,
    ) -> Result<ProcessOutput, CoreError> {
        let mut child = Command::new(&self.mkvmerge_bin)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // If the caller drops this future (e.g. job cancellation), make
            // sure the child mkvmerge process actually dies with it instead
            // of running to completion in the background.
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| CoreError::Spawn {
                program: self.mkvmerge_bin.clone(),
                source,
            })?;

        // Drain stdout/stderr concurrently with waiting on the child so a
        // full pipe buffer can never deadlock the process.
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        let stdout_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let mut collected = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(pct) = parse_gui_progress(&line) {
                    on_progress(pct);
                }
                collected.push_str(&line);
                collected.push('\n');
            }
            collected
        });
        let stderr_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            let mut collected = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                collected.push_str(&line);
                collected.push('\n');
            }
            collected
        });

        let status = child.wait().await.map_err(|source| CoreError::Spawn {
            program: self.mkvmerge_bin.clone(),
            source,
        })?;
        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();
        let status_code = status.code().unwrap_or(-1);

        if status_code == 2 {
            return Err(CoreError::ProcessFailed {
                program: self.mkvmerge_bin.clone(),
                status: status_code,
                stderr,
            });
        }
        Ok(ProcessOutput { stdout, stderr })
    }

    async fn run_mkvpropedit(&self, args: Vec<String>) -> Result<ProcessOutput, CoreError> {
        let output = Command::new(&self.mkvpropedit_bin)
            .args(&args)
            .output()
            .await
            .map_err(|source| CoreError::Spawn {
                program: self.mkvpropedit_bin.clone(),
                source,
            })?;
        let status_code = output.status.code().unwrap_or(-1);
        Self::finish(&self.mkvpropedit_bin, status_code, output.stdout, output.stderr)
    }

    async fn mediainfo(&self, path: &Path, format: MediaInfoOutput) -> Result<String, CoreError> {
        let mut command = Command::new(&self.mediainfo_bin);
        if format == MediaInfoOutput::Json {
            command.arg("--Output=JSON");
        }
        let output = command
            .arg(path)
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|source| CoreError::Spawn {
                program: self.mediainfo_bin.clone(),
                source,
            })?;
        // Unlike MKVToolNix, MediaInfo uses plain 0 / non-zero exit codes.
        if !output.status.success() {
            return Err(CoreError::ProcessFailed {
                program: self.mediainfo_bin.clone(),
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[derive(Debug, Clone)]
pub enum RecordedCall {
    Identify(std::path::PathBuf),
    Mkvmerge(Vec<String>),
    Mkvpropedit(Vec<String>),
    MediaInfo(std::path::PathBuf, MediaInfoOutput),
}

/// Test double: records every invocation and returns canned output, so
/// command construction and output parsing can be verified without any of
/// the external tools installed.
pub struct MockProcessRunner {
    pub identify_stdout: String,
    pub mkvmerge_progress_lines: Vec<String>,
    pub mkvmerge_output: ProcessOutput,
    pub mkvpropedit_output: ProcessOutput,
    pub mediainfo_json: String,
    pub mediainfo_text: String,
    pub calls: Mutex<Vec<RecordedCall>>,
}

impl Default for MockProcessRunner {
    fn default() -> Self {
        Self {
            identify_stdout: String::new(),
            mkvmerge_progress_lines: Vec::new(),
            mkvmerge_output: ProcessOutput::default(),
            mkvpropedit_output: ProcessOutput::default(),
            mediainfo_json: r#"{"media":{"track":[]}}"#.to_string(),
            mediainfo_text: String::new(),
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ProcessRunner for MockProcessRunner {
    async fn identify(&self, path: &Path) -> Result<String, CoreError> {
        self.calls
            .lock()
            .unwrap()
            .push(RecordedCall::Identify(path.to_path_buf()));
        Ok(self.identify_stdout.clone())
    }

    async fn run_mkvmerge(
        &self,
        args: Vec<String>,
        on_progress: ProgressSink,
    ) -> Result<ProcessOutput, CoreError> {
        for line in &self.mkvmerge_progress_lines {
            if let Some(pct) = parse_gui_progress(line) {
                on_progress(pct);
            }
        }
        self.calls.lock().unwrap().push(RecordedCall::Mkvmerge(args));
        Ok(self.mkvmerge_output.clone())
    }

    async fn run_mkvpropedit(&self, args: Vec<String>) -> Result<ProcessOutput, CoreError> {
        self.calls
            .lock()
            .unwrap()
            .push(RecordedCall::Mkvpropedit(args));
        Ok(self.mkvpropedit_output.clone())
    }

    async fn mediainfo(&self, path: &Path, output: MediaInfoOutput) -> Result<String, CoreError> {
        self.calls
            .lock()
            .unwrap()
            .push(RecordedCall::MediaInfo(path.to_path_buf(), output));
        Ok(match output {
            MediaInfoOutput::Json => self.mediainfo_json.clone(),
            MediaInfoOutput::Text => self.mediainfo_text.clone(),
        })
    }
}
