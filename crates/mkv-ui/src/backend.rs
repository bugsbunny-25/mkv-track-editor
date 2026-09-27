//! The platform-I/O seam. Shared UI/view-model code only ever talks to this
//! trait, never to `rfd`, native process execution, or HTTP directly — that's
//! what keeps the `.slint` files and the view-model identical between the
//! native and wasm builds.
//!
//! Contract: every callback passed into a `Backend` method is guaranteed by
//! the implementation to be invoked on the Slint UI thread (i.e. the impl
//! does its own background work — a thread on desktop, a spawned future on
//! wasm — and delivers the result via `slint::invoke_from_event_loop`
//! itself). View-model code can therefore touch UI state directly inside
//! these callbacks without any further thread marshaling.

use mkv_profiles::AppConfig;
use mkv_types::{DirListing, FileAnalysis, JobId, JobOutcome, JobProgress, RootInfo, TrackEdit};

/// Where a source or output file lives, as produced by one platform's own
/// dialogs/browser and fed back into that same platform's `analyze` /
/// `submit_job`. The two variants are never mixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRef {
    /// Desktop: a trusted absolute path from a native file dialog.
    LocalPath(String),
    /// Web: a path relative to a configured server-side root.
    ServerPath { root_id: String, path: String },
}

impl SourceRef {
    /// Just the file name, for tab titles and the release-name parser.
    pub fn file_name(&self) -> &str {
        let path = match self {
            SourceRef::LocalPath(p) => p,
            SourceRef::ServerPath { path, .. } => path,
        };
        path.rsplit(['/', '\\']).next().unwrap_or(path)
    }

    pub fn display(&self) -> String {
        match self {
            SourceRef::LocalPath(p) => p.clone(),
            SourceRef::ServerPath { root_id, path } => format!("{root_id}:/{path}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UiError {
    pub message: String,
}

impl UiError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for UiError {}

#[derive(Debug, Clone, Copy, Default)]
pub struct BackendCapabilities {
    /// If true, the UI uses native Open/Save dialogs. If false, it shows the
    /// in-app `VolumeBrowser` and writes into the server's output root.
    pub native_file_dialog: bool,
}

pub type UiCallback<T> = Box<dyn FnOnce(Result<T, UiError>) + Send>;
pub type ProgressCallback = Box<dyn Fn(JobProgress) + Send + Sync>;

/// `Send + Sync` because callbacks are handed to background work (a thread on
/// desktop, a spawned future on wasm) and called back via
/// `slint::invoke_from_event_loop`, which requires `Send` all the way
/// through. View-model state shared with these callbacks must use
/// `Arc<Mutex<_>>`, not `Rc<RefCell<_>>`.
pub trait Backend: Send + Sync + 'static {
    fn capabilities(&self) -> BackendCapabilities;

    fn list_roots(&self, cb: UiCallback<Vec<RootInfo>>);
    fn list_dir(&self, root_id: String, path: String, cb: UiCallback<DirListing>);

    /// Desktop: native multi-select "Open" dialog. An empty list means the
    /// user cancelled.
    fn pick_open_paths(&self, cb: UiCallback<Vec<SourceRef>>);

    /// mkvmerge probe + MediaInfo analysis of one file.
    fn analyze(&self, source: SourceRef, cb: UiCallback<FileAnalysis>);

    /// Desktop: native "Save As" dialog, starting next to `source` with
    /// `suggested_name` prefilled. `Ok(None)` means cancelled.
    fn pick_save_path(&self, source: &SourceRef, suggested_name: String, cb: UiCallback<Option<SourceRef>>);

    /// Desktop: folder picker for "Save all". `Ok(None)` means cancelled.
    fn pick_output_dir(&self, cb: UiCallback<Option<String>>);

    /// Where a file named `file_name` goes when saved into `dir` (desktop) or
    /// the server's output root (web, `dir` ignored).
    fn output_in(&self, dir: Option<&str>, file_name: &str) -> SourceRef;

    fn submit_job(
        &self,
        source: SourceRef,
        output: SourceRef,
        kept_track_order: Vec<u64>,
        edits: Vec<TrackEdit>,
        cb: UiCallback<JobId>,
    );

    fn watch_job(&self, job: JobId, on_progress: ProgressCallback, on_done: UiCallback<JobOutcome>);

    fn cancel_job(&self, job: JobId);

    fn load_config(&self, cb: UiCallback<AppConfig>);
    fn save_config(&self, config: AppConfig, cb: UiCallback<()>);
}
