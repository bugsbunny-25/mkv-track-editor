use crate::error::AppError;
use crate::jobs::JobEntry;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Json;
use futures_util::stream::{self, Stream, StreamExt};
use mkv_core::config_store;
use mkv_profiles::AppConfig;
use mkv_types::{
    ApiError, DirListing, FileAnalysis, JobId, JobPhase, JobProgress, ProbeResult, RemuxPlan, RootInfo, TrackEdit,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;

pub async fn list_volumes(State(state): State<AppState>) -> Json<Vec<RootInfo>> {
    Json(state.volumes.list_roots())
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    path: String,
}

pub async fn browse(
    State(state): State<AppState>,
    Path(root_id): Path<String>,
    Query(q): Query<BrowseQuery>,
) -> Result<Json<DirListing>, AppError> {
    let volumes = state.volumes.clone();
    let root_id_for_task = root_id.clone();
    let path_for_task = q.path.clone();
    let entries = spawn_blocking_flatten(move || volumes.list_dir(&root_id_for_task, &path_for_task)).await?;
    Ok(Json(DirListing {
        root_id,
        path: q.path,
        entries,
    }))
}

#[derive(Deserialize)]
pub struct AnalyzeRequest {
    root_id: String,
    path: String,
}

/// mkvmerge probe + MediaInfo analysis. MediaInfo's absolute "Complete name"
/// is replaced with the bare file name by `mkv_core::mediainfo`, so no
/// container filesystem layout reaches the client.
pub async fn analyze(
    State(state): State<AppState>,
    Json(req): Json<AnalyzeRequest>,
) -> Result<Json<FileAnalysis>, AppError> {
    let abs_path = resolve_source(&state, &req.root_id, &req.path).await?;
    let result = mkv_core::analyze(state.runner.as_ref(), &abs_path).await?;
    Ok(Json(result))
}

pub async fn get_config(State(state): State<AppState>) -> Result<Json<AppConfig>, AppError> {
    let config = config_store::load_or_init(&state.config_path).await?;
    Ok(Json(config))
}

/// Validates before writing (see `config_store::save`), so a bad template or
/// regex is rejected with a 400 instead of corrupting the stored config.
pub async fn put_config(State(state): State<AppState>, Json(config): Json<AppConfig>) -> Result<(), AppError> {
    if let Err(e) = config.validate() {
        return Err(AppError::bad_request(e.to_string()));
    }
    config_store::save(&state.config_path, &config).await?;
    tracing::info!(path = %state.config_path.display(), "settings saved");
    Ok(())
}

#[derive(Deserialize)]
pub struct SubmitJobRequest {
    root_id: String,
    source_path: String,
    output_path: String,
    kept_track_order: Vec<u64>,
    edits: Vec<TrackEdit>,
}

#[derive(Serialize)]
pub struct SubmitJobResponse {
    job_id: JobId,
}

pub async fn submit_job(
    State(state): State<AppState>,
    Json(req): Json<SubmitJobRequest>,
) -> Result<Json<SubmitJobResponse>, AppError> {
    let source_path = resolve_source(&state, &req.root_id, &req.source_path).await?;

    let volumes = state.volumes.clone();
    let output_rel = req.output_path.clone();
    let output_path = spawn_blocking_flatten(move || volumes.resolve_output(&output_rel)).await?;

    let plan = RemuxPlan {
        source_path: source_path.to_string_lossy().into_owned(),
        output_path: output_path.to_string_lossy().into_owned(),
        kept_track_order: req.kept_track_order,
        edits: req.edits,
    };
    mkv_core::plan::validate(&plan)?;

    let original = mkv_core::probe(state.runner.as_ref(), StdPath::new(&plan.source_path)).await?;

    let job_id = JobId::new();
    let entry = state.jobs.create(job_id.clone());

    let runner = state.runner.clone();
    let entry_for_task = entry.clone();
    tokio::spawn(run_job(runner, original, plan, entry_for_task));

    Ok(Json(SubmitJobResponse { job_id }))
}

async fn run_job(
    runner: Arc<mkv_core::TokioProcessRunner>,
    original: ProbeResult,
    plan: RemuxPlan,
    entry: Arc<JobEntry>,
) {
    entry.publish(JobProgress {
        phase: JobPhase::Running,
        percent: 0,
        message: None,
        output_path: None,
    });

    let sink: Box<dyn Fn(JobProgress) + Send + Sync> = {
        let entry = entry.clone();
        Box::new(move |p: JobProgress| entry.publish(p))
    };

    let outcome = tokio::select! {
        r = mkv_core::execute_plan(runner.as_ref(), &original, &plan, sink) => {
            r.map_err(|e| ApiError::new("remux_failed", e.to_string()))
        }
        _ = entry.cancel.cancelled() => Err(ApiError::new("cancelled", "job was cancelled")),
    };

    entry.finish(&outcome);
}

enum SseState {
    Initial(JobProgress, tokio::sync::broadcast::Receiver<JobProgress>),
    Live(tokio::sync::broadcast::Receiver<JobProgress>),
    Done,
}

fn is_terminal(p: &JobProgress) -> bool {
    matches!(p.phase, JobPhase::Succeeded | JobPhase::Failed | JobPhase::Cancelled)
}

pub async fn job_events(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let entry = state
        .jobs
        .get(&JobId(id))
        .ok_or_else(|| AppError::not_found("unknown job"))?;
    let (initial, rx) = entry.subscribe();

    // A single state machine handles both the initial snapshot (so a client
    // that subscribes after the job already progressed catches up
    // immediately) and the live broadcast feed, and stops the stream right
    // after the terminal (succeeded/failed/cancelled) event is emitted.
    let event_stream = stream::unfold(SseState::Initial(initial, rx), |state| async move {
        match state {
            SseState::Initial(p, rx) => {
                let next = if is_terminal(&p) { SseState::Done } else { SseState::Live(rx) };
                Some((p, next))
            }
            SseState::Live(mut rx) => loop {
                match rx.recv().await {
                    Ok(p) => {
                        let next = if is_terminal(&p) { SseState::Done } else { SseState::Live(rx) };
                        return Some((p, next));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            },
            SseState::Done => None,
        }
    })
    .map(|p| Ok(Event::default().json_data(&p).expect("JobProgress always serializes")));

    Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
}

pub async fn cancel_job(State(state): State<AppState>, Path(id): Path<String>) -> Result<(), AppError> {
    let entry = state
        .jobs
        .get(&JobId(id))
        .ok_or_else(|| AppError::not_found("unknown job"))?;
    entry.cancel.cancel();
    Ok(())
}

async fn resolve_source(state: &AppState, root_id: &str, path: &str) -> Result<PathBuf, AppError> {
    let volumes = state.volumes.clone();
    let root_id = root_id.to_string();
    let path = path.to_string();
    spawn_blocking_flatten(move || volumes.resolve(&root_id, &path)).await
}

async fn spawn_blocking_flatten<T, E, F>(f: F) -> Result<T, AppError>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
    AppError: From<E>,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| AppError::internal("background task panicked"))?
        .map_err(AppError::from)
}
