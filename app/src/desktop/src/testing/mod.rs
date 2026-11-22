//! Test fixture generator: builds git repositories of known shapes in temporary folders.
//!
//! Every repo is deterministic (fixed authors and an advancing fake clock, so commit ids are
//! stable) and isolated from the developer's own git config (no signing, hooks, templates or
//! global settings leak in). Shapes cover what the git layer, graph and backup tests need:
//! linear, many branches, merges, octopus merge, conflicts, large history, LFS, and a clone with
//! an upstream for ahead/behind.

#![allow(dead_code)] // Shapes are used by later tests as features land.

use std::cell::Cell;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::git::GitCommand;

/// 2020-01-01T00:00:00Z. Each commit advances the clock by one minute.
const EPOCH: i64 = 1_577_836_800;

pub struct FixtureRepo {
    _dir: TempDir,
    path: PathBuf,
    /// Isolated "global" config file (may not exist; git treats that as empty).
    home: PathBuf,
    clock: Cell<i64>,
}

impl FixtureRepo {
    /// Empty repository on branch `main`.
    pub fn new() -> Self {
        Self::new_with(&[])
    }

    /// Empty repository; `init_args` are extra `git init` arguments (e.g. `--bare`).
    pub fn new_with(init_args: &[&str]) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("repo");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let repo = FixtureRepo {
            _dir: dir,
            path,
            home,
            clock: Cell::new(EPOCH),
        };
        let mut args = vec!["init", "-q", "-b", "main"];
        args.extend_from_slice(init_args);
        args.push(repo.path.to_str().unwrap());
        repo.cmd_in(repo.path.parent().unwrap(), &args);
        repo
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A sibling folder inside this fixture's temp dir (for clones, public repos, bundles).
    pub fn sibling(&self, name: &str) -> PathBuf {
        self.path.parent().unwrap().join(name)
    }

    /// Run git in the repo; panics with stderr on failure. Returns trimmed stdout.
    pub fn git(&self, args: &[&str]) -> String {
        self.cmd_in(&self.path, args)
    }

    /// Run git with the fixture's isolated environment in another folder.
    pub fn cmd_in(&self, cwd: &Path, args: &[&str]) -> String {
        self.command(cwd, args)
            .run()
            .unwrap_or_else(|e| panic!("fixture git failed: {e}"))
            .stdout_str()
    }

    /// The configured command, for tests that need to inspect failures or pass stdin.
    pub fn command(&self, cwd: &Path, args: &[&str]) -> GitCommand {
        let t = self.clock.get();
        let date = format!("@{t} +0000");
        let cmd = GitCommand::new(cwd)
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(["-c", "core.autocrlf=false"])
            // Outside the working tree, so tools that install hooks (git-lfs) leave no files in it.
            .arg("-c")
            .arg(format!(
                "core.hooksPath={}",
                self.home.join("no-hooks").display()
            ))
            .args(["-c", "init.templateDir=", "-c", "advice.detachedHead=false"])
            .args(args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date);
        self.isolated_env()
            .into_iter()
            .fold(cmd, |cmd, (k, v)| cmd.env(k, v))
    }

    /// The environment that keeps git away from the developer's own config: no system or
    /// global config (the fixture's own, possibly missing, file instead) and a fixed identity.
    /// For code under test that builds its own git commands.
    pub fn isolated_env(&self) -> Vec<(OsString, OsString)> {
        let home = self.home.as_os_str().to_os_string();
        vec![
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            (
                "GIT_CONFIG_GLOBAL".into(),
                self.home.join("gitconfig").into_os_string(),
            ),
            ("HOME".into(), home.clone()),
            ("XDG_CONFIG_HOME".into(), home),
            ("GIT_AUTHOR_NAME".into(), "Fixture Author".into()),
            ("GIT_AUTHOR_EMAIL".into(), "author@example.invalid".into()),
            ("GIT_COMMITTER_NAME".into(), "Fixture Committer".into()),
            (
                "GIT_COMMITTER_EMAIL".into(),
                "committer@example.invalid".into(),
            ),
        ]
    }

    fn tick(&self) {
        self.clock.set(self.clock.get() + 60);
    }

