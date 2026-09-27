//! [`mkv_ui::Backend`] implementation for the wasm web build: no native
//! dialogs, no process execution — everything goes over HTTP/SSE to the
//! Rust server, which does all real file I/O and `mkvmerge`/`mkvpropedit`
//! execution.

use gloo_net::http::{Request, Response};
use mkv_profiles::AppConfig;
use mkv_types::{
    AttachmentInfo, DirListing, FileAnalysis, JobId, JobOutcome, JobPhase, JobProgress, RootInfo, TrackEdit,
};
use mkv_ui::{Backend, BackendCapabilities, ProgressCallback, SourceRef, UiCallback, UiError};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Zero-sized: all state lives in the browser (the server) or in short-lived
/// per-call closures, so `WebBackend` trivially satisfies the `Backend:
/// Send + Sync` supertrait bound (required for the desktop build's real
/// cross-thread use) even though wasm32 is single-threaded and none of this
/// ever actually crosses a thread.
pub struct WebBackend;

fn to_ui_error(e: gloo_net::Error) -> UiError {
    UiError::new(e.to_string())
}

/// The server's JSON `ApiError` message, or the HTTP status if the body
/// isn't one.
async fn error_from(response: Response) -> UiError {
    let status = response.status();
    let message = response
        .json::<mkv_types::ApiError>()
        .await
        .ok()
        .map(|e| e.message)
        .unwrap_or_else(|| format!("request failed with status {status}"));
    UiError::new(message)
}

async fn finish<T: DeserializeOwned>(response: Result<Response, gloo_net::Error>) -> Result<T, UiError> {
    let response = response.map_err(to_ui_error)?;
    if response.ok() {
        response.json::<T>().await.map_err(to_ui_error)
    } else {
        Err(error_from(response).await)
    }
}

async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T, UiError> {
    finish(Request::get(url).send().await).await
}

async fn get_json_with_query<T: DeserializeOwned>(url: &str, query: [(&str, &str); 1]) -> Result<T, UiError> {
    finish(Request::get(url).query(query).send().await).await
}

async fn post_json<B: Serialize, T: DeserializeOwned>(url: &str, body: &B) -> Result<T, UiError> {
    let builder = Request::post(url).json(body).map_err(to_ui_error)?;
    finish(builder.send().await).await
}

/// Runs `fut` on the wasm single-threaded executor and delivers its result
/// on the Slint UI thread via `invoke_from_event_loop`, per the `Backend`
/// contract (identical mechanism to the desktop build).
fn spawn_and_deliver<T, Fut>(cb: UiCallback<T>, fut: Fut)
where
    T: Send + 'static,
    Fut: Future<Output = Result<T, UiError>> + 'static,
{
    wasm_bindgen_futures::spawn_local(async move {
        let result = fut.await;
        let _ = slint::invoke_from_event_loop(move || cb(result));
    });
}

