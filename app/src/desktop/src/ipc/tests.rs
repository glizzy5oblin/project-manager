use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use super::*;
use crate::git::{CancelToken, GitCommand, GitError, Progress};
use crate::testing::shapes::linear;

struct ChannelSink(Mutex<mpsc::Sender<TaskEvent>>);

impl EventSink for ChannelSink {
    fn emit(&self, event: &TaskEvent) {
        let _ = self.0.lock().unwrap().send(event.clone());
    }
}

fn manager() -> (TaskManager, mpsc::Receiver<TaskEvent>) {
    let (tx, rx) = mpsc::channel();
    (TaskManager::new(ChannelSink(Mutex::new(tx))), rx)
}

/// Collect events until task `id` finishes.
fn finish(rx: &mpsc::Receiver<TaskEvent>, id: TaskId) -> (Vec<TaskEvent>, TaskOutcome) {
    let mut seen = Vec::new();
    loop {
        let e = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("task finished in time");
        if let TaskEvent::Finished { id: done, outcome } = &e
            && *done == id
        {
            return (seen, outcome.clone());
        }
        seen.push(e);
    }
}

#[test]
fn result_is_reported() {
    let (tasks, rx) = manager();
    let id = tasks
        .spawn("sum", "Adding", None, |_| Ok::<_, AppError>(vec![1, 2, 3]))
        .unwrap();
    let (seen, outcome) = finish(&rx, id);
    assert!(matches!(&seen[0], TaskEvent::Started { task } if task.id == id && task.kind == "sum"));
    assert_eq!(
        outcome,
        TaskOutcome::Ok {
            value: serde_json::json!([1, 2, 3])
        }
    );
    assert!(tasks.running().is_empty());
}

#[test]
fn errors_and_panics_become_app_errors() {
    let (tasks, rx) = manager();
    let id = tasks
        .spawn("x", "x", None, |_| -> Result<(), AppError> {
            Err(AppError::not_found("nope"))
        })
        .unwrap();
    let (_, outcome) = finish(&rx, id);
    assert!(matches!(outcome, TaskOutcome::Failed { error } if error.kind == ErrorKind::NotFound));

    let id = tasks
        .spawn("x", "x", None, |_| -> Result<(), AppError> {
            panic!("boom")
        })
        .unwrap();
    let (_, outcome) = finish(&rx, id);
    match outcome {
        TaskOutcome::Failed { error } => {
            assert_eq!(error.kind, ErrorKind::Internal);
            assert!(error.message.contains("boom"));
        }
        other => panic!("{other:?}"),
    }
    assert!(tasks.running().is_empty(), "a panicking task is removed");
}

/// Cancelling stops a real git process, and the outcome says cancelled rather than failed.
#[test]
fn cancel_stops_a_running_git_command() {
    let (tasks, rx) = manager();
    let r = linear(1);
    let root = r.path().to_path_buf();
    let (started_tx, started_rx) = mpsc::channel();
    let id = tasks
        .spawn("wait", "Waiting", None, move |ctx| {
            started_tx.send(()).unwrap();
            // A shell alias that sleeps far longer than the test waits.
            let out = GitCommand::new(&root)
                .args(["-c", "alias.pmwait=!sleep 30", "pmwait"])
                .run_with(&ctx.cancel, |_| {});
            match out {
                Err(e @ GitError::Cancelled { .. }) => Err(e.into()),
                other => Ok(format!("{other:?}")),
            }
        })
        .unwrap();
    started_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(tasks.cancel(id));
    let (_, outcome) = finish(&rx, id);
    assert_eq!(outcome, TaskOutcome::Cancelled);
    assert!(!tasks.cancel(id), "finished tasks cannot be cancelled");
}

#[test]
fn one_task_per_repository() {
    let (tasks, rx) = manager();
    let gate = CancelToken::new();
    let g = gate.clone();
    let lock = Some(std::path::PathBuf::from("/repo"));
    let first = tasks
        .spawn("fetch", "Fetching", lock.clone(), move |_| {
            while !g.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok::<_, AppError>(())
        })
        .unwrap();
    let busy = tasks
        .spawn("push", "Pushing", lock.clone(), |_| Ok::<_, AppError>(()))
        .unwrap_err();
    assert_eq!(busy.kind, ErrorKind::Busy);
    assert!(busy.message.contains("Fetching"));
    // A different repository is fine.
    let other = tasks
        .spawn("push", "Pushing", Some("/other".into()), |_| {
            Ok::<_, AppError>(())
        })
        .unwrap();
    finish(&rx, other);
    assert_eq!(tasks.running().len(), 1);
    gate.cancel();
    finish(&rx, first);
    // The lock is released.
    let again = tasks
        .spawn("push", "Pushing", lock, |_| Ok::<_, AppError>(()))
        .unwrap();
    finish(&rx, again);
}

#[test]
fn progress_is_rate_limited_per_phase() {
    let (tasks, rx) = manager();
    let id = tasks
        .spawn("p", "p", None, |ctx| {
            for i in 0..=100u8 {
                ctx.progress(Progress::parse(&format!(
                    "Receiving objects: {i:3}% ({i}/100)"
                )));
            }
            ctx.progress(Progress::parse("Resolving deltas: 0% (0/5)"));
            Ok::<_, AppError>(())
        })
        .unwrap();
    let (seen, _) = finish(&rx, id);
    let progress: Vec<_> = seen
        .iter()
        .filter_map(|e| match e {
            TaskEvent::Progress { phase, percent, .. } => Some((phase.clone(), *percent)),
            _ => None,
        })
        .collect();
    assert!(progress.len() < 10, "{progress:?}");
    assert_eq!(progress[0], ("Receiving objects".into(), Some(0)));
    assert!(progress.contains(&("Receiving objects".into(), Some(100))));
    assert_eq!(
        progress.last().unwrap(),
        &("Resolving deltas".into(), Some(0))
    );
}

/// The wire format the frontend relies on.
#[test]
fn event_json_shape() {
    let e = TaskEvent::Finished {
        id: 7,
        outcome: TaskOutcome::Failed {
            error: AppError::new(ErrorKind::GitMissing, "no git").with_detail("x"),
        },
    };
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        serde_json::json!({
            "type": "finished",
            "id": 7,
            "outcome": {
                "status": "failed",
                "error": {"kind": "gitMissing", "message": "no git", "detail": "x", "issues": []}
            }
        })
    );
    let e = TaskEvent::Progress {
        id: 1,
        phase: "p".into(),
        percent: None,
        message: "m".into(),
    };
    assert_eq!(serde_json::to_value(&e).unwrap()["type"], "progress");
}

#[test]
fn git_errors_map_to_kinds() {
    let e: AppError = GitError::AuthFailed {
        command: "git push".into(),
        stderr: "fatal: Authentication failed".into(),
        hint: "Sign in again".into(),
    }
    .into();
    assert_eq!(e.kind, ErrorKind::Auth);
    assert_eq!(e.message, "Sign in again");
    assert_eq!(e.detail.as_deref(), Some("fatal: Authentication failed"));

    let e: AppError = GitError::Failed {
        command: "git merge x".into(),
        code: Some(1),
        stderr: String::new(),
    }
    .into();
    assert_eq!(e.kind, ErrorKind::Git);
    assert_eq!(e.detail, None, "empty stderr gives no detail");

    let e: AppError = crate::proj::ProjFile::parse("schema_version = 1\n[project]\nname = \"\"\n")
        .unwrap_err()
        .into();
    assert_eq!(e.kind, ErrorKind::InvalidProject);
}
