//! Wires the shared `.slint` UI (via the `Bridge` global) to the pure
//! document/settings logic and to a [`Backend`]. Identical for desktop and
//! wasm; the only platform difference is which `Backend` is passed in and
//! the `native_file_dialog` capability it reports.
//!
//! Locking rule: never hold the state lock while calling into the backend —
//! desktop dialogs invoke their callback synchronously, and that callback
//! locks the state again.

use crate::backend::{Backend, ProgressCallback, SourceRef, UiCallback};
use crate::document::{Document, FactKind, Flag, JobState};
use crate::settings::{self, NamingForm, ProfileForm, COLON_OPTIONS};
use crate::file_types::{self, FileKind};
use crate::{AppWindow, BadgeData, Bridge, DirRow, DocTabData, DropState, InfoRowData, Tone, TrackKind, TrackRowData};
use mkv_profiles::labels::{audio_attrs, video_attrs};
use mkv_profiles::{lang, AppConfig, MediaKind, QualitySource, Revision};
use mkv_types::{FileAnalysis, JobOutcome, JobProgress, TrackType};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const REVISIONS: [Revision; 3] = [Revision::None, Revision::Proper, Revision::Repack];

struct SettingsState {
    draft: AppConfig,
    profile_index: usize,
    dirty: bool,
    errors: String,
}

struct State {
    native: bool,
    config: AppConfig,
    docs: Vec<Document>,
    active: Option<usize>,
    next_id: u64,
    show_settings: bool,
    show_browser: bool,
    /// Analyses in flight.
    loading: usize,
    browse_root: String,
    browse_path: String,
    /// Codes behind the language ComboBox as last shown ("" = not set).
    language_codes: Vec<String>,
    settings: SettingsState,
    /// "Save all": (doc id, output) waiting to run, one job at a time.
    save_queue: VecDeque<(u64, SourceRef)>,
    error: String,
    /// Files currently dragged over the window.
    drag: DragState,
}

/// The OS reports a drag one file at a time (hover events), then delivers
/// the drop one file at a time too; this accumulates both.
#[derive(Default)]
struct DragState {
    openable: usize,
    rejected: usize,
    /// Dropped files that were turned away, reported together once the
    /// whole drop has been delivered.
    dropped_rejects: Vec<(String, FileKind)>,
    flush_scheduled: bool,
}

impl State {
    fn active_doc(&self) -> Option<&Document> {
        self.active.and_then(|i| self.docs.get(i))
    }

    fn doc_index(&self, id: u64) -> Option<usize> {
        self.docs.iter().position(|d| d.id == id)
    }
}

#[derive(Clone)]
struct Ctx {
    window: slint::Weak<AppWindow>,
    backend: Arc<dyn Backend>,
    state: Arc<Mutex<State>>,
}

impl Ctx {
    /// Runs `f` with the Bridge and the locked state, if the window is alive.
    fn with<R>(&self, f: impl FnOnce(&Bridge, &mut State) -> R) -> Option<R> {
        let window = self.window.upgrade()?;
        let bridge = window.global::<Bridge>();
        let mut state = self.state.lock().unwrap();
        Some(f(&bridge, &mut state))
    }

    /// Like `with`, but for the active document (together with the config).
    fn with_doc<R>(&self, f: impl FnOnce(&mut Document, &AppConfig) -> R) -> Option<R> {
        let mut st = self.state.lock().unwrap();
        let State { docs, config, active, .. } = &mut *st;
        let doc = docs.get_mut((*active)?)?;
        Some(f(doc, config))
    }
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(items.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

/// Wires the shared `.slint` UI to `backend` and runs the Slint event loop.
pub fn launch(backend: impl Backend) -> Result<(), slint::PlatformError> {
    App::new(backend)?.run()
}

/// The wired-up application before its event loop runs. Platforms that need
/// to hook into the window (e.g. desktop drag and drop) do so between
/// [`App::new`] and [`App::run`].
pub struct App {
    window: AppWindow,
    ctx: Ctx,
}

/// A cheap, cloneable handle for feeding platform events into the UI. Use
/// it only on the UI thread.
#[derive(Clone)]
pub struct AppHandle {
    ctx: Ctx,
}

impl App {
    pub fn new(backend: impl Backend) -> Result<Self, slint::PlatformError> {
        new_app(Arc::new(backend))
    }

    pub fn window(&self) -> &AppWindow {
        &self.window
    }

    pub fn handle(&self) -> AppHandle {
        AppHandle { ctx: self.ctx.clone() }
    }

    pub fn run(self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }
}

impl AppHandle {
    /// A file is being dragged over the window (called once per file).
    pub fn drag_entered(&self, path: &str) {
        self.ctx.with(|b, st| {
            if file_types::classify(path).is_supported() {
                st.drag.openable += 1;
            } else {
                st.drag.rejected += 1;
            }
            sync_drop(b, st);
        });
    }

    /// The drag left the window or was cancelled.
    pub fn drag_left(&self) {
        self.ctx.with(|b, st| {
            st.drag.openable = 0;
            st.drag.rejected = 0;
            sync_drop(b, st);
        });
    }

    /// A file was dropped (called once per file). MKV files open in new
    /// tabs; anything else is reported in one message after the drop.
    pub fn file_dropped(&self, path: String) {
        let kind = file_types::classify(&path);
        let schedule_flush = self
            .ctx
            .with(|b, st| {
                st.drag.openable = 0;
                st.drag.rejected = 0;
                sync_drop(b, st);
                if kind.is_supported() {
                    return false;
                }
                let name = SourceRef::LocalPath(path.clone()).file_name().to_string();
                st.drag.dropped_rejects.push((name, kind));
                !std::mem::replace(&mut st.drag.flush_scheduled, true)
            })
            .unwrap_or(false);

        if kind.is_supported() {
            open_source(&self.ctx, SourceRef::LocalPath(path));
        } else if schedule_flush {
            // The remaining files of this drop arrive as separate events in
            // the same event-loop turn; report after all of them.
            let ctx = self.ctx.clone();
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                ctx.with(|b, st| {
                    let rejected = std::mem::take(&mut st.drag.dropped_rejects);
                    st.drag.flush_scheduled = false;
                    if let Some(message) = file_types::rejection_message(&rejected) {
                        st.error = message;
                        sync_nav(b, st);
                    }
                });
            });
        }
    }
}

