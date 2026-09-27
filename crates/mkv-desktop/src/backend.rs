//! [`mkv_ui::Backend`] implementation for the native desktop app: native
//! file dialogs via `rfd` for trusted absolute paths, and `mkv-core` called
//! in-process (spawned onto a background tokio runtime, never blocking the
//! Slint UI thread) for analysis and remuxing.

use mkv_core::{analyze, config_store, execute_plan, probe, TokioProcessRunner, ToolPaths};
use mkv_profiles::AppConfig;
use mkv_types::{
    AttachmentInfo, DirListing, FileAnalysis, JobId, JobOutcome, JobProgress, ProbeResult, RemuxPlan, RootInfo, TrackEdit,
};
use mkv_ui::{Backend, BackendCapabilities, ProgressCallback, SourceRef, UiCallback, UiError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

struct PendingJob {
    original: ProbeResult,
    plan: RemuxPlan,
}

pub struct DesktopBackend {
    rt: Handle,
    runner: Arc<TokioProcessRunner>,
    config_path: PathBuf,
    pending: Arc<Mutex<HashMap<JobId, PendingJob>>>,
    running: Arc<Mutex<HashMap<JobId, CancellationToken>>>,
}

impl DesktopBackend {
    pub fn new(rt: Handle, tools: ToolPaths, config_path: PathBuf) -> Self {
        Self {
            rt,
            runner: Arc::new(TokioProcessRunner::new(tools)),
            config_path,
            pending: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Delivers a callback result on the Slint UI thread from whatever
/// background thread/task we're currently on.
fn deliver<T: Send + 'static>(cb: UiCallback<T>, result: Result<T, UiError>) {
    let _ = slint::invoke_from_event_loop(move || cb(result));
}

fn local(source: SourceRef) -> Result<String, UiError> {
    match source {
        SourceRef::LocalPath(p) => Ok(p),
        SourceRef::ServerPath { .. } => Err(UiError::new("invalid source for desktop backend")),
    }
}

impl Backend for DesktopBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities { native_file_dialog: true }
    }

    fn list_roots(&self, cb: UiCallback<Vec<RootInfo>>) {
        // Desktop browses the filesystem via native dialogs, not server roots.
        cb(Ok(Vec::new()));
    }

    fn list_dir(&self, _root_id: String, _path: String, cb: UiCallback<DirListing>) {
        cb(Err(UiError::new("directory browsing is not available in the desktop build")));
    }

    fn pick_open_paths(&self, cb: UiCallback<Vec<SourceRef>>) {
        // Native dialogs are modal; showing one synchronously from a UI
        // callback (already on the UI thread) is the standard pattern.
        let picked = rfd::FileDialog::new()
            .add_filter("Matroska", mkv_ui::file_types::SUPPORTED_EXTENSIONS)
            .set_title("Open MKV files")
            .pick_files()
            .unwrap_or_default();
        cb(Ok(picked
            .into_iter()
            .map(|p| SourceRef::LocalPath(p.to_string_lossy().into_owned()))
            .collect()));
    }

    fn analyze(&self, source: SourceRef, cb: UiCallback<FileAnalysis>) {
        let path = match local(source) {
            Ok(p) => p,
            Err(e) => return cb(Err(e)),
        };
        let runner = self.runner.clone();
        self.rt.spawn(async move {
            let result = analyze(runner.as_ref(), Path::new(&path))
                .await
                .map_err(|e| UiError::new(e.to_string()));
            deliver(cb, result);
        });
    }

    fn pick_save_path(&self, source: &SourceRef, suggested_name: String, cb: UiCallback<Option<SourceRef>>) {
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Matroska", &["mkv"])
            .set_file_name(&suggested_name)
            .set_title("Save edited MKV as");
        if let SourceRef::LocalPath(p) = source {
            if let Some(dir) = Path::new(p).parent() {
                dialog = dialog.set_directory(dir);
            }
        }
        let picked = dialog.save_file();
        cb(Ok(picked.map(|p| SourceRef::LocalPath(p.to_string_lossy().into_owned()))));
    }

    fn pick_output_dir(&self, cb: UiCallback<Option<String>>) {
        let picked = rfd::FileDialog::new()
            .set_title("Save all edited files into…")
            .pick_folder();
        cb(Ok(picked.map(|p| p.to_string_lossy().into_owned())));
    }

    fn output_in(&self, dir: Option<&str>, file_name: &str) -> SourceRef {
        let path = match dir {
            Some(dir) => Path::new(dir).join(file_name),
            None => PathBuf::from(file_name),
        };
        SourceRef::LocalPath(path.to_string_lossy().into_owned())
    }

    fn submit_job(
        &self,
        source: SourceRef,
        output: SourceRef,
        kept_track_order: Vec<u64>,
        edits: Vec<TrackEdit>,
        remove_attachments: Vec<AttachmentInfo>,
        cb: UiCallback<JobId>,
    ) {
        let (source_path, output_path) = match (local(source), local(output)) {
            (Ok(s), Ok(o)) => (s, o),
            (Err(e), _) | (_, Err(e)) => return cb(Err(e)),
        };
        let plan = RemuxPlan {
            source_path,
            output_path,
            kept_track_order,
            edits,
            remove_attachments,
        };
        if let Err(e) = mkv_core::plan::validate(&plan) {
            return cb(Err(UiError::new(e.to_string())));
        }

        let runner = self.runner.clone();
        let pending = self.pending.clone();
        let job_id = JobId::new();

        self.rt.spawn(async move {
            // Re-probe at submit time so `classify()` compares against an
            // authoritative, current track order rather than a possibly
            // stale one from whenever the file was first opened.
            match probe(runner.as_ref(), Path::new(&plan.source_path)).await {
                Ok(original) => {
                    pending.lock().unwrap().insert(job_id.clone(), PendingJob { original, plan });
                    deliver(cb, Ok(job_id));
                }
                Err(e) => deliver(cb, Err(UiError::new(e.to_string()))),
            }
        });
    }

    fn watch_job(&self, job: JobId, on_progress: ProgressCallback, on_done: UiCallback<JobOutcome>) {
        let pending_job = self.pending.lock().unwrap().remove(&job);
        let Some(PendingJob { original, plan }) = pending_job else {
            deliver(on_done, Err(UiError::new("unknown job")));
            return;
        };

        let cancel_token = CancellationToken::new();
        self.running.lock().unwrap().insert(job.clone(), cancel_token.clone());
        let running = self.running.clone();
        let runner = self.runner.clone();

        self.rt.spawn(async move {
            let on_progress = Arc::new(on_progress);
            let sink: Box<dyn Fn(JobProgress) + Send + Sync> = {
                let on_progress = on_progress.clone();
                Box::new(move |p: JobProgress| {
                    let on_progress = on_progress.clone();
                    let _ = slint::invoke_from_event_loop(move || on_progress(p));
                })
            };

            let outcome = tokio::select! {
                r = execute_plan(runner.as_ref(), &original, &plan, sink) => {
                    r.map_err(|e| UiError::new(e.to_string()))
                }
                _ = cancel_token.cancelled() => Err(UiError::new("cancelled")),
            };

            running.lock().unwrap().remove(&job);
            deliver(on_done, outcome);
        });
    }

    fn cancel_job(&self, job: JobId) {
        if let Some(token) = self.running.lock().unwrap().remove(&job) {
            token.cancel();
        }
    }

    fn load_config(&self, cb: UiCallback<AppConfig>) {
        let path = self.config_path.clone();
        self.rt.spawn(async move {
            let result = config_store::load_or_init(&path)
                .await
                .map_err(|e| UiError::new(e.to_string()));
            deliver(cb, result);
        });
    }

    fn save_config(&self, config: AppConfig, cb: UiCallback<()>) {
        let path = self.config_path.clone();
        self.rt.spawn(async move {
            let result = config_store::save(&path, &config)
                .await
                .map_err(|e| UiError::new(e.to_string()));
            deliver(cb, result);
        });
    }
}
