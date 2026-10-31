pub mod config_store;
pub mod error;
pub mod mediainfo;
pub mod mux;
pub mod plan;
pub mod probe;
pub mod process;
pub mod propedit;
pub mod remux;

pub use error::CoreError;
pub use mediainfo::analyze;
pub use plan::ExecutionStrategy;
pub use process::{
    MediaInfoOutput, MockProcessRunner, ProcessOutput, ProcessRunner, ProgressSink, TokioProcessRunner, ToolPaths,
};
pub use probe::probe;

use mkv_types::{JobOutcome, JobPhase, JobProgress, MuxPlan, ProbeResult, RemuxPlan};
use tokio::process::Command;

/// First MediaInfo release that reads HDR10+ (SMPTE ST 2094 App 4) from
/// Matroska; older versions silently report HDR10+ files as plain HDR10.
const MIN_MEDIAINFO_FOR_MKV_HDR10PLUS: (u32, u32) = (23, 4);

/// Verifies `mkvmerge`, `mkvpropedit` and `mediainfo` are installed and
/// runnable. Both desktop and server call this at startup and fail fast with
/// a clear error if any is missing. Returns non-fatal warnings (e.g. a
/// MediaInfo too old to detect HDR10+ in Matroska) for the caller to log.
pub async fn check_dependencies(tools: &ToolPaths) -> Result<Vec<String>, CoreError> {
    check_binary(&tools.mkvmerge, "--version").await?;
    check_binary(&tools.mkvpropedit, "--version").await?;
    let mediainfo_version = check_binary(&tools.mediainfo, "--Version").await?;

    let mut warnings = Vec::new();
    match parse_mediainfo_version(&mediainfo_version) {
        Some(v) if v < MIN_MEDIAINFO_FOR_MKV_HDR10PLUS => warnings.push(format!(
            "MediaInfo {}.{:02} is older than 23.04 and cannot detect HDR10+ in Matroska files; \
             HDR10+ content will be labeled as HDR10. Please upgrade MediaInfo.",
            v.0, v.1
        )),
        Some(_) => {}
        None => warnings.push("could not determine the MediaInfo version".to_string()),
    }
    Ok(warnings)
}