fn new_app(backend: Arc<dyn Backend>) -> Result<App, slint::PlatformError> {
    let window = AppWindow::new()?;
    let native = backend.capabilities().native_file_dialog;
    let config = AppConfig::default();

    let state = State {
        native,
        settings: SettingsState {
            draft: config.clone(),
            profile_index: 0,
            dirty: false,
            errors: String::new(),
        },
        config,
        docs: Vec::new(),
        active: None,
        next_id: 1,
        show_settings: false,
        // Web: the in-app browser *is* the "open" screen.
        show_browser: !native,
        loading: 0,
        browse_root: String::new(),
        browse_path: String::new(),
        language_codes: Vec::new(),
        save_queue: VecDeque::new(),
        error: String::new(),
        drag: DragState::default(),
    };
    let ctx = Ctx {
        window: window.as_weak(),
        backend,
        state: Arc::new(Mutex::new(state)),
    };

    {
        let b = window.global::<Bridge>();
        b.set_native_dialogs(native);
        b.set_source_names(strings(QualitySource::ALL.iter().map(|s| s.label().to_string())));
        b.set_revision_names(strings(["None", "Proper", "Repack"].map(String::from)));
        b.set_s_colon_names(strings(COLON_OPTIONS.iter().map(|(_, l)| l.to_string())));
    }

    wire_navigation(&window, &ctx);
    wire_browser(&window, &ctx);
    wire_document(&window, &ctx);
    wire_tracks(&window, &ctx);
    wire_naming(&window, &ctx);
    wire_jobs(&window, &ctx);
    wire_settings(&window, &ctx);

    ctx.with(sync_all);
    load_config(&ctx);
    if !native {
        load_roots(&ctx);
    }

    Ok(App { window, ctx })
}

// ---------------------------------------------------------------------------
// Sync: state → Bridge
// ---------------------------------------------------------------------------

fn sync_all(b: &Bridge, st: &mut State) {
    sync_nav(b, st);
    sync_doc(b, st);
    sync_settings_form(b, st);
    sync_settings_status(b, st);
}

fn sync_nav(b: &Bridge, st: &State) {
    let tabs: Vec<DocTabData> = st
        .docs
        .iter()
        .map(|d| {
            let (busy, progress) = match d.job {
                JobState::Running { percent, .. } => (true, percent as i32),
                JobState::Queued => (true, 0),
                _ => (false, 0),
            };
            DocTabData {
                title: d.title().into(),
                changed: d.has_changes(),
                busy,
                progress,
                failed: matches!(d.job, JobState::Failed { .. }),
                done: matches!(d.job, JobState::Done { .. }),
            }
        })
        .collect();
    b.set_tabs(ModelRc::new(VecModel::from(tabs)));
    b.set_active_tab(st.active.map(|i| i as i32).unwrap_or(-1));
    b.set_show_settings(st.show_settings);
    b.set_show_browser(st.show_browser);
    b.set_loading(st.loading > 0);
    b.set_error_message(st.error.clone().into());
    b.set_doc_count(st.docs.len() as i32);
    b.set_any_busy(st.docs.iter().any(|d| d.job.is_busy()));
    if let Some(doc) = st.active_doc() {
        sync_header(b, doc);
    }
}

fn sync_drop(b: &Bridge, st: &State) {
    let DragState { openable, rejected, .. } = st.drag;
    b.set_drop_state(match (openable, rejected) {
        (0, 0) => DropState::None,
        (0, _) => DropState::Reject,
        _ => DropState::Accept,
    });
    b.set_drop_count(openable as i32);
}

/// Title, file name, media facts and change count. Cheap, so it runs with
/// every nav sync — the facts follow which tracks are kept.
fn sync_header(b: &Bridge, doc: &Document) {
    b.set_doc_title(doc.display_title().into());
    b.set_doc_subtitle(doc.source.display().into());
    let badges: Vec<BadgeData> = doc
        .header_facts()
        .into_iter()
        .map(|fact| BadgeData { text: fact.text.into(), tone: fact_tone(fact.kind) })
        .collect();
    b.set_doc_badges(ModelRc::new(VecModel::from(badges)));
    b.set_change_count(doc.change_count() as i32);
}

fn fact_tone(kind: FactKind) -> Tone {
    match kind {
        FactKind::Video => Tone::Video,
        FactKind::HighDynamicRange => Tone::Warning,
        FactKind::Audio => Tone::Audio,
        FactKind::Subtitle => Tone::Subtitle,
        FactKind::Detail => Tone::Neutral,
    }
}

/// Everything for the active document, including the text fields (only
/// called on tab switches / structural changes, never while the user types
/// into those fields).
fn sync_doc(b: &Bridge, st: &mut State) {
    let Some(doc) = st.active_doc().cloned() else {
        b.set_tracks(ModelRc::default());
        b.set_warnings(ModelRc::default());
        return;
    };

    let profile_names: Vec<String> = st.config.profiles.iter().map(|p| p.name.clone()).collect();
    b.set_profile_index(profile_names.iter().position(|n| *n == doc.profile).unwrap_or(0) as i32);
    b.set_profile_names(strings(profile_names));

    let mut codes = doc.language_options();
    let mut labels: Vec<String> = codes
        .iter()
        .map(|c| format!("{} ({c})", lang::display_name(c)))
        .collect();
    if doc.original_language.is_none() {
        codes.insert(0, String::new());
        labels.insert(0, "Not set".into());
    }
    let index = doc
        .original_language
        .as_ref()
        .and_then(|o| codes.iter().position(|c| c == o))
        .unwrap_or(0);
    b.set_language_labels(strings(labels));
    b.set_language_index(index as i32);
    st.language_codes = codes;

    sync_tracks(b, &doc, true);
    sync_warnings(b, &doc);

    b.set_info_sections(strings(doc.analysis.sections.iter().map(|s| s.title.clone())));
    b.set_info_filter(doc.info_filter.clone().into());
    sync_info(b, &doc);

    sync_naming_form(b, &doc);
    sync_output(b, &doc, &st.config, true);
    sync_job(b, &doc);
}

