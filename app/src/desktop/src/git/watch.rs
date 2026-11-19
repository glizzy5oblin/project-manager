//! Filesystem watcher: tells the app when a watched repository changed so its status can be
//! re-read. Events are debounced (a checkout touching thousands of files gives one refresh) and
//! noise that never changes status (object store writes, lock files) is ignored.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

/// Wait this long after the last event before reporting.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(300);
/// Report at least this often while events keep arriving.
pub const DEFAULT_MAX_WAIT: Duration = Duration::from_secs(2);

/// Watches repository roots. Dropping it stops watching and ends its thread.
pub struct RepoWatcher {
    watcher: notify::RecommendedWatcher,
    roots: std::sync::Arc<std::sync::Mutex<Vec<PathBuf>>>,
}

impl RepoWatcher {
    /// `on_change` runs on the watcher thread with the set of roots that changed.
    pub fn new(
        debounce: Duration,
        max_wait: Duration,
        on_change: impl Fn(BTreeSet<PathBuf>) + Send + 'static,
    ) -> notify::Result<Self> {
        let roots = std::sync::Arc::new(std::sync::Mutex::new(Vec::<PathBuf>::new()));
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let thread_roots = roots.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            let roots = thread_roots.lock().unwrap_or_else(|e| e.into_inner());
            for p in &event.paths {
                if let Some(root) = roots.iter().find(|r| p.starts_with(r))
                    && is_relevant(root, p)
                {
                    let _ = tx.send(root.clone());
                }
            }
        })?;
        std::thread::Builder::new()
            .name("pm-watch".into())
            .spawn(move || debounce_loop(rx, debounce, max_wait, on_change))
            .expect("spawn watcher thread");
        Ok(RepoWatcher { watcher, roots })
    }

    pub fn watch(&mut self, root: &Path) -> notify::Result<()> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        {
            let mut roots = self.roots.lock().unwrap_or_else(|e| e.into_inner());
            if roots.contains(&root) {
                return Ok(());
            }
            roots.push(root.clone());
        }
        self.watcher.watch(&root, RecursiveMode::Recursive)
    }

    pub fn unwatch(&mut self, root: &Path) -> notify::Result<()> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        self.roots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|r| r != &root);
        self.watcher.unwatch(&root)
    }
}

/// Whether a change at `path` inside `root` can change what the app shows.
pub(crate) fn is_relevant(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let mut parts = rel.components().map(|c| c.as_os_str().to_string_lossy());
    let first = parts.next();
    if path.extension().is_some_and(|e| e == "lock") {
        return false;
    }
    if first.as_deref() == Some(".git") {
        // HEAD, refs, index, packed-refs, MERGE_HEAD and friends matter; object writes and
        // logs only follow changes that are reported through those anyway.
        return !matches!(
            parts.next().as_deref(),
            Some("objects" | "logs" | "lfs" | "modules" | "hooks")
        );
    }
    true
}

fn debounce_loop(
    rx: mpsc::Receiver<PathBuf>,
    debounce: Duration,
    max_wait: Duration,
    on_change: impl Fn(BTreeSet<PathBuf>),
) {
    loop {
        // Block until something happens, or stop when the watcher is dropped.
        let Ok(first) = rx.recv() else { return };
        let started = Instant::now();
        let mut changed = BTreeSet::from([first]);
        loop {
            let left = max_wait.saturating_sub(started.elapsed());
            match rx.recv_timeout(debounce.min(left)) {
                Ok(p) => {
                    changed.insert(p);
                    if started.elapsed() >= max_wait {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    on_change(changed);
                    return;
                }
            }
        }
        on_change(changed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::shapes::linear;

    #[test]
    fn relevance_filter() {
        let root = Path::new("/r");
        assert!(is_relevant(root, Path::new("/r/src/a.rs")));
        assert!(is_relevant(root, Path::new("/r/.git/HEAD")));
        assert!(is_relevant(root, Path::new("/r/.git/refs/heads/main")));
        assert!(is_relevant(root, Path::new("/r/.git/index")));
        assert!(!is_relevant(root, Path::new("/r/.git/index.lock")));
        assert!(!is_relevant(root, Path::new("/r/.git/objects/ab/cdef")));
        assert!(!is_relevant(root, Path::new("/r/.git/logs/HEAD")));
        assert!(!is_relevant(root, Path::new("/elsewhere/a")));
    }

    fn wait_for(rx: &mpsc::Receiver<BTreeSet<PathBuf>>, secs: u64) -> Option<BTreeSet<PathBuf>> {
        rx.recv_timeout(Duration::from_secs(secs)).ok()
    }

    #[test]
    fn reports_working_tree_and_ref_changes_debounced() {
        let r = linear(1);
        let (tx, rx) = mpsc::channel();
        let mut w = RepoWatcher::new(
            Duration::from_millis(150),
            Duration::from_secs(2),
            move |s| {
                let _ = tx.send(s);
            },
        )
        .unwrap();
        w.watch(r.path()).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        // Many writes in a burst give one report.
        for i in 0..20 {
            r.write(&format!("f{i}.txt"), "x");
        }
        let got = wait_for(&rx, 5).expect("change reported");
        let root = std::fs::canonicalize(r.path()).unwrap();
        assert_eq!(got, BTreeSet::from([root.clone()]));
        std::thread::sleep(Duration::from_millis(400));
        while rx.try_recv().is_ok() {}

        // A commit made outside the app (refs and index change) is reported too.
        r.commit_all("outside");
        assert!(wait_for(&rx, 5).is_some(), "commit not reported");

        // After unwatch, nothing more arrives.
        std::thread::sleep(Duration::from_millis(400));
        while rx.try_recv().is_ok() {}
        w.unwatch(r.path()).unwrap();
        r.write("later.txt", "x");
        assert!(wait_for(&rx, 1).is_none(), "event after unwatch");
    }
}