/// Runs `<bin> <version_flag>` and returns its stdout.
async fn check_binary(bin: &str, version_flag: &str) -> Result<String, CoreError> {
    let output = Command::new(bin)
        .arg(version_flag)
        .output()
        .await
        .map_err(|source| CoreError::MissingBinary {
            binary: bin.to_string(),
            source,
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(CoreError::BinaryVersionCheckFailed { binary: bin.to_string() })
    }
}

/// Extracts `(major, minor)` from MediaInfo's `--Version` output, e.g.
/// "MediaInfo Command line,\nMediaInfoLib - v26.05".
fn parse_mediainfo_version(output: &str) -> Option<(u32, u32)> {
    let after_v = output.split(" - v").nth(1)?;
    let mut parts = after_v.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()?;
    Some((major, minor))
}

/// Single entry point used identically by the desktop binary (in-process)
/// and the server binary (behind the HTTP API): validates the plan, decides
/// between the `mkvpropedit` fast path and a full `mkvmerge` remux, runs it,
/// and reports progress via `on_progress`.
pub async fn execute_plan(
    runner: &dyn ProcessRunner,
    original: &ProbeResult,
    plan: &RemuxPlan,
    on_progress: Box<dyn Fn(JobProgress) + Send + Sync + 'static>,
) -> Result<JobOutcome, CoreError> {
    self::plan::validate(plan)?;
    self::plan::validate_attachments(original, plan)?;

    on_progress(JobProgress {
        phase: JobPhase::Running,
        percent: 0,
        message: None,
        output_path: None,
    });

    match self::plan::classify(original, plan) {
        ExecutionStrategy::MetadataOnly => {
            tracing::info!(source = %plan.source_path, "taking mkvpropedit fast path (metadata-only edit)");
            propedit::run(runner, plan).await?;
        }
        ExecutionStrategy::Remux => {
            // mkvmerge reads the source while writing the output; writing
            // over the input would destroy it. (The mkvpropedit path above
            // legitimately edits in place.)
            if same_file(&plan.source_path, &plan.output_path) {
                return Err(CoreError::InvalidPlan(
                    "removing or reordering tracks needs a new output file; choose a different name or folder"
                        .into(),
                ));
            }
            tracing::info!(source = %plan.source_path, "running full mkvmerge remux");
            let on_progress = std::sync::Arc::new(on_progress);
            let sink: ProgressSink = Box::new(move |pct: u8| {
                on_progress(JobProgress {
                    phase: JobPhase::Running,
                    percent: pct,
                    message: None,
                    output_path: None,
                });
            });
            remux::run(runner, plan, sink).await?;
        }
    }

    Ok(JobOutcome {
        output_path: plan.output_path.clone(),
    })
}

/// Like [`execute_plan`] for several source files combined into one new
/// output. `originals` are the inputs' current probe results, in input order.
pub async fn execute_mux(
    runner: &dyn ProcessRunner,
    originals: &[ProbeResult],
    plan: &MuxPlan,
    on_progress: Box<dyn Fn(JobProgress) + Send + Sync + 'static>,
) -> Result<JobOutcome, CoreError> {
    mux::validate(originals, plan)?;
    // mkvmerge reads every input while writing the output.
    if plan.inputs.iter().any(|input| same_file(&input.source_path, &plan.output_path)) {
        return Err(CoreError::InvalidPlan(
            "the muxed file can't replace one of its sources; choose a different name or folder".into(),
        ));
    }

    let on_progress = std::sync::Arc::new(on_progress);
    on_progress(JobProgress { phase: JobPhase::Running, percent: 0, message: None, output_path: None });
    tracing::info!(inputs = plan.inputs.len(), output = %plan.output_path, "running mkvmerge mux");
    let sink: ProgressSink = Box::new(move |pct: u8| {
        on_progress(JobProgress { phase: JobPhase::Running, percent: pct, message: None, output_path: None });
    });
    mux::run(runner, plan, sink).await?;

    Ok(JobOutcome {
        output_path: plan.output_path.clone(),
    })
}

/// Compares canonicalized paths when both exist (catches `./a.mkv` vs
/// `/dir/a.mkv`, symlinks), falling back to a plain string comparison.
fn same_file(a: &str, b: &str) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mkv_types::{ContainerInfo, TrackEdit, TrackFlags, TrackInfo, TrackType};

    #[tokio::test]
    async fn remux_refuses_to_overwrite_its_own_input() {
        let track = |id| TrackInfo {
            id,
            uid: id + 10,
            track_type: TrackType::Audio,
            codec: "AAC".into(),
            language: None,
            name: None,
            flags: TrackFlags::default(),
        };
        let edit = |id| TrackEdit {
            id,
            uid: id + 10,
            track_type: TrackType::Audio,
            name: None,
            language: None,
            flags: TrackFlags::default(),
        };
        let original = ProbeResult { container: ContainerInfo::default(), tracks: vec![track(0), track(1)], attachments: vec![] };
        let runner = MockProcessRunner::default();

        // Reorder (needs remux) onto the same path → refused, nothing run.
        let plan = RemuxPlan {
            source_path: "/lib/a.mkv".into(),
            output_path: "/lib/a.mkv".into(),
            kept_track_order: vec![1, 0],
            edits: vec![edit(1), edit(0)],
            remove_attachments: vec![],
        };
        let err = execute_plan(&runner, &original, &plan, Box::new(|_| {})).await.unwrap_err();
        assert!(err.to_string().contains("new output file"), "{err}");
        assert!(runner.calls.lock().unwrap().is_empty());

        // Metadata-only onto the same path → in-place mkvpropedit is fine.
        let plan = RemuxPlan { kept_track_order: vec![0, 1], edits: vec![edit(0), edit(1)], ..plan };
        execute_plan(&runner, &original, &plan, Box::new(|_| {})).await.unwrap();
        assert_eq!(runner.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn mux_refuses_to_overwrite_a_source() {
        use mkv_types::{MuxInput, MuxTrack};
        let track = TrackInfo {
            id: 0,
            uid: 10,
            track_type: TrackType::Video,
            codec: "HEVC".into(),
            language: None,
            name: None,
            flags: TrackFlags::default(),
        };
        let original = ProbeResult { container: ContainerInfo::default(), tracks: vec![track], attachments: vec![] };
        let edit = TrackEdit { id: 0, uid: 10, track_type: TrackType::Video, name: None, language: None, flags: TrackFlags::default() };
        let plan = MuxPlan {
            inputs: vec![
                MuxInput { source_path: "/lib/a.mkv".into(), remove_attachments: vec![] },
                MuxInput { source_path: "/lib/b.mkv".into(), remove_attachments: vec![] },
            ],
            output_path: "/lib/b.mkv".into(),
            tracks: vec![MuxTrack { input: 0, edit: edit.clone() }, MuxTrack { input: 1, edit }],
        };
        let runner = MockProcessRunner::default();
        let originals = [original.clone(), original];
        let err = execute_mux(&runner, &originals, &plan, Box::new(|_| {})).await.unwrap_err();
        assert!(err.to_string().contains("can't replace one of its sources"), "{err}");
        assert!(runner.calls.lock().unwrap().is_empty());

        let plan = MuxPlan { output_path: "/out/muxed.mkv".into(), ..plan };
        execute_mux(&runner, &originals, &plan, Box::new(|_| {})).await.unwrap();
        assert_eq!(runner.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn parses_mediainfo_version_output() {
        assert_eq!(
            parse_mediainfo_version("MediaInfo Command line, \nMediaInfoLib - v26.05\n"),
            Some((26, 5))
        );
        assert_eq!(parse_mediainfo_version("MediaInfoLib - v23.01.1"), Some((23, 1)));
        assert_eq!(parse_mediainfo_version("garbage"), None);
        assert!((23, 1) < MIN_MEDIAINFO_FOR_MKV_HDR10PLUS);
        assert!((26, 5) >= MIN_MEDIAINFO_FOR_MKV_HDR10PLUS);
    }
}