fn sync_warnings(b: &Bridge, doc: &Document) {
    b.set_warnings(strings(doc.warnings.iter().cloned()));
}

fn track_row(doc: &Document, i: usize) -> TrackRowData {
    let row = &doc.rows[i];
    let analysis: &FileAnalysis = &doc.analysis;
    let media = analysis.media.track(row.edit.uid);
    let mkv_codec = analysis
        .probe
        .tracks
        .iter()
        .find(|t| t.uid == row.edit.uid)
        .map(|t| t.codec.clone())
        .unwrap_or_default();
    let (kind, codec, detail) = match row.edit.track_type {
        TrackType::Video => {
            let v = video_attrs(media, &mkv_codec);
            let mut parts = Vec::new();
            if let Some(m) = media {
                if let (Some(w), Some(h)) = (m.width, m.height) {
                    parts.push(format!("{w}×{h}"));
                }
            }
            parts.push(v.dynamic_range.label());
            if let Some(bits) = v.bit_depth {
                parts.push(format!("{bits}-bit"));
            }
            (TrackKind::Video, v.codec, parts.join(" · "))
        }
        TrackType::Audio => {
            let a = audio_attrs(media, &mkv_codec);
            let mut parts = Vec::new();
            if let Some(ch) = &a.channels {
                parts.push(ch.clone());
            }
            if a.atmos {
                parts.push("Atmos".into());
            }
            if let Some(rate) = media.and_then(|m| m.sampling_rate) {
                parts.push(format!("{:.1} kHz", rate as f64 / 1000.0));
            }
            (TrackKind::Audio, a.format, parts.join(" · "))
        }
        TrackType::Subtitles => {
            let detail = media
                .and_then(|m| m.element_count)
                .map(|n| format!("{n} events"))
                .unwrap_or_default();
            (TrackKind::Subtitle, mkv_codec.clone(), detail)
        }
    };
    let ordinal = doc.rows[..=i].iter().filter(|r| r.edit.track_type == row.edit.track_type).count();
    let language_name = row
        .edit
        .language
        .as_deref()
        .filter(|l| !lang::is_undetermined(l))
        .map(lang::display_name)
        .unwrap_or_default();
    let f = row.edit.flags;
    TrackRowData {
        keep: row.keep,
        kind,
        ordinal: ordinal as i32,
        language_name: language_name.into(),
        codec: codec.into(),
        detail: detail.into(),
        name: row.edit.name.clone().unwrap_or_default().into(),
        language: row.edit.language.clone().unwrap_or_default().into(),
        is_default: f.default,
        is_forced: f.forced,
        is_original: f.original,
        is_hi: f.hearing_impaired,
        is_vi: f.visual_impaired,
        is_commentary: f.commentary,
        can_up: i > 0 && doc.can_move(i, i - 1),
        can_down: doc.can_move(i, i + 1),
    }
}

/// `rebuild` replaces the model (tab switch, reorder, profile apply).
/// Otherwise rows are updated in place so the list doesn't lose its scroll
/// position or the focused editor.
fn sync_tracks(b: &Bridge, doc: &Document, rebuild: bool) {
    let rows: Vec<TrackRowData> = (0..doc.rows.len()).map(|i| track_row(doc, i)).collect();
    if !rebuild {
        let model = b.get_tracks();
        if let Some(vm) = model.as_any().downcast_ref::<VecModel<TrackRowData>>() {
            if vm.row_count() == rows.len() {
                for (i, row) in rows.into_iter().enumerate() {
                    if vm.row_data(i).as_ref() != Some(&row) {
                        vm.set_row_data(i, row);
                    }
                }
                return;
            }
        }
    }
    b.set_tracks(ModelRc::new(VecModel::from(rows)));
}

fn sync_info(b: &Bridge, doc: &Document) {
    b.set_info_section_index(doc.info_section as i32);
    let rows: Vec<InfoRowData> = doc
        .info_rows()
        .into_iter()
        .map(|(key, value)| InfoRowData { key: key.into(), value: value.into() })
        .collect();
    b.set_info_rows(ModelRc::new(VecModel::from(rows)));
}

fn episodes_to_text(episodes: &[u32]) -> String {
    let consecutive = episodes.windows(2).all(|w| w[1] == w[0] + 1);
    match episodes {
        [] => String::new(),
        [one] => one.to_string(),
        [first, .., last] if consecutive => format!("{first}-{last}"),
        many => many.iter().map(u32::to_string).collect::<Vec<_>>().join(","),
    }
}

fn episodes_from_text(text: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().trim_start_matches(['E', 'e']).parse::<u32>()) {
                    if a <= b && b - a < 100 {
                        out.extend(a..=b);
                    }
                }
            }
            None => out.extend(part.parse::<u32>().ok()),
        }
    }
    out
}

fn sync_naming_form(b: &Bridge, doc: &Document) {
    let m = &doc.metadata;
    b.set_kind_index(if m.kind == MediaKind::Episode { 1 } else { 0 });
    b.set_meta_title(m.title.clone().into());
    b.set_meta_year(m.year.map(|y| y.to_string()).unwrap_or_default().into());
    b.set_meta_tmdb(m.tmdb_id.clone().into());
    b.set_meta_imdb(m.imdb_id.clone().into());
    b.set_meta_tvdb(m.tvdb_id.clone().into());
    b.set_meta_edition(m.edition.clone().into());
    b.set_meta_custom_formats(m.custom_formats.clone().into());
    b.set_meta_group(m.release_group.clone().into());
    b.set_meta_season(m.season.map(|s| s.to_string()).unwrap_or_default().into());
    b.set_meta_episodes(episodes_to_text(&m.episodes).into());
    b.set_meta_episode_title(m.episode_title.clone().into());
    b.set_source_index(QualitySource::ALL.iter().position(|s| *s == m.source).unwrap_or(0) as i32);
    b.set_revision_index(REVISIONS.iter().position(|r| *r == m.revision).unwrap_or(0) as i32);
}

