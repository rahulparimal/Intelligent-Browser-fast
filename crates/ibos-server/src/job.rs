//! One background task at a time (re-index, folder index), with progress the admin page can poll.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};

use crate::App;

#[derive(Debug, Default)]
pub(crate) struct Job {
    state: Mutex<State>,
}

#[derive(Debug, Default, Clone)]
struct State {
    id: u64,
    running: bool,
    kind: Option<&'static str>,
    done: usize,
    total: usize,
    result: Option<Value>,
    error: Option<String>,
}

impl Job {
    pub(crate) fn snapshot(&self) -> Value {
        let s = self.state.lock().unwrap_or_else(PoisonError::into_inner).clone();
        json!({"id": s.id, "running": s.running, "kind": s.kind, "done": s.done, "total": s.total, "result": s.result, "error": s.error})
    }

    /// Forget the last result (used when the index is emptied).
    pub(crate) fn clear_result(&self) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.result = None;
        s.error = None;
    }

    fn begin(&self, kind: &'static str) -> Option<u64> {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if s.running {
            return None;
        }
        let id = s.id + 1;
        *s = State { id, running: true, kind: Some(kind), ..State::default() };
        Some(id)
    }

    fn progress(&self, done: usize, total: usize) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (s.done, s.total) = (done, total);
    }

    fn finish(&self, result: Result<Value, String>) {
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.running = false;
        match result {
            Ok(v) => (s.result, s.error) = (Some(v), None),
            Err(e) => (s.result, s.error) = (None, Some(e)),
        }
    }
}

/// Start a job on the blocking pool. Returns `false` if one is already running.
pub(crate) fn start(
    app: &Arc<App>,
    kind: &'static str,
    work: impl FnOnce(&App, &(dyn Fn(usize, usize) + Sync)) -> Result<Value, String> + Send + 'static,
) -> bool {
    if app.job.begin(kind).is_none() {
        return false;
    }
    let app = Arc::clone(app);
    tokio::task::spawn_blocking(move || {
        let progress = |d: usize, t: usize| app.job.progress(d, t);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&app, &progress)))
            .unwrap_or_else(|_| Err("indexing crashed".into()));
        app.job.finish(result);
    });
    true
}

pub(crate) fn start_reindex(app: &Arc<App>, force: bool) -> bool {
    start(app, "reindex", move |app, progress| {
        let stats = app.store.reindex_all(Some(progress), force).map_err(|e| e.to_string())?;
        serde_json::to_value(stats).map_err(|e| e.to_string())
    })
}

pub(crate) fn start_index(app: &Arc<App>, path: PathBuf) -> bool {
    start(app, "index", move |app, progress| {
        let stats = app
            .store
            .index_path(&path, ibos_search::IndexOptions::default(), Some(progress))
            .map_err(|e| e.to_string())?;
        serde_json::to_value(stats).map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};

    use ibos_search::LiteStore;
    use serde_json::json;

    use super::*;

    fn app() -> (tempfile::TempDir, Arc<App>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(LiteStore::open(Some(&dir.path().join("i.db")), None).unwrap());
        let app = Arc::new(App {
            store,
            job: Job::default(),
            port: AtomicU64::new(0),
            last_ping: AtomicU64::new(0),
            started: Instant::now(),
        });
        (dir, app)
    }

    async fn wait_idle(app: &Arc<App>) {
        for _ in 0..200 {
            if app.job.snapshot()["running"] == json!(false) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("job did not finish");
    }

    #[tokio::test]
    async fn only_one_job_runs_at_a_time_and_ids_increase() {
        let (_d, app) = app();
        let (release, gate) = std::sync::mpsc::channel::<()>();
        assert!(start(&app, "first", move |_, _| {
            gate.recv().ok();
            Ok(json!({"indexed": 1}))
        }));
        assert!(!start(&app, "second", |_, _| Ok(json!({}))), "refused while the first is running");
        assert_eq!(app.job.snapshot()["running"], json!(true));
        release.send(()).unwrap();
        wait_idle(&app).await;
        let s = app.job.snapshot();
        assert_eq!((s["id"].clone(), s["result"]["indexed"].clone()), (json!(1), json!(1)));
        assert!(start(&app, "third", |_, _| Ok(json!({}))));
        wait_idle(&app).await;
        assert_eq!(app.job.snapshot()["id"], json!(2), "a fast job is still visible to the page through its new id");
    }

    #[tokio::test]
    async fn errors_and_panics_are_reported_not_fatal() {
        let (_d, app) = app();
        assert!(start(&app, "bad", |_, _| Err("disk full".into())));
        wait_idle(&app).await;
        assert_eq!(app.job.snapshot()["error"], json!("disk full"));
        assert!(start(&app, "crash", |_, _| panic!("boom")));
        wait_idle(&app).await;
        assert_eq!(app.job.snapshot()["error"], json!("indexing crashed"));
        assert!(start(&app, "ok", |_, _| Ok(json!({}))), "the runner recovers after a crash");
        wait_idle(&app).await;
        app.job.clear_result();
        assert_eq!(app.job.snapshot()["result"], json!(null));
    }
}