    pub fn write(&self, rel: &str, contents: impl AsRef<[u8]>) {
        let p = self.path.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path.join(rel)).unwrap()
    }

    pub fn remove(&self, rel: &str) {
        std::fs::remove_file(self.path.join(rel)).unwrap();
    }

    /// Stage everything and commit. Returns the new commit id.
    pub fn commit_all(&self, message: &str) -> String {
        self.tick();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.head()
    }

    /// Write one file and commit it.
    pub fn commit_file(&self, rel: &str, contents: &str, message: &str) -> String {
        self.write(rel, contents);
        self.commit_all(message)
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn rev(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev])
    }

    pub fn branch(&self, name: &str) {
        self.git(&["branch", name]);
    }

    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "-q", name]);
    }

    pub fn checkout_new(&self, name: &str) {
        self.git(&["checkout", "-q", "-b", name]);
    }

    /// Merge commit (always `--no-ff`) of the given branches into the current one.
    pub fn merge(&self, branches: &[&str], message: &str) -> String {
        self.tick();
        let mut args = vec!["merge", "-q", "--no-ff", "-m", message];
        args.extend_from_slice(branches);
        self.git(&args);
        self.head()
    }

    pub fn tag(&self, name: &str, message: Option<&str>) {
        self.tick();
        match message {
            Some(m) => self.git(&["tag", "-a", name, "-m", m]),
            None => self.git(&["tag", name]),
        };
    }

    pub fn commit_count(&self, rev: &str) -> usize {
        self.git(&["rev-list", "--count", rev]).parse().unwrap()
    }

    pub fn parents(&self, rev: &str) -> Vec<String> {
        let line = self.git(&["rev-list", "--parents", "-n", "1", rev]);
        line.split_whitespace()
            .skip(1)
            .map(str::to_string)
            .collect()
    }

    /// Whether `git lfs` is available for LFS fixtures.
    pub fn lfs_available() -> bool {
        GitCommand::new(".").args(["lfs", "version"]).run().is_ok()
    }
}

/// Shapes. Each returns a fresh repo checked out on `main` unless documented otherwise.
pub mod shapes {
    use super::*;

    /// `n` commits on `main`, each changing `file.txt`.
    pub fn linear(n: usize) -> FixtureRepo {
        let r = FixtureRepo::new();
        for i in 1..=n {
            r.commit_file("file.txt", &format!("line {i}\n"), &format!("commit {i}"));
        }
        r
    }

    /// `main` plus `count` feature branches, each forked from a different point with its own
    /// commits, and one tag. No merges.
    pub fn many_branches(count: usize) -> FixtureRepo {
        let r = linear(3);
        r.tag("v0.1.0", Some("first release"));
        for b in 0..count {
            r.checkout("main");
            let name = format!("feature/{b}");
            r.checkout_new(&name);
            for c in 0..=(b % 3) {
                r.commit_file(
                    &format!("feature{b}.txt"),
                    &format!("{c}\n"),
                    &format!("{name} work {c}"),
                );
            }
            r.checkout("main");
            r.commit_file(
                "file.txt",
                &format!("main after {name}\n"),
                &format!("main {b}"),
            );
        }
        r
    }

    /// Two feature branches merged back into `main` (one merge after main moved on), plus a
    /// release branch that merged `main` into itself.
    pub fn merges() -> FixtureRepo {
        let r = linear(2);
        r.checkout_new("feature/a");
        r.commit_file("a.txt", "a1\n", "a: first");
        r.commit_file("a.txt", "a2\n", "a: second");
        r.checkout("main");
        r.commit_file("main.txt", "m1\n", "main moves on");
        r.merge(&["feature/a"], "Merge feature/a");
        r.checkout_new("feature/b");
        r.commit_file("b.txt", "b1\n", "b: first");
        r.checkout("main");
        r.merge(&["feature/b"], "Merge feature/b");
        r.git(&["checkout", "-q", "-b", "release", "HEAD~1"]);
        r.commit_file("release.txt", "r1\n", "release prep");
        r.merge(&["main"], "Merge main into release");
        r.checkout("main");
        r
    }

    /// One octopus merge of three branches into `main`.
    pub fn octopus() -> FixtureRepo {
        let r = linear(1);
        for name in ["oct/a", "oct/b", "oct/c"] {
            r.checkout("main");
            r.checkout_new(name);
            r.commit_file(
                &format!("{}.txt", name.replace('/', "-")),
                "x\n",
                &format!("{name} work"),
            );
        }
        r.checkout("main");
        r.merge(&["oct/a", "oct/b", "oct/c"], "Octopus merge");
        r
    }

    /// `main` and `topic` both change line 1 of `conflict.txt`. Nothing is merged yet; call
    /// [`start_conflict`] to leave the repo mid-merge.
    pub fn conflict() -> FixtureRepo {
        let r = FixtureRepo::new();
        r.commit_file("conflict.txt", "base\n", "base");
        r.checkout_new("topic");
        r.commit_file("conflict.txt", "topic version\n", "topic edit");
        r.checkout("main");
        r.commit_file("conflict.txt", "main version\n", "main edit");
        r
    }

