//! Backups taken before dangerous actions, and restoring them.
//!
//! A backup with id `<id>` is a set of refs under `refs/pm/backups/<id>/`:
//!
//! - `head`: the commit HEAD pointed at (absent on an unborn branch);
//! - `heads/<branch>` and `tags/<tag>`: every local branch and tag;
//! - `index`: a commit whose tree is the staged index (absent when the index had conflicts);
//! - `worktree`: a commit whose tree is the whole working tree, untracked files included
//!   (ignored files are not), built in a temporary index so the real index and `refs/stash`
//!   are never touched.
//!
//! Optionally the refs are also written to a `git bundle` outside the repository. The backup
//! index (`<git dir>/pm/backups.json`) lists them for the Undo UI. Refs keep every backed-up
//! object alive through `git gc`. Git LFS content is stored by git-lfs in `.git/lfs/objects`
//! when the snapshot is taken; bundles carry only the pointers (see `docs/backups.md`).

#[cfg(test)]
mod tests;

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git::{GitCommand, GitError};

pub const REF_PREFIX: &str = "refs/pm/backups/";
const INDEX_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the backup index {} is damaged: {message}", path.display())]
    Index { path: PathBuf, message: String },
    #[error("invalid action name {0:?}: use 1 to 40 lowercase letters, digits and dashes")]
    BadAction(String),
    #[error("no backup with id {0:?}")]
    NotFound(String),
    #[error("backup {0:?} has no bundle file")]
    NoBundle(String),
    #[error("refused to touch {path:?}: {reason}")]
    UnsafePath { path: String, reason: &'static str },
}

impl BackupError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        BackupError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RefTip {
    pub name: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecord {
    pub id: String,
    /// What the backup was taken before, e.g. `reset-hard`, `rebase`, `restore`.
    pub action: String,
    /// Unix seconds.
    #[ts(type = "number")]
    pub created: u64,
    /// Branch HEAD was on (short name), `None` when detached.
    pub head_branch: Option<String>,
    pub head_commit: Option<String>,
    pub branches: Vec<RefTip>,
    pub tags: Vec<RefTip>,
    /// Commit holding the index tree; `None` when the index had conflicts.
    pub index_commit: Option<String>,
    /// Commit holding the working tree including untracked files.
    pub worktree_commit: Option<String>,
    /// Files that were untracked at backup time (restored as untracked).
    pub untracked: Vec<String>,
    /// The index had unresolved conflicts; restoring brings back the files but not the
    /// conflict stages.
    pub conflicted: bool,
    /// The repository uses Git LFS.
    pub lfs: bool,
    #[ts(type = "string | null")]
    pub bundle: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    backups: Vec<BackupRecord>,
}

#[derive(Debug, Clone, Default)]
pub struct BackupOptions {
    /// Also write a `git bundle` into this folder.
    pub bundle_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    /// The backup taken just before restoring (restore it to undo the restore).
    pub pre_restore: BackupRecord,
    pub warnings: Vec<String>,
}

/// How many backups to keep. Both limits apply; `None` means no limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Retention {
    pub max_count: Option<u32>,
    pub max_age_days: Option<u32>,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            max_count: Some(50),
            max_age_days: Some(30),
        }
    }
}

/// Backups of one repository (working tree root).
pub struct Backups {
    root: PathBuf,
    git_dir: PathBuf,
}

impl Backups {
    pub fn open(root: &Path) -> Result<Self, BackupError> {
        let git_dir = PathBuf::from(
            git(root)
                .args(["rev-parse", "--absolute-git-dir"])
                .run()?
                .stdout_str(),
        );
        Ok(Backups {
            root: root.to_path_buf(),
            git_dir,
        })
    }

    fn index_path(&self) -> PathBuf {
        self.git_dir.join("pm").join("backups.json")
    }

