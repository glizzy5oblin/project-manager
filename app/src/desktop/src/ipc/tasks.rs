//! Background tasks: long work (clone, fetch, restore) runs on its own thread and reports through
//! [`TaskEvent`]s, so commands return at once and the window never freezes. Each task can be
//! cancelled. Tasks that write to a repository take that repository's lock, so two of them never
//! run on the same repository at once.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use ts_rs::TS;

use super::{AppError, ErrorKind};
use crate::git::{CancelToken, Progress};

pub type TaskId = u64;

/// Progress events closer together than this are dropped (except the last one of a phase).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    #[ts(type = "number")]
    pub id: TaskId,
    /// Machine name, e.g. `clone`.
    pub kind: String,
    /// Shown to the user, e.g. `Cloning ExampleMod`.
    pub title: String,
}

/// Sent to the frontend on the `task` event.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TaskEvent {
    Started {
        task: TaskInfo,
    },
    Progress {
        #[ts(type = "number")]
        id: TaskId,
        /// e.g. `Receiving objects`; empty for a plain message.
        phase: String,
        percent: Option<u8>,
        message: String,
    },
    Finished {
        #[ts(type = "number")]
        id: TaskId,
        outcome: TaskOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum TaskOutcome {
    /// `value` is the task's result; its type depends on the command that started it.
    Ok {
        #[ts(type = "unknown")]
        value: serde_json::Value,
    },
    Failed {
        error: AppError,
    },
    Cancelled,
}

/// Where task events go: the Tauri window in the app, a channel in tests.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: &TaskEvent);
}

struct Running {
    info: TaskInfo,
    cancel: CancelToken,
    lock: Option<PathBuf>,
}

struct Inner {
    next: AtomicU64,
    running: Mutex<BTreeMap<TaskId, Running>>,
    sink: Box<dyn EventSink>,
}

impl Inner {
    fn running(&self) -> MutexGuard<'_, BTreeMap<TaskId, Running>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<Inner>,
}

/// Handed to the task body: its id, its cancel token, and a way to report progress.
pub struct TaskCtx {
    pub id: TaskId,
    pub cancel: CancelToken,
    inner: Arc<Inner>,
    last_progress: Mutex<Option<(Instant, String)>>,
}

impl TaskCtx {
    /// Report a git progress line. Rate-limited; a phase change or 100% always goes through.
    pub fn progress(&self, p: Progress) {
        let mut last = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if let Some((at, phase)) = last.as_ref()
            && *phase == p.phase
            && now.duration_since(*at) < PROGRESS_INTERVAL
            && p.percent != Some(100)
        {
            return;
        }
        *last = Some((now, p.phase.clone()));
        drop(last);
        self.inner.sink.emit(&TaskEvent::Progress {
            id: self.id,
            phase: p.phase,
            percent: p.percent,
            message: p.line,
        });
    }

    /// Report a step that is not a git progress line.
    pub fn message(&self, text: impl Into<String>) {
        self.progress(Progress {
            phase: String::new(),
            percent: None,
            current: None,
            total: None,
            line: text.into(),
        });
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// `Err(Cancelled)` if the user cancelled; call between steps.
    pub fn check(&self) -> Result<(), AppError> {
        if self.is_cancelled() {
            Err(AppError::new(ErrorKind::Cancelled, "Cancelled"))
        } else {
            Ok(())
        }
    }
}

impl TaskManager {
    pub fn new(sink: impl EventSink) -> Self {
        TaskManager {
            inner: Arc::new(Inner {
                next: AtomicU64::new(1),
                running: Mutex::new(BTreeMap::new()),
                sink: Box::new(sink),
            }),
        }
    }

    /// Start `body` on a new thread and return its id at once. With `lock` set, fails with
    /// [`ErrorKind::Busy`] while another task holds the same lock (a repository path).
    pub fn spawn<T, F>(
        &self,
        kind: &str,
        title: impl Into<String>,
        lock: Option<PathBuf>,
        body: F,
    ) -> Result<TaskId, AppError>
    where
        T: Serialize,
        F: FnOnce(&TaskCtx) -> Result<T, AppError> + Send + 'static,
    {
        let id = self.inner.next.fetch_add(1, Ordering::SeqCst);
        let info = TaskInfo {
            id,
            kind: kind.to_string(),
            title: title.into(),
        };
        let cancel = CancelToken::new();
        {
            let mut running = self.inner.running();
            if let Some(path) = &lock
                && let Some(other) = running.values().find(|r| r.lock.as_ref() == Some(path))
            {
                return Err(AppError::new(
                    ErrorKind::Busy,
                    format!(
                        "Wait for \"{}\" to finish, or cancel it, first",
                        other.info.title
                    ),
                ));
            }
            running.insert(
                id,
                Running {
                    info: info.clone(),
                    cancel: cancel.clone(),
                    lock,
                },
            );
        }
        self.inner.sink.emit(&TaskEvent::Started { task: info });

        let ctx = TaskCtx {
            id,
            cancel,
            inner: self.inner.clone(),
            last_progress: Mutex::new(None),
        };
        let spawned = std::thread::Builder::new()
            .name(format!("pm-task-{id}"))
            .spawn(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(|| body(&ctx)));
                let outcome = match result {
                    Ok(Ok(value)) => match serde_json::to_value(value) {
                        Ok(value) => TaskOutcome::Ok { value },
                        Err(e) => TaskOutcome::Failed {
                            error: AppError::internal(format!("could not encode the result: {e}")),
                        },
                    },
                    Ok(Err(e)) if e.kind == ErrorKind::Cancelled || ctx.is_cancelled() => {
                        TaskOutcome::Cancelled
                    }
                    Ok(Err(error)) => TaskOutcome::Failed { error },
                    Err(panic) => {
                        let what = panic
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| panic.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown panic".into());
                        log::error!("task {id} panicked: {what}");
                        TaskOutcome::Failed {
                            error: AppError::internal(format!("internal error: {what}")),
                        }
                    }
                };
                ctx.inner.running().remove(&id);
                ctx.inner.sink.emit(&TaskEvent::Finished { id, outcome });
            });
        if let Err(e) = spawned {
            self.inner.running().remove(&id);
            let error = AppError::internal(format!("could not start a thread: {e}"));
            self.inner.sink.emit(&TaskEvent::Finished {
                id,
                outcome: TaskOutcome::Failed {
                    error: error.clone(),
                },
            });
            return Err(error);
        }
        Ok(id)
    }

    /// Ask a task to stop. `false` when no such task is running.
    pub fn cancel(&self, id: TaskId) -> bool {
        match self.inner.running().get(&id) {
            Some(r) => {
                r.cancel.cancel();
                true
            }
            None => false,
        }
    }

    pub fn running(&self) -> Vec<TaskInfo> {
        self.inner
            .running()
            .values()
            .map(|r| r.info.clone())
            .collect()
    }

    /// Cancel everything (on app exit).
    pub fn cancel_all(&self) {
        for r in self.inner.running().values() {
            r.cancel.cancel();
        }
    }
}