    /// Merge `topic` into `main` in a [`conflict`] repo, leaving the merge in progress.
    pub fn start_conflict(r: &FixtureRepo) {
        let out = r.command(r.path(), &["merge", "topic"]).output().unwrap();
        assert!(!out.success(), "merge was expected to conflict");
    }

    /// `n` linear commits written with `git fast-import` (thousands per second), each touching
    /// one of a few files. For performance and paging tests.
    pub fn large_history(n: usize) -> FixtureRepo {
        let r = FixtureRepo::new();
        let mut stream = String::with_capacity(n * 160);
        for i in 1..=n {
            let t = EPOCH + i as i64 * 60;
            let msg = format!("commit {i}\n");
            let content = format!("{i}\n");
            let _ = write!(
                stream,
                "commit refs/heads/main\nmark :{i}\n\
                 author Fixture Author <author@example.invalid> {t} +0000\n\
                 committer Fixture Committer <committer@example.invalid> {t} +0000\n\
                 data {}\n{msg}",
                msg.len()
            );
            if i > 1 {
                let _ = writeln!(stream, "from :{}", i - 1);
            }
            let _ = write!(
                stream,
                "M 100644 inline file{}.txt\ndata {}\n{content}\n",
                i % 7,
                content.len()
            );
        }
        r.command(r.path(), &["fast-import", "--quiet"])
            .stdin(stream)
            .run()
            .unwrap_or_else(|e| panic!("fast-import failed: {e}"));
        r.git(&["reset", "-q", "--hard", "main"]);
        r.clock.set(EPOCH + n as i64 * 60);
        r
    }

    /// `n` commits spread over `branches` branches (`main`, `b1`, `b2`, ...) with forks and
    /// merges between them, written with `git fast-import`. A fixed pseudo-random sequence
    /// (from `seed`) picks the shape, so the same arguments always give the same repo. With
    /// `same_second`, groups of five commits share a timestamp, to test date-order ties.
    /// Commit `i` changes `dir{i % 3}/file{branch}.txt`. For graph tests and benchmarks.
    pub fn branchy_history(n: usize, branches: usize, seed: u64, same_second: bool) -> FixtureRepo {
        let r = FixtureRepo::new();
        let mut rng = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let mut next = move |m: usize| {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((rng >> 33) % m as u64) as usize
        };
        let name = |b: usize| {
            if b == 0 {
                "main".to_string()
            } else {
                format!("b{b}")
            }
        };
        // Mark of each branch's tip (0 = not started).
        let mut tips = vec![0usize; branches.max(1)];
        let mut stream = String::with_capacity(n * 200);
        for i in 1..=n {
            let b = if i == 1 { 0 } else { next(tips.len()) };
            let t = EPOCH
                + if same_second {
                    (i / 5) as i64
                } else {
                    i as i64
                } * 60;
            let msg = format!("{} commit {i}\n", name(b));
            let _ = write!(
                stream,
                "commit refs/heads/{}\nmark :{i}\n\
                 author Fixture Author <author@example.invalid> {t} +0000\n\
                 committer Fixture Committer <committer@example.invalid> {t} +0000\n\
                 data {}\n{msg}",
                name(b),
                msg.len()
            );
            if tips[b] != 0 {
                let _ = writeln!(stream, "from :{}", tips[b]);
                // Sometimes merge another started branch in.
                let other = next(tips.len());
                if other != b && tips[other] != 0 && next(6) == 0 {
                    let _ = writeln!(stream, "merge :{}", tips[other]);
                }
            } else if i > 1 {
                // Fork a new branch from a recent commit.
                let _ = writeln!(stream, "from :{}", i - 1 - next((i - 1).min(20)));
            }
            let content = format!("{i}\n");
            let _ = write!(
                stream,
                "M 100644 inline dir{}/file{b}.txt\ndata {}\n{content}\n",
                i % 3,
                content.len()
            );
            tips[b] = i;
        }
        r.command(r.path(), &["fast-import", "--quiet"])
            .stdin(stream)
            .run()
            .unwrap_or_else(|e| panic!("fast-import failed: {e}"));
        r.git(&["reset", "-q", "--hard", "main"]);
        r.clock.set(EPOCH + n as i64 * 60);
        r
    }