    /// All backups, newest first.
    pub fn list(&self) -> Result<Vec<BackupRecord>, BackupError> {
        let mut b = self.read_index()?.backups;
        b.sort_by(|a, b| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
        Ok(b)
    }

    pub fn get(&self, id: &str) -> Result<BackupRecord, BackupError> {
        self.read_index()?
            .backups
            .into_iter()
            .find(|b| b.id == id)
            .ok_or_else(|| BackupError::NotFound(id.to_string()))
    }

    fn read_index(&self) -> Result<IndexFile, BackupError> {
        let path = self.index_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| BackupError::Index {
                path,
                message: e.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(IndexFile {
                version: INDEX_VERSION,
                backups: Vec::new(),
            }),
            Err(e) => Err(BackupError::io(&path, e)),
        }
    }

    fn write_index(&self, index: &IndexFile) -> Result<(), BackupError> {
        let path = self.index_path();
        let dir = path.parent().expect("index has a parent");
        std::fs::create_dir_all(dir).map_err(|e| BackupError::io(dir, e))?;
        let text = serde_json::to_string_pretty(index).expect("index serialises");
        let tmp = dir.join("backups.json.tmp");
        std::fs::write(&tmp, text).map_err(|e| BackupError::io(&tmp, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| BackupError::io(&path, e))
    }

    /// Take a backup before `action`. Any failure is returned and nothing is recorded, so the
    /// caller must not run the action (dangerous actions never proceed without a backup).
    pub fn create(&self, action: &str, opts: &BackupOptions) -> Result<BackupRecord, BackupError> {
        validate_action(action)?;
        let now = unix_now();
        let mut index = self.read_index()?;
        let base = format!("{}-{action}", format_utc(now));
        let mut id = base.clone();
        let mut n = 2;
        while index.backups.iter().any(|b| b.id == id) || self.has_refs(&id)? {
            id = format!("{base}-{n}");
            n += 1;
        }
        let prefix = format!("{REF_PREFIX}{id}/");

        let head_commit = self.rev_opt("HEAD")?;
        let head_branch = {
            let o = git(&self.root)
                .args(["symbolic-ref", "-q", "--short", "HEAD"])
                .output()?;
            o.success().then(|| o.stdout_str())
        };
        let branches = self.tips("refs/heads/")?;
        let tags = self.tips("refs/tags/")?;

        // Index tree: fails when there are unmerged entries.
        let index_tree = {
            let o = git(&self.root).arg("write-tree").output()?;
            o.success().then(|| o.stdout_str())
        };
        let conflicted = index_tree.is_none();
        let index_commit = match &index_tree {
            Some(tree) => Some(self.commit_tree(
                tree,
                head_commit.as_deref(),
                &format!("pm backup {id}: index"),
            )?),
            None => None,
        };

        // Working tree including untracked files, via a temporary copy of the index.
        let (worktree_tree, untracked) = self.snapshot_worktree()?;
        let worktree_commit = self.commit_tree(
            &worktree_tree,
            head_commit.as_deref(),
            &format!("pm backup {id}: working tree ({action})"),
        )?;

        let mut updates = String::new();
        let mut add = |name: String, commit: &str| {
            updates.push_str(&format!("create {name} {commit}\n"));
        };
        if let Some(h) = &head_commit {
            add(format!("{prefix}head"), h);
        }
        for b in &branches {
            add(format!("{prefix}heads/{}", b.name), &b.commit);
        }
        for t in &tags {
            // Annotated tags point at tag objects; back up the object itself.
            add(format!("{prefix}tags/{}", t.name), &t.commit);
        }
        if let Some(c) = &index_commit {
            add(format!("{prefix}index"), c);
        }
        add(format!("{prefix}worktree"), &worktree_commit);
        git(&self.root)
            .args(["update-ref", "--stdin"])
            .stdin(updates)
            .run()?;

        let lfs = self.uses_lfs();
        let mut record = BackupRecord {
            id: id.clone(),
            action: action.to_string(),
            created: now,
            head_branch,
            head_commit,
            branches,
            tags,
            index_commit,
            worktree_commit: Some(worktree_commit),
            untracked,
            conflicted,
            lfs,
            bundle: None,
        };

        if let Some(dir) = &opts.bundle_dir {
            match self.write_bundle(&id, dir) {
                Ok(path) => record.bundle = Some(path),
                Err(e) => {
                    self.delete_refs(&id)?;
                    return Err(e);
                }
            }
        }

        index.version = INDEX_VERSION;
        index.backups.push(record.clone());
        if let Err(e) = self.write_index(&index) {
            self.delete_refs(&id)?;
            return Err(e);
        }
        Ok(record)
    }

    fn rev_opt(&self, rev: &str) -> Result<Option<String>, BackupError> {
        let o = git(&self.root)
            .args(["rev-parse", "-q", "--verify", &format!("{rev}^{{commit}}")])
            .output()?;
        Ok(o.success().then(|| o.stdout_str()))
    }

    /// (short name, object id) of every ref under `prefix`.
    fn tips(&self, prefix: &str) -> Result<Vec<RefTip>, BackupError> {
        let out = git(&self.root)
            .args(["for-each-ref", "--format=%(objectname) %(refname)", prefix])
            .run()?
            .stdout_str();
        Ok(out
            .lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(id, name)| RefTip {
                name: name.strip_prefix(prefix).unwrap_or(name).to_string(),
                commit: id.to_string(),
            })
            .collect())
    }

    fn has_refs(&self, id: &str) -> Result<bool, BackupError> {
        Ok(!self.tips(&format!("{REF_PREFIX}{id}/"))?.is_empty())
    }

    fn commit_tree(
        &self,
        tree: &str,
        parent: Option<&str>,
        message: &str,
    ) -> Result<String, BackupError> {
        let mut c = backup_identity(git(&self.root)).args([
            "-c",
            "commit.gpgsign=false",
            "commit-tree",
            tree,
            "-m",
            message,
        ]);
        if let Some(p) = parent {
            c = c.args(["-p", p]);
        }
        Ok(c.run()?.stdout_str())
    }

    /// Tree id of the working tree with untracked (not ignored) files, and the untracked paths.
    fn snapshot_worktree(&self) -> Result<(String, Vec<String>), BackupError> {
        let tmp_dir = self.git_dir.join("pm");
        std::fs::create_dir_all(&tmp_dir).map_err(|e| BackupError::io(&tmp_dir, e))?;
        let tmp_index = tmp_dir.join(format!("snapshot-index-{}", std::process::id()));
        let real_index = PathBuf::from(
            git(&self.root)
                .args(["rev-parse", "--path-format=absolute", "--git-path", "index"])
                .run()?
                .stdout_str(),
        );
        let result = (|| {
            if real_index.exists() {
                std::fs::copy(&real_index, &tmp_index)
                    .map_err(|e| BackupError::io(&tmp_index, e))?;
            }
            let untracked = git(&self.root)
                .args(["ls-files", "-z", "--others", "--exclude-standard"])
                .run()?;
            let untracked = split_z(&untracked.stdout);
            git_exact(&self.root)
                .env("GIT_INDEX_FILE", &tmp_index)
                .args(["add", "-A", "--", "."])
                .run()?;
            let tree = git(&self.root)
                .env("GIT_INDEX_FILE", &tmp_index)
                .arg("write-tree")
                .run()?
                .stdout_str();
            Ok((tree, untracked))
        })();
        let _ = std::fs::remove_file(&tmp_index);
        result
    }

    fn uses_lfs(&self) -> bool {
        self.git_dir.join("lfs").is_dir()
            || std::fs::read_to_string(self.root.join(".gitattributes"))
                .is_ok_and(|a| a.contains("filter=lfs"))
    }

    fn write_bundle(&self, id: &str, dir: &Path) -> Result<PathBuf, BackupError> {
        std::fs::create_dir_all(dir).map_err(|e| BackupError::io(dir, e))?;
        let path = dir.join(format!("{id}.bundle"));
        let refs: Vec<String> = self
            .tips(&format!("{REF_PREFIX}{id}/"))?
            .into_iter()
            .map(|t| format!("{REF_PREFIX}{id}/{}", t.name))
            .collect();
        git(&self.root)
            .args(["bundle", "create", "-q"])
            .arg(&path)
            .args(&refs)
            .run()?;
        git(&self.root)
            .args(["bundle", "verify", "-q"])
            .arg(&path)
            .run()?;
        Ok(path)
    }

    /// Bring a backup's refs back from its bundle (after they were deleted from the repo).
    pub fn import_bundle(&self, id: &str) -> Result<(), BackupError> {
        let rec = self.get(id)?;
        let bundle = rec
            .bundle
            .ok_or_else(|| BackupError::NoBundle(id.to_string()))?;
        let spec = format!("+{REF_PREFIX}{id}/*:{REF_PREFIX}{id}/*");
        git(&self.root)
            .args(["fetch", "-q", "--no-tags", "--no-write-fetch-head"])
            .arg(&bundle)
            .arg(spec)
            .run()?;
        Ok(())
    }

    fn delete_refs(&self, id: &str) -> Result<(), BackupError> {
        let prefix = format!("{REF_PREFIX}{id}/");
        let mut stdin = String::new();
        for t in self.tips(&prefix)? {
            stdin.push_str(&format!("delete {prefix}{}\n", t.name));
        }
        if !stdin.is_empty() {
            git(&self.root)
                .args(["update-ref", "--stdin"])
                .stdin(stdin)
                .run()?;
        }
        Ok(())
    }

    /// Delete one backup: its refs, bundle file and index entry.
    pub fn delete(&self, id: &str) -> Result<(), BackupError> {
        let mut index = self.read_index()?;
        let Some(pos) = index.backups.iter().position(|b| b.id == id) else {
            return Err(BackupError::NotFound(id.to_string()));
        };
        self.delete_refs(id)?;
        let rec = index.backups.remove(pos);
        if let Some(b) = &rec.bundle {
            let _ = std::fs::remove_file(b);
        }
        self.write_index(&index)
    }

    /// Apply the retention policy at time `now` (unix seconds). Returns the deleted ids.
    pub fn prune(&self, policy: Retention, now: u64) -> Result<Vec<String>, BackupError> {
        let list = self.list()?; // newest first
        let mut doomed = Vec::new();
        for (i, b) in list.iter().enumerate() {
            let too_many = policy.max_count.is_some_and(|m| i >= m as usize);
            let too_old = policy
                .max_age_days
                .is_some_and(|d| now.saturating_sub(b.created) > u64::from(d) * 86_400);
            if too_many || too_old {
                doomed.push(b.id.clone());
            }
        }
        for id in &doomed {
            self.delete(id)?;
        }
        Ok(doomed)
    }

    /// Return the repository to the state recorded in backup `id`: branch and tag tips, HEAD,
    /// working tree (including files that were untracked) and index. A `restore` backup of the
    /// current state is taken first; if that fails nothing is changed.
    ///
    /// Branches and tags created after the backup are kept. Ignored files are left alone.
    pub fn restore(&self, id: &str, opts: &BackupOptions) -> Result<RestoreOutcome, BackupError> {
        let rec = self.get(id)?;
        let worktree = rec.worktree_commit.clone();
        // All objects must still exist before we change anything.
        for c in rec
            .branches
            .iter()
            .chain(&rec.tags)
            .map(|t| t.commit.as_str())
            .chain(rec.head_commit.as_deref())
            .chain(worktree.as_deref())
            .chain(rec.index_commit.as_deref())
        {
            let ok = git(&self.root)
                .args(["cat-file", "-e", c])
                .output()?
                .success();
            if !ok {
                return Err(BackupError::Git(GitError::Read {
                    path: self.root.clone(),
                    message: format!(
                        "object {c} of backup {id} is missing (import its bundle first)"
                    ),
                }));
            }
        }

        let pre_restore = self.create("restore", opts)?;
        let mut warnings = Vec::new();

        // Leave any operation in progress; the pre-restore backup holds its working state.
        self.abort_operations(&mut warnings)?;

        // Branch and tag tips.
        let mut updates = String::new();
        for b in &rec.branches {
            updates.push_str(&format!("update refs/heads/{} {}\n", b.name, b.commit));
        }
        for t in &rec.tags {
            updates.push_str(&format!("update refs/tags/{} {}\n", t.name, t.commit));
        }
        if !updates.is_empty() {
            git(&self.root)
                .args(["update-ref", "--stdin"])
                .stdin(updates)
                .run()?;
        }

        // HEAD, then a clean working tree at it.
        match (&rec.head_branch, &rec.head_commit) {
            (Some(branch), _) => {
                git(&self.root)
                    .args(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])
                    .run()?;
            }
            (None, Some(commit)) => {
                git(&self.root)
                    .args(["update-ref", "--no-deref", "HEAD", commit])
                    .run()?;
            }
            (None, None) => {}
        }
        if rec.head_commit.is_some() {
            git(&self.root)
                .args(["reset", "-q", "--hard", "HEAD"])
                .run()?;
        } else {
            git(&self.root).args(["read-tree", "--empty"]).run()?;
        }
        git(&self.root).args(["clean", "-q", "-f", "-d"]).run()?;

        // Working tree files from the snapshot.
        if let Some(wt) = &worktree {
            let tmp_index = self
                .git_dir
                .join("pm")
                .join(format!("restore-index-{}", std::process::id()));
            let result = (|| -> Result<(), BackupError> {
                git(&self.root)
                    .env("GIT_INDEX_FILE", &tmp_index)
                    .args(["read-tree", &format!("{wt}^{{tree}}")])
                    .run()?;
                git_exact(&self.root)
                    .env("GIT_INDEX_FILE", &tmp_index)
                    .args(["checkout-index", "-a", "-f", "-q"])
                    .run()?;
                Ok(())
            })();
            let _ = std::fs::remove_file(&tmp_index);
            result?;
            // Files in HEAD that were deleted in the working tree at backup time.
            if rec.head_commit.is_some() {
                let gone = git(&self.root)
                    .args([
                        "diff-tree",
                        "-r",
                        "-z",
                        "--name-only",
                        "--no-renames",
                        "--diff-filter=D",
                        "HEAD",
                        wt,
                    ])
                    .run()?;
                for rel in split_z(&gone.stdout) {
                    let p = safe_join(&self.root, &rel)?;
                    match std::fs::remove_file(&p) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(BackupError::io(&p, e)),
                    }
                }
            }
        }

