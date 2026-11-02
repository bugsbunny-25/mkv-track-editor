mod error;
mod handlers;
mod jobs;

use axum::routing::{delete, get, post};
use axum::Router;
use jobs::JobRegistry;
use mkv_core::{config_store, TokioProcessRunner, ToolPaths};
use mkv_volume::VolumeManager;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct AppState {
    volumes: Arc<VolumeManager>,
    runner: Arc<TokioProcessRunner>,
    jobs: Arc<JobRegistry>,
    config_path: Arc<PathBuf>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let tools = ToolPaths::from_env();
    match mkv_core::check_dependencies(&tools).await {
        Ok(warnings) => {
            for warning in warnings {
                tracing::warn!("{warning}");
            }
        }
        Err(e) => {
            eprintln!(
                "error: {e}\n\nmkvmerge and mkvpropedit (MKVToolNix) and mediainfo (MediaInfo CLI) must be \
                 installed in the container/host and on PATH."
            );
            std::process::exit(1);
        }
    }

    let volumes = match VolumeManager::from_env() {
        Ok(v) => Arc::new(v),
        Err(e) => {
            eprintln!(
                "error: {e}\n\nSet MKV_LIBRARY_ROOTS, e.g. MKV_LIBRARY_ROOTS=movies=/data/library \
                 (optionally MKV_OUTPUT_ROOT=/data/output)."
            );
            std::process::exit(1);
        }
    };

    let config_dir = std::env::var("MKV_CONFIG_DIR").unwrap_or_else(|_| "config".to_string());
    let config_path = config_store::config_file(&PathBuf::from(config_dir));
    // Load (or create) once at startup so a broken config file is reported
    // immediately rather than on first use.
    if let Err(e) = config_store::load_or_init(&config_path).await {
        eprintln!("error: {e}\n\nFix or remove the config file to restore defaults.");
        std::process::exit(1);
    }
    tracing::info!(config = %config_path.display(), "using settings file");

    let state = AppState {
        volumes,
        runner: Arc::new(TokioProcessRunner::new(tools)),
        jobs: JobRegistry::new(),
        config_path: Arc::new(config_path),
    };

    let static_dir = std::env::var("MKV_STATIC_DIR").unwrap_or_else(|_| "static".to_string());

    let app = Router::new()
        .route("/api/volumes", get(handlers::list_volumes))
        .route("/api/browse/{root_id}", get(handlers::browse))
        .route("/api/files/{root_id}", delete(handlers::delete_file))
        .route("/api/analyze", post(handlers::analyze))
        .route("/api/config", get(handlers::get_config).put(handlers::put_config))
        .route("/api/jobs", post(handlers::submit_job))
        .route("/api/mux", post(handlers::submit_mux))
        .route("/api/jobs/{id}/events", get(handlers::job_events))
        .route("/api/jobs/{id}/cancel", post(handlers::cancel_job))
        .fallback_service(ServeDir::new(static_dir).append_index_html_on_directories(true))
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|e| panic!("failed to bind port {port}: {e}"));

    tracing::info!(%port, "mkv-server listening");
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "server exited with an error");
        std::process::exit(1);
    }
}
