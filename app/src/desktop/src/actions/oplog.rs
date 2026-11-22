//! The operation log: every git command a write action ran, and every backup it took, with the
//! exit code, how long it took and git's error output. Shown to the user for transparency and
//! debugging. The latest [`CAPACITY`] entries are kept in memory and, when the log has a file
//! ([`OpLog::persistent`]), also on disk as JSON lines, so they survive a restart. Each entry is
//! also copied to the app log.
//!
//! Nothing secret reaches it: command lines and stderr are redacted by the git runner (URL
//! credentials), and commit messages go to git on stdin, not on the command line.

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git::{GitError, GitOutput};

/// Entries kept; older ones are dropped.
pub const CAPACITY: usize = 500;
/// Longest stderr kept per entry, in bytes.
const STDERR_LIMIT: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum OpKind {
    /// A git command; `command` is how it would be typed.
    Git,
    /// A backup taken before a dangerous action; `command` names the backup.
    Backup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OpEntry {
    /// Increases by one per entry, from 1.
    #[ts(type = "number")]
    pub seq: u64,
    /// Unix milliseconds.
    #[ts(type = "number")]
    pub time: u64,
    /// Repository root.
    pub repo: String,
    /// The action this was part of, e.g. `reset`.
    pub action: String,
    pub kind: OpKind,
    pub command: String,
    pub ok: bool,
    /// Exit code; `None` when git did not run to the end (killed, cancelled, not started).
    pub code: Option<i32>,
    #[ts(type = "number")]
    pub duration_ms: u64,
    /// git's error output, or why the step failed; trimmed and shortened.
    pub stderr: String,
}

type Listener = Box<dyn Fn(&OpEntry) + Send + Sync>;

struct Inner {
    entries: VecDeque<OpEntry>,
    next: u64,
    /// Where entries are saved, if anywhere.
    file: Option<PathBuf>,
    /// Lines appended to the file since it was last rewritten.
    appended: usize,
}

impl Inner {
    /// Write the kept entries to the file, replacing what it had.
    fn rewrite(&mut self) {
        let Some(path) = &self.file else { return };
        let mut text = String::new();
        for e in &self.entries {
            if let Ok(line) = serde_json::to_string(e) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        if let Err(e) = fs::write(path, text) {
            log::warn!("could not save the operation log: {e}");
        }
        self.appended = 0;
    }

    /// Append one entry to the file; rewrite it now and then so it stays near [`CAPACITY`] lines.
    fn save(&mut self, entry: &OpEntry) {
        let Some(path) = &self.file else { return };
        if self.appended >= CAPACITY {
            return self.rewrite();
        }
        let Ok(line) = serde_json::to_string(entry) else {
            return;
        };
        let written = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| writeln!(f, "{line}"));
        match written {
            Ok(()) => self.appended += 1,
            Err(e) => log::warn!("could not save the operation log: {e}"),
        }
    }
}

#[derive(Clone)]
pub struct OpLog {
    inner: Arc<Mutex<Inner>>,
    listener: Arc<Option<Listener>>,
}

impl Default for OpLog {
    fn default() -> Self {
        Self::new()
    }
}

impl OpLog {
    pub fn new() -> Self {
        OpLog {
            inner: Arc::new(Mutex::new(Inner {
                entries: VecDeque::new(),
                next: 1,
                file: None,
                appended: 0,
            })),
            listener: Arc::new(None),
        }
    }

    /// A log that also hands each new entry to `listener` (the app sends it to the window).
    pub fn with_listener(listener: impl Fn(&OpEntry) + Send + Sync + 'static) -> Self {
        OpLog {
            listener: Arc::new(Some(Box::new(listener))),
            ..Self::new()
        }
    }

    /// A log saved to `path` (JSON lines): the entries already there are loaded, and every new
    /// one is appended. A missing or damaged file just means fewer entries.
    pub fn persistent(path: PathBuf, listener: impl Fn(&OpEntry) + Send + Sync + 'static) -> Self {
        let log = Self::with_listener(listener);
        {
            let mut inner = log.lock();
            let text = fs::read_to_string(&path).unwrap_or_default();
            for entry in text
                .lines()
                .filter_map(|l| serde_json::from_str::<OpEntry>(l).ok())
            {
                if inner.entries.len() == CAPACITY {
                    inner.entries.pop_front();
                }
                inner.next = inner.next.max(entry.seq + 1);
                inner.entries.push_back(entry);
            }
            inner.file = Some(path);
            inner.rewrite();
        }
        log
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Oldest first.
    pub fn entries(&self) -> Vec<OpEntry> {
        self.lock().entries.iter().cloned().collect()
    }

    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.entries.clear();
        inner.rewrite();
    }

    /// Add `entry`, filling in its sequence number and time.
    fn push(&self, mut entry: OpEntry) {
        entry.stderr = shorten(entry.stderr.trim());
        {
            let mut inner = self.lock();
            entry.seq = inner.next;
            entry.time = now_ms();
            inner.next += 1;
            if inner.entries.len() == CAPACITY {
                inner.entries.pop_front();
            }
            inner.entries.push_back(entry.clone());
            inner.save(&entry);
        }
        if entry.ok {
            log::info!(
                "[{}] {} ({} ms)",
                entry.action,
                entry.command,
                entry.duration_ms
            );
        } else {
            log::warn!(
                "[{}] {} failed ({:?}): {}",
                entry.action,
                entry.command,
                entry.code,
                entry.stderr
            );
        }
        if let Some(listener) = self.listener.as_ref() {
            listener(&entry);
        }
    }

    fn entry(repo: &str, action: &str, kind: OpKind, command: String) -> OpEntry {
        OpEntry {
            seq: 0,
            time: 0,
            repo: repo.to_string(),
            action: action.to_string(),
            kind,
            command,
            ok: false,
            code: None,
            duration_ms: 0,
            stderr: String::new(),
        }
    }

    /// Record a git command that ran (whatever its exit code).
    pub fn git(&self, repo: &str, action: &str, out: &GitOutput) {
        self.push(OpEntry {
            ok: out.success(),
            code: out.code,
            duration_ms: millis(out.duration),
            stderr: out.stderr.clone(),
            ..Self::entry(repo, action, OpKind::Git, out.command.clone())
        });
    }

    /// Record a git command that could not run to the end (not started, cancelled).
    pub fn git_error(&self, repo: &str, action: &str, command: String, err: &GitError) {
        self.push(OpEntry {
            stderr: err.to_string(),
            ..Self::entry(repo, action, OpKind::Git, command)
        });
    }

    /// Record a backup: `Ok(id)` when it was taken, or why it failed.
    pub fn backup(&self, repo: &str, action: &str, result: Result<&str, String>, took: Duration) {
        let base = |command| Self::entry(repo, action, OpKind::Backup, command);
        self.push(match result {
            Ok(id) => OpEntry {
                ok: true,
                duration_ms: millis(took),
                ..base(format!("backup {id}"))
            },
            Err(why) => OpEntry {
                duration_ms: millis(took),
                stderr: why,
                ..base(format!("backup before {action}"))
            },
        });
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn shorten(s: &str) -> String {
    if s.len() <= STDERR_LIMIT {
        return s.to_string();
    }
    let mut end = STDERR_LIMIT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