fn read_naming_form(b: &Bridge, doc: &mut Document) {
    let m = &mut doc.metadata;
    m.kind = if b.get_kind_index() == 1 { MediaKind::Episode } else { MediaKind::Movie };
    m.title = b.get_meta_title().trim().to_string();
    m.year = b.get_meta_year().trim().parse().ok();
    m.tmdb_id = b.get_meta_tmdb().trim().to_string();
    m.imdb_id = b.get_meta_imdb().trim().to_string();
    m.tvdb_id = b.get_meta_tvdb().trim().to_string();
    m.edition = b.get_meta_edition().trim().to_string();
    m.custom_formats = b.get_meta_custom_formats().trim().to_string();
    m.release_group = b.get_meta_group().trim().to_string();
    m.season = b.get_meta_season().trim().parse().ok();
    m.episodes = episodes_from_text(&b.get_meta_episodes());
    m.episode_title = b.get_meta_episode_title().trim().to_string();
    m.source = QualitySource::ALL
        .get(b.get_source_index().max(0) as usize)
        .copied()
        .unwrap_or_default();
    m.revision = REVISIONS.get(b.get_revision_index().max(0) as usize).copied().unwrap_or_default();
}

/// `set_text` is false while the user is typing in the output field.
fn sync_output(b: &Bridge, doc: &Document, config: &AppConfig, set_text: bool) {
    let (auto, unknown) = doc.auto_output_name(config);
    b.set_preview_name(auto.into());
    b.set_preview_problem(if unknown.is_empty() {
        SharedString::default()
    } else {
        format!("Unknown tokens in the naming format: {}", unknown.join(", ")).into()
    });
    if set_text {
        b.set_output_name(doc.output_name.clone().into());
    }
    b.set_output_auto(!doc.output_overridden);
}

fn sync_job(b: &Bridge, doc: &Document) {
    let (busy, progress, status) = match &doc.job {
        JobState::Idle => (false, 0, String::new()),
        JobState::Queued => (true, 0, "Queued…".to_string()),
        JobState::Running { percent, .. } => (true, *percent as i32, format!("Saving… {percent}%")),
        JobState::Done { output } => (false, 100, format!("Saved to {output}")),
        JobState::Failed { message } => (false, 0, format!("Save failed: {message}")),
    };
    b.set_busy(busy);
    b.set_progress(progress);
    b.set_status_message(status.into());
}

/// After any edit: a finished/failed job no longer describes the document.
fn touch(doc: &mut Document) {
    if matches!(doc.job, JobState::Done { .. } | JobState::Failed { .. }) {
        doc.job = JobState::Idle;
    }
}

// ---------------------------------------------------------------------------
// Navigation, opening and closing files
// ---------------------------------------------------------------------------

fn load_config(ctx: &Ctx) {
    let ctx2 = ctx.clone();
    ctx.backend.load_config(Box::new(move |result| {
        ctx2.with(|b, st| {
            match result {
                Ok(config) => {
                    st.settings.draft = config.clone();
                    st.settings.profile_index = 0;
                    st.settings.dirty = false;
                    for doc in &mut st.docs {
                        doc.refresh_output_name(&config);
                    }
                    st.config = config;
                }
                Err(e) => st.error = format!("Could not load settings (using defaults): {}", e.message),
            }
            sync_all(b, st);
        });
    }));
}

fn show_error(ctx: &Ctx, message: String) {
    ctx.with(|b, st| {
        st.error = message;
        sync_nav(b, st);
    });
}

fn open_source(ctx: &Ctx, source: SourceRef) {
    let already_open = ctx.with(|b, st| {
        if let Some(i) = st.docs.iter().position(|d| d.source == source) {
            st.active = Some(i);
            st.show_browser = false;
            st.show_settings = false;
            sync_all(b, st);
            return true;
        }
        st.loading += 1;
        sync_nav(b, st);
        false
    });
    if already_open != Some(false) {
        return;
    }

    let ctx2 = ctx.clone();
    let source2 = source.clone();
    ctx.backend.analyze(source, Box::new(move |result| {
        ctx2.with(|b, st| {
            st.loading = st.loading.saturating_sub(1);
            match result {
                Ok(analysis) => {
                    let id = st.next_id;
                    st.next_id += 1;
                    let doc = Document::open(id, source2, analysis, &st.config);
                    st.docs.push(doc);
                    st.active = Some(st.docs.len() - 1);
                    st.show_browser = false;
                    st.show_settings = false;
                }
                Err(e) => st.error = format!("Could not open {}: {}", source2.file_name(), e.message),
            }
            sync_all(b, st);
        });
    }));
}