impl Backend for WebBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities { native_file_dialog: false }
    }

    fn list_roots(&self, cb: UiCallback<Vec<RootInfo>>) {
        spawn_and_deliver(cb, async { get_json("/api/volumes").await });
    }

    fn list_dir(&self, root_id: String, path: String, cb: UiCallback<DirListing>) {
        spawn_and_deliver(cb, async move {
            let url = format!("/api/browse/{root_id}");
            get_json_with_query(&url, [("path", path.as_str())]).await
        });
    }

    fn pick_open_paths(&self, cb: UiCallback<Vec<SourceRef>>) {
        cb(Err(UiError::new("native file dialogs are not available in the browser")));
    }

    fn analyze(&self, source: SourceRef, cb: UiCallback<FileAnalysis>) {
        let SourceRef::ServerPath { root_id, path } = source else {
            cb(Err(UiError::new("invalid source for web backend")));
            return;
        };
        #[derive(Serialize)]
        struct Req {
            root_id: String,
            path: String,
        }
        spawn_and_deliver(cb, async move { post_json("/api/analyze", &Req { root_id, path }).await });
    }

    fn pick_save_path(&self, _source: &SourceRef, _suggested_name: String, cb: UiCallback<Option<SourceRef>>) {
        cb(Err(UiError::new("native file dialogs are not available in the browser")));
    }

    fn pick_output_dir(&self, cb: UiCallback<Option<String>>) {
        cb(Err(UiError::new("native file dialogs are not available in the browser")));
    }

    /// The server always writes into its configured output root; `dir` is a
    /// desktop concept and is ignored.
    fn output_in(&self, _dir: Option<&str>, file_name: &str) -> SourceRef {
        SourceRef::ServerPath {
            root_id: "output".into(),
            path: file_name.into(),
        }
    }

    fn load_config(&self, cb: UiCallback<AppConfig>) {
        spawn_and_deliver(cb, async { get_json("/api/config").await });
    }

    fn save_config(&self, config: AppConfig, cb: UiCallback<()>) {
        spawn_and_deliver(cb, async move {
            let builder = Request::put("/api/config").json(&config).map_err(to_ui_error)?;
            let response = builder.send().await.map_err(to_ui_error)?;
            if response.ok() {
                Ok(())
            } else {
                Err(error_from(response).await)
            }
        });
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
        let (SourceRef::ServerPath { root_id, path: source_path }, SourceRef::ServerPath { path: output_path, .. }) =
            (source, output)
        else {
            cb(Err(UiError::new("invalid source/output for web backend")));
            return;
        };

        #[derive(Serialize)]
        struct Req {
            root_id: String,
            source_path: String,
            output_path: String,
            kept_track_order: Vec<u64>,
            edits: Vec<TrackEdit>,
            remove_attachments: Vec<AttachmentInfo>,
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            job_id: JobId,
        }

        spawn_and_deliver(cb, async move {
            let body = Req {
                root_id,
                source_path,
                output_path,
                kept_track_order,
                edits,
                remove_attachments,
            };
            let resp: Resp = post_json("/api/jobs", &body).await?;
            Ok(resp.job_id)
        });
    }

    fn watch_job(&self, job: JobId, on_progress: ProgressCallback, on_done: UiCallback<JobOutcome>) {
        let url = format!("/api/jobs/{}/events", job.0);
        let event_source = match web_sys::EventSource::new(&url) {
            Ok(es) => es,
            Err(_) => {
                let _ = slint::invoke_from_event_loop(move || {
                    on_done(Err(UiError::new("failed to open progress stream")))
                });
                return;
            }
        };

        // `on_done` is FnOnce but EventSource's message handler is FnMut
        // (called repeatedly); take it out exactly once, on the terminal
        // event. `on_progress` is `Fn`, but each delivery needs its own
        // owned closure to hand to `invoke_from_event_loop`, hence `Arc`.
        let on_done = Rc::new(RefCell::new(Some(on_done)));
        let on_progress = std::sync::Arc::new(on_progress);
        let es_for_close = event_source.clone();

        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            let Some(data) = event.data().as_string() else { return };
            let Ok(progress) = serde_json::from_str::<JobProgress>(&data) else { return };

            let terminal = matches!(
                progress.phase,
                JobPhase::Succeeded | JobPhase::Failed | JobPhase::Cancelled
            );

            {
                let progress = progress.clone();
                let on_progress = on_progress.clone();
                let _ = slint::invoke_from_event_loop(move || on_progress(progress));
            }

            if terminal {
                es_for_close.close();
                if let Some(done) = on_done.borrow_mut().take() {
                    let outcome = match progress.phase {
                        JobPhase::Succeeded => Ok(JobOutcome {
                            output_path: progress.output_path.unwrap_or_default(),
                        }),
                        _ => Err(UiError::new(
                            progress.message.unwrap_or_else(|| "job did not complete successfully".to_string()),
                        )),
                    };
                    let _ = slint::invoke_from_event_loop(move || done(outcome));
                }
            }
        });
        event_source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        // The closure must outlive the EventSource's callback registration;
        // it's only ever invoked while `event_source` itself is alive, and
        // we intentionally leak it rather than plumb a persistent owner
        // through the Backend trait for a single subscription's lifetime.
        on_message.forget();
    }

    fn cancel_job(&self, job: JobId) {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = Request::post(&format!("/api/jobs/{}/cancel", job.0)).send().await;
        });
    }
}