    /// A repo with `*.bin` tracked by LFS and one LFS file committed. `None` when git-lfs is
    /// not installed, so callers can skip.
    pub fn lfs() -> Option<FixtureRepo> {
        if !FixtureRepo::lfs_available() {
            return None;
        }
        let r = FixtureRepo::new();
        r.git(&["lfs", "install", "--local"]);
        r.git(&["lfs", "track", "*.bin"]);
        r.commit_file("readme.txt", "lfs fixture\n", "track bin files");
        r.write("assets/big.bin", vec![7u8; 4096]);
        r.commit_all("add lfs asset");
        Some(r)
    }

    /// A working clone of a bare "remote", with `main` tracking `origin/main`. The clone is 2
    /// commits ahead and 1 behind its upstream. Returns (clone, remote).
    pub fn ahead_behind() -> (FixtureRepo, FixtureRepo) {
        let remote = FixtureRepo::new_with(&["--bare"]);
        let seed = linear(2);
        seed.git(&["push", "-q", remote.path().to_str().unwrap(), "main"]);

        let clone = FixtureRepo::new();
        let url = remote.path().to_str().unwrap();
        clone.git(&["remote", "add", "origin", url]);
        clone.git(&["fetch", "-q", "origin"]);
        clone.git(&["reset", "-q", "--hard", "origin/main"]);
        clone.git(&["branch", "-q", "--set-upstream-to=origin/main", "main"]);

        // Someone else pushes one commit...
        seed.commit_file("other.txt", "theirs\n", "their commit");
        seed.git(&["push", "-q", url, "main"]);
        clone.git(&["fetch", "-q", "origin"]);
        // ...while the clone made two of its own.
        clone.clock.set(seed.clock.get() + 600);
        clone.commit_file("mine.txt", "1\n", "my commit 1");
        clone.commit_file("mine.txt", "2\n", "my commit 2");
        (clone, remote)
    }
}

#[cfg(test)]
mod tests {
    use super::shapes::*;
    use super::*;

    #[test]
    fn linear_is_deterministic() {
        let a = linear(3);
        let b = linear(3);
        assert_eq!(a.commit_count("HEAD"), 3);
        assert_eq!(
            a.head(),
            b.head(),
            "same inputs must give the same commit ids"
        );
    }

    #[test]
    fn many_branches_shape() {
        let r = many_branches(5);
        let branches = r.git(&["branch", "--format=%(refname:short)"]);
        assert_eq!(branches.lines().count(), 6);
        assert_eq!(r.git(&["tag"]), "v0.1.0");
        assert_eq!(r.git(&["rev-list", "--merges", "--all"]), "");
    }

    #[test]
    fn merges_shape() {
        let r = merges();
        let merges = r.git(&["rev-list", "--merges", "--all"]);
        assert_eq!(merges.lines().count(), 3);
        assert_eq!(r.parents("main").len(), 2);
    }

    #[test]
    fn octopus_has_four_parents() {
        let r = octopus();
        assert_eq!(r.parents("HEAD").len(), 4);
    }

    #[test]
    fn conflict_can_be_started() {
        let r = conflict();
        start_conflict(&r);
        let status = r.git(&["status", "--porcelain"]);
        assert_eq!(status, "UU conflict.txt");
    }

    #[test]
    fn large_history_is_fast_and_complete() {
        let started = std::time::Instant::now();
        let r = large_history(5000);
        assert_eq!(r.commit_count("main"), 5000);
        assert_eq!(r.git(&["status", "--porcelain"]), "");
        assert!(started.elapsed().as_secs() < 30);
    }

    #[test]
    fn lfs_fixture_stores_a_pointer() {
        let Some(r) = lfs() else {
            eprintln!("git-lfs not installed; skipping");
            return;
        };
        let blob = r.git(&["cat-file", "-p", "HEAD:assets/big.bin"]);
        assert!(
            blob.starts_with("version https://git-lfs.github.com/spec/v1"),
            "{blob}"
        );
    }

    #[test]
    fn ahead_behind_counts() {
        let (clone, _remote) = ahead_behind();
        let counts = clone.git(&["rev-list", "--left-right", "--count", "main...origin/main"]);
        assert_eq!(counts.split_whitespace().collect::<Vec<_>>(), ["2", "1"]);
    }

    #[test]
    fn user_config_does_not_leak_in() {
        let r = FixtureRepo::new();
        let out = r
            .command(r.path(), &["config", "--global", "--list"])
            .output()
            .unwrap();
        assert_eq!(out.stdout_str(), "");
    }
}
