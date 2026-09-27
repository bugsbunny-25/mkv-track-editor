use mkv_types::{ApiError, JobId, JobOutcome, JobPhase, JobProgress};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Per-job state: a broadcast channel so multiple SSE subscribers (tabs,
/// reconnects) can watch the same job, the latest snapshot so a new
/// subscriber can catch up immediately, and a cancellation token so
/// `cancel_job` can stop the underlying `mkvmerge` process.
pub struct JobEntry {
    tx: broadcast::Sender<JobProgress>,
    latest: Mutex<JobProgress>,
    pub cancel: CancellationToken,
}

impl JobEntry {
    pub fn publish(&self, progress: JobProgress) {
        *self.latest.lock().unwrap() = progress.clone();
        let _ = self.tx.send(progress);
    }

    pub fn finish(&self, outcome: &Result<JobOutcome, ApiError>) {
        let phase = if outcome.is_ok() { JobPhase::Succeeded } else { JobPhase::Failed };
        let message = outcome.as_ref().err().map(|e| e.message.clone());
        let output_path = outcome.as_ref().ok().map(|o| o.output_path.clone());
        self.publish(JobProgress { phase, percent: 100, message, output_path });
    }

    pub fn subscribe(&self) -> (JobProgress, broadcast::Receiver<JobProgress>) {
        // Subscribe *before* reading the latest snapshot: any progress
        // published in between shows up as a (harmless) duplicate on the
        // live stream rather than being silently missed.
        let rx = self.tx.subscribe();
        let latest = self.latest.lock().unwrap().clone();
        (latest, rx)
    }
}

#[derive(Default)]
pub struct JobRegistry {
    jobs: Mutex<HashMap<JobId, Arc<JobEntry>>>,
}

impl JobRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn create(&self, id: JobId) -> Arc<JobEntry> {
        let (tx, _rx) = broadcast::channel(64);
        let entry = Arc::new(JobEntry {
            tx,
            latest: Mutex::new(JobProgress {
                phase: JobPhase::Queued,
                percent: 0,
                message: None,
                output_path: None,
            }),
            cancel: CancellationToken::new(),
        });
        self.jobs.lock().unwrap().insert(id, entry.clone());
        entry
    }

    pub fn get(&self, id: &JobId) -> Option<Arc<JobEntry>> {
        self.jobs.lock().unwrap().get(id).cloned()
    }
}