        // The index as it was (files that were untracked stay untracked).
        match &rec.index_commit {
            Some(c) => {
                git(&self.root)
                    .args(["read-tree", &format!("{c}^{{tree}}")])
                    .run()?;
            }
            None => {
                if rec.head_commit.is_some() {
                    git(&self.root).args(["read-tree", "HEAD"]).run()?;
                }
                warnings.push(
                    "The index had unresolved conflicts when this backup was taken. The files are \
                     restored, but the conflict itself is not; stage the resolved files again."
                        .into(),
                );
            }
        }
        let _ = git(&self.root)
            .args(["update-index", "-q", "--refresh"])
            .output()?;

        Ok(RestoreOutcome {
            pre_restore,
            warnings,
        })
    }

    fn abort_operations(&self, warnings: &mut Vec<String>) -> Result<(), BackupError> {
        let gd = &self.git_dir;
        let steps: &[(&str, &[&str], &str)] = &[
            ("rebase-merge", &["rebase", "--abort"], "rebase"),
            ("rebase-apply", &["rebase", "--abort"], "rebase or am"),
            (
                "sequencer",
                &["cherry-pick", "--quit"],
                "cherry-pick or revert sequence",
            ),
        ];
        for (marker, args, what) in steps {
            if gd.join(marker).exists() {
                let o = git(&self.root).args(*args).output()?;
                if !o.success() && *marker == "rebase-apply" {
                    git(&self.root).args(["am", "--abort"]).run()?;
                } else if !o.success() {
                    return Err(GitError::Failed {
                        command: o.command,
                        code: o.code,
                        stderr: o.stderr,
                    }
                    .into());
                }
                warnings.push(format!(
                    "A {what} in progress was stopped before restoring."
                ));
            }
        }
        Ok(())
    }
}