fn wire_navigation(window: &AppWindow, ctx: &Ctx) {
    let b = window.global::<Bridge>();

    let c = ctx.clone();
    b.on_select_tab(move |i| {
        c.with(|b, st| {
            if (i as usize) < st.docs.len() {
                st.active = Some(i as usize);
                st.show_settings = false;
                st.show_browser = false;
                sync_all(b, st);
            }
        });
    });

    let c = ctx.clone();
    b.on_close_tab(move |i| {
        c.with(|b, st| {
            let i = i as usize;
            if st.docs.get(i).is_none_or(|d| d.job.is_busy()) {
                return;
            }
            st.docs.remove(i);
            st.active = match st.active {
                _ if st.docs.is_empty() => None,
                Some(a) if a > i => Some(a - 1),
                Some(a) => Some(a.min(st.docs.len() - 1)),
                None => None,
            };
            if st.docs.is_empty() && !st.native {
                st.show_browser = true;
            }
            sync_all(b, st);
        });
    });

    let c = ctx.clone();
    b.on_open_requested(move || {
        if c.backend.capabilities().native_file_dialog {
            let c2 = c.clone();
            c.backend.pick_open_paths(Box::new(move |result| match result {
                Ok(sources) => {
                    for source in sources {
                        open_source(&c2, source);
                    }
                }
                Err(e) => show_error(&c2, e.message),
            }));
        } else {
            let needs_listing = c.with(|b, st| {
                st.show_browser = true;
                st.show_settings = false;
                sync_all(b, st);
                !st.browse_root.is_empty()
            });
            if needs_listing == Some(true) {
                list_dir(&c);
            }
        }
    });

    let c = ctx.clone();
    b.on_toggle_settings(move || {
        c.with(|b, st| {
            st.show_settings = !st.show_settings;
            if st.show_settings && !st.settings.dirty {
                st.settings.draft = st.config.clone();
                st.settings.profile_index = 0;
            }
            sync_all(b, st);
        });
    });

    let c = ctx.clone();
    b.on_dismiss_error(move || {
        c.with(|b, st| {
            st.error.clear();
            sync_nav(b, st);
        });
    });
}

// ---------------------------------------------------------------------------
// Volume browser (web)
// ---------------------------------------------------------------------------

fn load_roots(ctx: &Ctx) {
    let c = ctx.clone();
    ctx.backend.list_roots(Box::new(move |result| {
        let first = c.with(|b, st| match result {
            Ok(roots) => {
                b.set_volume_roots(strings(roots.iter().map(|r| r.id.clone())));
                let first = roots.first().map(|r| r.id.clone());
                if let Some(first) = &first {
                    st.browse_root = first.clone();
                    st.browse_path.clear();
                    b.set_browse_root(first.clone().into());
                }
                first
            }
            Err(e) => {
                st.error = format!("Could not list libraries: {}", e.message);
                sync_nav(b, st);
                None
            }
        });
        if first.flatten().is_some() {
            list_dir(&c);
        }
    }));
}

fn list_dir(ctx: &Ctx) {
    let Some((root, path)) = ctx.with(|_, st| (st.browse_root.clone(), st.browse_path.clone())) else {
        return;
    };
    if root.is_empty() {
        return;
    }
    let c = ctx.clone();
    ctx.backend.list_dir(root.clone(), path.clone(), Box::new(move |result| {
        c.with(|b, st| match result {
            Ok(listing) => {
                let rows: Vec<DirRow> = listing
                    .entries
                    .iter()
                    .filter(|e| e.is_dir || e.name.to_ascii_lowercase().ends_with(".mkv"))
                    .map(|e| DirRow { name: e.name.clone().into(), path: e.path.clone().into(), is_dir: e.is_dir })
                    .collect();
                b.set_browse_entries(ModelRc::new(VecModel::from(rows)));
                b.set_browse_root(root.into());
                b.set_browse_path(path.into());
            }
            Err(e) => {
                st.error = format!("Could not list folder: {}", e.message);
                sync_nav(b, st);
            }
        });
    }));
}

fn wire_browser(window: &AppWindow, ctx: &Ctx) {
    let b = window.global::<Bridge>();

    let c = ctx.clone();
    b.on_browse_root_selected(move |root| {
        c.with(|_, st| {
            st.browse_root = root.to_string();
            st.browse_path.clear();
        });
        list_dir(&c);
    });

    let c = ctx.clone();
    b.on_browse_entry_activated(move |path, is_dir| {
        if is_dir {
            c.with(|_, st| st.browse_path = path.to_string());
            list_dir(&c);
        } else if let Some(root_id) = c.with(|_, st| st.browse_root.clone()) {
            open_source(&c, SourceRef::ServerPath { root_id, path: path.to_string() });
        }
    });

    let c = ctx.clone();
    b.on_browse_up(move || {
        c.with(|_, st| match st.browse_path.rfind('/') {
            Some(pos) => st.browse_path.truncate(pos),
            None => st.browse_path.clear(),
        });
        list_dir(&c);
    });

    let c = ctx.clone();
    b.on_browse_close(move || {
        c.with(|b, st| {
            if !st.docs.is_empty() {
                st.show_browser = false;
                sync_all(b, st);
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Active document: profile, language, media info
// ---------------------------------------------------------------------------

/// Mutates the active document and re-syncs the parts that depend on it.
fn edit_doc(ctx: &Ctx, rebuild_tracks: bool, f: impl FnOnce(&mut Document, &AppConfig)) {
    ctx.with(|b, st| {
        let State { docs, config, active, .. } = &mut *st;
        let Some(doc) = active.and_then(|i| docs.get_mut(i)) else { return };
        f(doc, config);
        touch(doc);
        let doc = doc.clone();
        sync_tracks(b, &doc, rebuild_tracks);
        sync_warnings(b, &doc);
        sync_output(b, &doc, &st.config, true);
        sync_job(b, &doc);
        sync_nav(b, st);
        sync_settings_status(b, st);
    });
}

fn wire_document(window: &AppWindow, ctx: &Ctx) {
    let b = window.global::<Bridge>();

    let c = ctx.clone();
    b.on_profile_selected(move |i| {
        let name = c.with(|_, st| st.config.profiles.get(i as usize).map(|p| p.name.clone())).flatten();
        if let Some(name) = name {
            edit_doc(&c, true, |doc, config| {
                doc.profile = name;
                doc.apply_profile(config);
            });
        }
    });

    let c = ctx.clone();
    b.on_language_selected(move |i| {
        let code = c.with(|_, st| st.language_codes.get(i as usize).cloned()).flatten();
        if let Some(code) = code {
            edit_doc(&c, true, |doc, config| {
                doc.original_language = (!code.is_empty()).then_some(code);
                doc.apply_profile(config);
            });
            // The "Not set" entry disappears once a language is picked.
            c.with(sync_doc);
        }
    });

    let c = ctx.clone();
    b.on_apply_profile(move || edit_doc(&c, true, |doc, config| doc.apply_profile(config)));

    let c = ctx.clone();
    b.on_reset_tracks(move || edit_doc(&c, true, |doc, config| doc.reset(config)));

    let c = ctx.clone();
    b.on_apply_all(move || {
        c.with(|b, st| {
            let State { docs, config, .. } = &mut *st;
            for doc in docs.iter_mut().filter(|d| !d.job.is_busy()) {
                doc.apply_profile(config);
                touch(doc);
            }
            sync_all(b, st);
        });
    });

    let c = ctx.clone();
    b.on_info_section_selected(move |i| {
        c.with(|b, st| {
            if let Some(doc) = st.active.and_then(|a| st.docs.get_mut(a)) {
                doc.info_section = i.max(0) as usize;
                doc.info_filter.clear();
                let doc = doc.clone();
                b.set_info_filter(SharedString::default());
                sync_info(b, &doc);
            }
        });
    });

    let c = ctx.clone();
    b.on_info_filter_edited(move |text| {
        c.with(|b, st| {
            if let Some(doc) = st.active.and_then(|a| st.docs.get_mut(a)) {
                doc.info_filter = text.to_string();
                let doc = doc.clone();
                sync_info(b, &doc);
            }
        });
    });
}

fn wire_tracks(window: &AppWindow, ctx: &Ctx) {
    let b = window.global::<Bridge>();

    let c = ctx.clone();
    b.on_move_track(move |from, to| {
        edit_doc(&c, true, |doc, config| doc.move_row(from as usize, to as usize, config));
    });

    let c = ctx.clone();
    b.on_set_keep(move |row, keep| {
        edit_doc(&c, false, |doc, config| doc.set_keep(row as usize, keep, config));
    });

    // Text edits: update state and the dependent bits (tab dot, output
    // name) but never rebuild the row being typed into.
    let c = ctx.clone();
    b.on_rename_track(move |row, text| {
        c.with_doc(|doc, _| {
            doc.set_name(row as usize, &text);
            touch(doc);
        });
        c.with(|b, st| sync_nav(b, st));
    });

    let c = ctx.clone();
    b.on_set_language(move |row, text| {
        c.with_doc(|doc, config| {
            doc.set_language(row as usize, &text, config);
            touch(doc);
        });
        c.with(|b, st| {
            if let Some(doc) = st.active_doc().cloned() {
                sync_output(b, &doc, &st.config, true);
            }
            sync_nav(b, st);
        });
    });

    let c = ctx.clone();
    b.on_set_flag(move |row, flag, value| {
        let flag = match flag.as_str() {
            "default" => Flag::Default,
            "forced" => Flag::Forced,
            "original" => Flag::Original,
            "hi" => Flag::HearingImpaired,
            "vi" => Flag::VisualImpaired,
            "commentary" => Flag::Commentary,
            _ => return,
        };
        edit_doc(&c, false, |doc, _| doc.set_flag(row as usize, flag, value));
    });
}

fn wire_naming(window: &AppWindow, ctx: &Ctx) {
    let b = window.global::<Bridge>();

    let c = ctx.clone();
    b.on_naming_edited(move || {
        c.with(|b, st| {
            let State { docs, config, active, .. } = &mut *st;
            let Some(doc) = active.and_then(|i| docs.get_mut(i)) else { return };
            read_naming_form(b, doc);
            doc.refresh_output_name(config);
            touch(doc);
            let doc = doc.clone();
            sync_output(b, &doc, &st.config, true);
            sync_nav(b, st);
            sync_settings_status(b, st);
        });
    });

    let c = ctx.clone();
    b.on_output_edited(move |text| {
        c.with(|b, st| {
            let State { docs, config, active, .. } = &mut *st;
            let Some(doc) = active.and_then(|i| docs.get_mut(i)) else { return };
            doc.output_name = text.to_string();
            doc.output_overridden = true;
            let doc = doc.clone();
            sync_output(b, &doc, config, false);
        });
    });

    let c = ctx.clone();
    b.on_output_reset(move || {
        c.with(|b, st| {
            let State { docs, config, active, .. } = &mut *st;
            let Some(doc) = active.and_then(|i| docs.get_mut(i)) else { return };
            doc.output_overridden = false;
            doc.refresh_output_name(config);
            let doc = doc.clone();
            sync_output(b, &doc, config, true);
        });
    });
}

// ---------------------------------------------------------------------------
// Saving (single + "Save all" queue)
// ---------------------------------------------------------------------------

/// Updates one document (by id) and re-syncs the tab strip, plus the job
/// panel if it's the active one.
fn update_doc(ctx: &Ctx, id: u64, f: impl FnOnce(&mut Document)) {
    ctx.with(|b, st| {
        let Some(i) = st.doc_index(id) else { return };
        f(&mut st.docs[i]);
        if st.active == Some(i) {
            let doc = st.docs[i].clone();
            sync_job(b, &doc);
        }
        sync_nav(b, st);
    });
}

fn start_job(ctx: &Ctx, id: u64, output: SourceRef) {
    let job_input = ctx.with(|b, st| {
        let i = st.doc_index(id)?;
        let doc = &mut st.docs[i];
        doc.job = JobState::Running { job: None, percent: 0 };
        let kept = doc.kept();
        let input = (doc.source.clone(), kept.iter().map(|e| e.id).collect::<Vec<_>>(), kept);
        if st.active == Some(i) {
            let doc = doc.clone();
            sync_job(b, &doc);
        }
        sync_nav(b, st);
        Some(input)
    });
    let Some(Some((source, order, edits))) = job_input else { return };

    let c = ctx.clone();
    let output_label = match &output {
        SourceRef::LocalPath(p) => p.clone(),
        SourceRef::ServerPath { path, .. } => format!("output:/{path}"),
    };
    ctx.backend.submit_job(source, output, order, edits, Box::new(move |result| match result {
        Ok(job_id) => {
            update_doc(&c, id, |doc| doc.job = JobState::Running { job: Some(job_id.clone()), percent: 0 });

            let c_progress = c.clone();
            let on_progress: ProgressCallback = Box::new(move |p: JobProgress| {
                update_doc(&c_progress, id, |doc| {
                    if let JobState::Running { percent, .. } = &mut doc.job {
                        *percent = p.percent;
                    }
                });
            });
            let c_done = c.clone();
            let on_done: UiCallback<JobOutcome> = Box::new(move |result| {
                update_doc(&c_done, id, |doc| {
                    doc.job = match result {
                        Ok(_) => JobState::Done { output: output_label },
                        Err(e) => JobState::Failed { message: e.message },
                    };
                });
                run_next_in_queue(&c_done);
            });
            c.backend.watch_job(job_id, on_progress, on_done);
        }
        Err(e) => {
            update_doc(&c, id, |doc| doc.job = JobState::Failed { message: e.message });
            run_next_in_queue(&c);
        }
    }));
}

fn run_next_in_queue(ctx: &Ctx) {
    let next = ctx.with(|_, st| {
        // Only one queued job at a time; skip if one is still running.
        let running = st.docs.iter().any(|d| matches!(d.job, JobState::Running { .. }));
        if running {
            None
        } else {
            st.save_queue.pop_front()
        }
    });
    if let Some(Some((id, output))) = next {
        start_job(ctx, id, output);
    }
}

fn wire_jobs(window: &AppWindow, ctx: &Ctx) {
    let b = window.global::<Bridge>();

    let c = ctx.clone();
    b.on_save(move || {
        let prepared = c.with(|b, st| {
            let doc = st.active_doc()?.clone();
            if let Err(message) = doc.validate_for_save() {
                st.error = message;
                sync_nav(b, st);
                return None;
            }
            Some(doc)
        });
        let Some(Some(doc)) = prepared else { return };

        if c.backend.capabilities().native_file_dialog {
            let c2 = c.clone();
            let id = doc.id;
            c.backend.pick_save_path(&doc.source, doc.output_name.clone(), Box::new(move |result| match result {
                Ok(Some(output)) => start_job(&c2, id, output),
                Ok(None) => {}
                Err(e) => show_error(&c2, e.message),
            }));
        } else {
            let output = c.backend.output_in(None, &doc.output_name);
            start_job(&c, doc.id, output);
        }
    });

    let c = ctx.clone();
    b.on_save_all(move || {
        let enqueue = move |c: &Ctx, dir: Option<String>| {
            c.with(|b, st| {
                let mut problems = Vec::new();
                for doc in &mut st.docs {
                    if doc.job.is_busy() {
                        continue;
                    }
                    match doc.validate_for_save() {
                        Ok(()) => {
                            let output = c.backend.output_in(dir.as_deref(), &doc.output_name);
                            doc.job = JobState::Queued;
                            st.save_queue.push_back((doc.id, output));
                        }
                        Err(e) => problems.push(format!("{}: {e}", doc.title())),
                    }
                }
                if !problems.is_empty() {
                    st.error = format!("Skipped: {}", problems.join("; "));
                }
                sync_all(b, st);
            });
            run_next_in_queue(c);
        };

        if c.backend.capabilities().native_file_dialog {
            let c2 = c.clone();
            c.backend.pick_output_dir(Box::new(move |result| match result {
                Ok(Some(dir)) => enqueue(&c2, Some(dir)),
                Ok(None) => {}
                Err(e) => show_error(&c2, e.message),
            }));
        } else {
            enqueue(&c, None);
        }
    });

    let c = ctx.clone();
    b.on_cancel(move || {
        let job = c.with(|b, st| {
            let i = st.active?;
            let id = st.docs[i].id;
            match &st.docs[i].job {
                JobState::Running { job, .. } => job.clone(),
                JobState::Queued => {
                    st.save_queue.retain(|(qid, _)| *qid != id);
                    st.docs[i].job = JobState::Idle;
                    let doc = st.docs[i].clone();
                    sync_job(b, &doc);
                    sync_nav(b, st);
                    None
                }
                _ => None,
            }
        });
        if let Some(Some(job)) = job {
            c.backend.cancel_job(job);
        }
    });
}

// ---------------------------------------------------------------------------
// Settings tab
// ---------------------------------------------------------------------------

fn sync_settings_form(b: &Bridge, st: &State) {
    let s = &st.settings;
    b.set_s_profile_names(strings(s.draft.profiles.iter().map(|p| p.name.clone())));
    b.set_s_profile_index(s.profile_index as i32);
    if let Some(profile) = s.draft.profiles.get(s.profile_index) {
        let f = settings::profile_to_form(profile);
        b.set_s_name(f.name.into());
        b.set_s_video_format(f.video_format.into());
        b.set_s_audio_format(f.audio_format.into());
        b.set_s_audio_rules(f.audio_rules.into());
        b.set_s_keep_commentary(f.keep_commentary);
        b.set_s_keep_descriptions(f.keep_descriptions);
        b.set_s_sub_languages(f.subtitle_languages.into());
        b.set_s_sub_format(f.subtitle_format.into());
        b.set_s_sub_order(f.subtitle_order.into());
        b.set_s_default_forced(f.default_forced_when_foreign);
        b.set_s_detect_forced(f.detect_forced_by_element_count);
    }
    let n = settings::naming_to_form(&s.draft.naming);
    b.set_s_movie_format(n.movie_format.into());
    b.set_s_episode_format(n.episode_format.into());
    b.set_s_colon_index(n.colon_index as i32);
    b.set_s_custom_formats(n.custom_formats.into());
}

fn sync_settings_status(b: &Bridge, st: &State) {
    let s = &st.settings;
    b.set_s_errors(s.errors.clone().into());
    b.set_s_dirty(s.dirty);
    let preview = st
        .active_doc()
        .map(|doc| settings::preview(doc, &s.draft, s.profile_index))
        .unwrap_or_default();
    b.set_s_preview(preview.into());
}

/// Reads the Settings form into the draft config. Parse errors keep the
/// previous draft values and are reported instead.
fn read_settings_form(b: &Bridge, st: &mut State) {
    let s = &mut st.settings;
    let mut problems = Vec::new();

    let form = ProfileForm {
        name: b.get_s_name().to_string(),
        video_format: b.get_s_video_format().to_string(),
        audio_format: b.get_s_audio_format().to_string(),
        audio_rules: b.get_s_audio_rules().to_string(),
        keep_commentary: b.get_s_keep_commentary(),
        keep_descriptions: b.get_s_keep_descriptions(),
        subtitle_languages: b.get_s_sub_languages().to_string(),
        subtitle_format: b.get_s_sub_format().to_string(),
        subtitle_order: b.get_s_sub_order().to_string(),
        default_forced_when_foreign: b.get_s_default_forced(),
        detect_forced_by_element_count: b.get_s_detect_forced(),
    };
    if let Some(base) = s.draft.profiles.get(s.profile_index).cloned() {
        match settings::form_to_profile(&form, &base) {
            Ok(p) => s.draft.profiles[s.profile_index] = p,
            Err(e) => problems.push(e),
        }
    }
    let naming = NamingForm {
        movie_format: b.get_s_movie_format().to_string(),
        episode_format: b.get_s_episode_format().to_string(),
        colon_index: b.get_s_colon_index().max(0) as usize,
        custom_formats: b.get_s_custom_formats().to_string(),
    };
    match settings::form_to_naming(&naming) {
        Ok(n) => s.draft.naming = n,
        Err(e) => problems.push(e),
    }
    if problems.is_empty() {
        if let Err(e) = s.draft.validate() {
            problems.push(e.to_string());
        }
    }
    s.errors = problems.join("\n");
}

fn wire_settings(window: &AppWindow, ctx: &Ctx) {
    let b = window.global::<Bridge>();

    let c = ctx.clone();
    b.on_s_edited(move || {
        c.with(|b, st| {
            read_settings_form(b, st);
            st.settings.dirty = true;
            // The profile name may have changed.
            b.set_s_profile_names(strings(st.settings.draft.profiles.iter().map(|p| p.name.clone())));
            b.set_s_profile_index(st.settings.profile_index as i32);
            sync_settings_status(b, st);
        });
    });

    let c = ctx.clone();
    b.on_s_profile_selected(move |i| {
        c.with(|b, st| {
            read_settings_form(b, st);
            st.settings.profile_index = (i.max(0) as usize).min(st.settings.draft.profiles.len().saturating_sub(1));
            sync_settings_form(b, st);
            sync_settings_status(b, st);
        });
    });

    let c = ctx.clone();
    b.on_s_add_profile(move || {
        c.with(|b, st| {
            read_settings_form(b, st);
            let s = &mut st.settings;
            if let Some(base) = s.draft.profiles.get(s.profile_index).cloned() {
                let mut copy = base.clone();
                let mut n = 2;
                copy.name = format!("{} copy", base.name);
                while s.draft.profiles.iter().any(|p| p.name == copy.name) {
                    copy.name = format!("{} copy {n}", base.name);
                    n += 1;
                }
                s.draft.profiles.push(copy);
                s.profile_index = s.draft.profiles.len() - 1;
                s.dirty = true;
            }
            sync_settings_form(b, st);
            sync_settings_status(b, st);
        });
    });

    let c = ctx.clone();
    b.on_s_delete_profile(move || {
        c.with(|b, st| {
            let s = &mut st.settings;
            if s.draft.profiles.len() > 1 {
                s.draft.profiles.remove(s.profile_index);
                s.profile_index = 0;
                s.dirty = true;
                s.errors = s.draft.validate().err().map(|e| e.to_string()).unwrap_or_default();
            }
            sync_settings_form(b, st);
            sync_settings_status(b, st);
        });
    });

    let c = ctx.clone();
    b.on_s_revert(move || {
        c.with(|b, st| {
            st.settings.draft = st.config.clone();
            st.settings.profile_index = 0;
            st.settings.dirty = false;
            st.settings.errors.clear();
            sync_settings_form(b, st);
            sync_settings_status(b, st);
        });
    });

    let c = ctx.clone();
    b.on_s_reset_defaults(move || {
        c.with(|b, st| {
            st.settings.draft = AppConfig::default();
            st.settings.profile_index = 0;
            st.settings.dirty = true;
            st.settings.errors.clear();
            sync_settings_form(b, st);
            sync_settings_status(b, st);
        });
    });

    let c = ctx.clone();
    b.on_s_save(move || {
        let draft = c.with(|b, st| {
            read_settings_form(b, st);
            sync_settings_status(b, st);
            st.settings.errors.is_empty().then(|| st.settings.draft.clone())
        });
        let Some(Some(draft)) = draft else { return };

        let c2 = c.clone();
        let saved = draft.clone();
        c.backend.save_config(draft, Box::new(move |result| {
            c2.with(|b, st| {
                match result {
                    Ok(()) => {
                        st.config = saved;
                        st.settings.dirty = false;
                        let State { docs, config, .. } = &mut *st;
                        for doc in docs.iter_mut() {
                            if config.profile(&doc.profile).is_none() {
                                doc.profile = config.profiles.first().map(|p| p.name.clone()).unwrap_or_default();
                            }
                            doc.refresh_output_name(config);
                        }
                    }
                    Err(e) => st.settings.errors = format!("Could not save settings: {}", e.message),
                }
                sync_all(b, st);
            });
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_text_round_trips() {
        assert_eq!(episodes_from_text("2"), [2]);
        assert_eq!(episodes_from_text("2-4"), [2, 3, 4]);
        assert_eq!(episodes_from_text("2-E3"), [2, 3]);
        assert_eq!(episodes_from_text("1, 5"), [1, 5]);
        assert_eq!(episodes_from_text("x"), Vec::<u32>::new());
        assert_eq!(episodes_to_text(&[2, 3, 4]), "2-4");
        assert_eq!(episodes_to_text(&[1, 5]), "1,5");
    }
}