/// A git command in `root` that never runs hooks. Backups must not trigger user hooks.
fn git(root: &Path) -> GitCommand {
    GitCommand::new(root).args(["-c", "core.hooksPath=/dev/null"])
}

/// For the commands that copy working-tree bytes into a snapshot and back. `core.autocrlf`
/// (on by default in Git for Windows) would store CRLF files as LF and write LF files back as
/// CRLF, so a restore would not give back the same bytes. Line-ending rules a repository sets in
/// `.gitattributes` still apply, as they would to any checkout.
fn git_exact(root: &Path) -> GitCommand {
    git(root).args(["-c", "core.autocrlf=false"])
}

fn backup_identity(c: GitCommand) -> GitCommand {
    c.env("GIT_AUTHOR_NAME", "Project Manager backup")
        .env("GIT_AUTHOR_EMAIL", "backup@project-manager.invalid")
        .env("GIT_COMMITTER_NAME", "Project Manager backup")
        .env("GIT_COMMITTER_EMAIL", "backup@project-manager.invalid")
}

fn validate_action(action: &str) -> Result<(), BackupError> {
    let ok = !action.is_empty()
        && action.len() <= 40
        && !action.starts_with('-')
        && action
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(BackupError::BadAction(action.to_string()))
    }
}

fn split_z(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Join a repository-relative path onto `root`, refusing anything that could escape it:
/// absolute paths, `..`, and any existing parent folder that is a symlink.
pub(crate) fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, BackupError> {
    let unsafe_path = |reason| BackupError::UnsafePath {
        path: rel.to_string(),
        reason,
    };
    let rel_path = Path::new(rel);
    let mut out = root.to_path_buf();
    let comps: Vec<_> = rel_path.components().collect();
    if comps.is_empty() {
        return Err(unsafe_path("empty path"));
    }
    for (i, c) in comps.iter().enumerate() {
        match c {
            Component::Normal(part) => {
                out.push(part);
                let last = i + 1 == comps.len();
                if !last
                    && std::fs::symlink_metadata(&out).is_ok_and(|m| m.file_type().is_symlink())
                {
                    return Err(unsafe_path("a parent folder is a symlink"));
                }
            }
            _ => return Err(unsafe_path("not a plain relative path")),
        }
    }
    Ok(out)
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `YYYYMMDD-HHMMSS` in UTC.
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian (H. Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
