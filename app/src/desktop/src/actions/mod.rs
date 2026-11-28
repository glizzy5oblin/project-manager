//! Write actions. Every change Project Manager makes to a repository is an [`Action`], and every
//! action runs through [`run`]:
//!
//! 1. The repository must be trusted (owned by the user, or allowed by `safe.directory`), or the
//!    user must have confirmed after a warning.
//! 2. The action's arguments are validated and resolved: paths must stay inside the working tree
//!    and are passed as literal pathspecs after `--`, branch names follow git's rules, and
//!    revisions are resolved to commit ids before any command uses them.
//! 3. A dangerous action (see [`Action::danger`], the one list of them) needs the token from
//!    [`preview`], which the UI gets while showing the impact in a confirmation dialog. The
//!    token ties the confirmation to the repository as it was shown; if anything changed since,
//!    the action stops and the UI previews again.
//! 4. A dangerous action then takes a backup. If the backup fails, the action does not run.
//! 5. The git commands run, each recorded in the [`OpLog`].

mod hunks;
mod interactive;
mod oplog;
mod remote;
mod replay;
mod stash;
#[cfg(test)]
mod tests;

pub use interactive::{RebaseCommit, RebaseRange, TodoItem, TodoOp, rebase_range};
pub use oplog::{CAPACITY as OP_LOG_CAPACITY, OpEntry, OpKind, OpLog};
pub use stash::{StashEntry, list_stashes};

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::backup::{BackupOptions, BackupRecord, Backups};
use crate::git::{CancelToken, GitCommand, GitOutput, Progress, ensure_trusted};
use crate::ipc::{AppError, ErrorKind};
use crate::proj::check_ref_name;

/// How many names an impact line lists before "and N more".
const LIST_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum ResetMode {
    /// Move the branch only; the index and working tree are kept.
    Soft,
    /// Move the branch and reset the index; the working tree is kept.
    Mixed,
    /// Move the branch and reset the index and tracked files; untracked files are kept.
    Hard,
}

/// How a conflicted file is settled.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Keep {
    /// Your version: the branch you are on, or during a rebase your commit being replayed
    /// (which git itself calls "theirs").
    Ours,
    /// The other side's version: the branch being merged in, the commit being copied or
    /// undone, or during a rebase the branch being rebased onto.
    Theirs,
    /// Remove the file.
    Delete,
    /// These exact contents, written to the file first.
    Edited { content: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// Stage the changes to these paths, deletions included.
    Stage {
        paths: Vec<String>,
    },
    /// Take these paths out of the index again; the working tree is kept.
    Unstage {
        paths: Vec<String>,
    },
    /// Stage some hunks of a file's unstaged diff, named by the ids `work_diff` gave them.
    StageHunks {
        path: String,
        hunks: Vec<String>,
    },
    /// Take some hunks of a file's staged diff out of the index; the working tree is kept.
    UnstageHunks {
        path: String,
        hunks: Vec<String>,
    },
    /// Add a line to the project's `.gitignore` (created if missing).
    Ignore {
        pattern: String,
    },
    /// Settle a conflicted file one way and mark it resolved. A restore point is taken first.
    Resolve {
        path: String,
        keep: Keep,
    },
    /// Throw away unstaged changes to these paths. Untracked ones are deleted.
    Discard {
        paths: Vec<String>,
    },
    /// Commit the staged changes, or with `amend`, replace the last commit with them.
    Commit {
        message: String,
        amend: bool,
    },
    /// `start` is a revision; `None` means HEAD.
    CreateBranch {
        name: String,
        start: Option<String>,
        checkout: bool,
    },
    RenameBranch {
        from: String,
        to: String,
    },
    DeleteBranch {
        name: String,
    },
    /// Switch to a local branch.
    Checkout {
        branch: String,
    },
    /// Merge a local or remote-tracking branch (e.g. `origin/main`) into HEAD.
    Merge {
        branch: String,
    },
    /// Stop a merge that is in progress and go back to where it started.
    AbortMerge,
    /// Move HEAD (and its branch) to `target`, a revision.
    Reset {
        target: String,
        mode: ResetMode,
    },
    /// Copy one commit (a revision) onto the current branch as a new commit.
    CherryPick {
        commit: String,
    },
    /// Add a commit that undoes `commit` (a revision on the current branch).
    Revert {
        commit: String,
    },
    /// Replay the current branch's own commits on top of `onto`, a branch or revision.
    Rebase {
        onto: String,
    },
    /// Rewrite the current branch's commits after `base` (all of them when `None`). `todo`
    /// lists each of them once, oldest first, in the new order, with what happens to it.
    RebaseInteractive {
        base: Option<String>,
        todo: Vec<TodoItem>,
    },
    /// Stop a cherry-pick that stopped on a conflict and go back to where it started.
    AbortCherryPick,
    /// Stop a revert that stopped on a conflict and go back to where it started.
    AbortRevert,
    /// Stop a rebase and put the branch back where it was.
    AbortRebase,
    /// Carry on with a cherry-pick, revert or rebase once its conflicts are settled.
    Continue,
    /// Set uncommitted changes aside in a new stash. `staged_only` stashes just the staged ones.
    StashPush {
        message: Option<String>,
        include_untracked: bool,
        staged_only: bool,
    },
    /// Bring a stash back, named by its commit id; it stays in the list. `index` stages again
    /// what was staged.
    StashApply {
        stash: String,
        index: bool,
    },
    /// Apply a stash and remove it from the list; after a conflict it is kept.
    StashPop {
        stash: String,
        index: bool,
    },
    /// Delete one stash.
    StashDrop {
        stash: String,
    },
    /// Delete every stash.
    StashClear,
    /// Download new commits, branches and tags from `remote`, or from every remote when `None`.
    Fetch {
        remote: Option<String>,
    },
    /// Fetch the current branch's upstream and merge it in, or with `rebase`, replay the
    /// branch's own commits on top of it.
    Pull {
        rebase: bool,
    },
    /// Send the current branch to its upstream, or to `remote` (its new upstream when it has
    /// none). `force` replaces the remote branch, leased on the commit the preview saw there.
    Push {
        remote: Option<String>,
        force: bool,
    },
    /// Send one tag to `remote`.
    PushTag {
        tag: String,
        remote: String,
    },
}

impl Action {
    /// The one list of dangerous actions: those that can lose work or rewrite history. Each
    /// returns the name its backup is recorded under. There is no catch-all arm, so a new action
    /// does not compile until it is classed here.
    pub fn danger(&self) -> Option<&'static str> {
        match self {
            Action::Discard { .. } => Some("discard"),
            Action::Commit { amend: true, .. } => Some("amend"),
            Action::DeleteBranch { .. } => Some("delete-branch"),
            Action::AbortMerge => Some("abort-merge"),
            Action::Rebase { .. } => Some("rebase"),
            Action::RebaseInteractive { .. } => Some("rebase-interactive"),
            Action::AbortCherryPick => Some("abort-cherry-pick"),
            Action::AbortRevert => Some("abort-revert"),
            Action::AbortRebase => Some("abort-rebase"),
            Action::StashDrop { .. } => Some("stash-drop"),
            Action::StashClear => Some("stash-clear"),
            Action::Pull { rebase: true } => Some("pull-rebase"),
            Action::Push { force: true, .. } => Some("force-push"),
            Action::Reset { mode, .. } => Some(match mode {
                ResetMode::Soft => "reset-soft",
                ResetMode::Mixed => "reset-mixed",
                ResetMode::Hard => "reset-hard",
            }),
            Action::Commit { amend: false, .. }
            | Action::Stage { .. }
            | Action::Unstage { .. }
            | Action::StageHunks { .. }
            | Action::UnstageHunks { .. }
            | Action::Resolve { .. }
            | Action::Ignore { .. }
            | Action::CreateBranch { .. }
            | Action::RenameBranch { .. }
            | Action::Checkout { .. }
            | Action::Merge { .. }
            | Action::CherryPick { .. }
            | Action::Revert { .. }
            | Action::Continue
            | Action::StashPush { .. }
            | Action::StashApply { .. }
            | Action::StashPop { .. }
            | Action::Fetch { .. }
            | Action::Pull { rebase: false }
            | Action::Push { force: false, .. }
            | Action::PushTag { .. } => None,
        }
    }

    /// Actions that are safe to run without asking but still leave a restore point, so the
    /// Activity view can undo them. The backup is taken first and the action does not run
    /// without it.
    pub fn auto_backup(&self) -> Option<&'static str> {
        match self {
            Action::StashPush { .. } => Some("stash-push"),
            Action::Resolve { .. } => Some("resolve"),
            _ => None,
        }
    }

    pub fn is_dangerous(&self) -> bool {
        self.danger().is_some()
    }

    /// Machine name, used in the operation log and as the task kind.
    pub fn name(&self) -> &'static str {
        match self {
            Action::Stage { .. } => "stage",
            Action::Unstage { .. } => "unstage",
            Action::StageHunks { .. } => "stage-hunks",
            Action::UnstageHunks { .. } => "unstage-hunks",
            Action::Resolve { .. } => "resolve",
            Action::Ignore { .. } => "ignore",
            Action::Discard { .. } => "discard",
            Action::Commit { amend: false, .. } => "commit",
            Action::Commit { amend: true, .. } => "amend",
            Action::CreateBranch { .. } => "create-branch",
            Action::RenameBranch { .. } => "rename-branch",
            Action::DeleteBranch { .. } => "delete-branch",
            Action::Checkout { .. } => "checkout",
            Action::Merge { .. } => "merge",
            Action::AbortMerge => "abort-merge",
            Action::Reset { .. } => "reset",
            Action::CherryPick { .. } => "cherry-pick",
            Action::Revert { .. } => "revert",
            Action::Rebase { .. } => "rebase",
            Action::RebaseInteractive { .. } => "rebase-interactive",
            Action::AbortCherryPick => "abort-cherry-pick",
            Action::AbortRevert => "abort-revert",
            Action::AbortRebase => "abort-rebase",
            Action::Continue => "continue",
            Action::StashPush { .. } => "stash-push",
            Action::StashApply { .. } => "stash-apply",
            Action::StashPop { .. } => "stash-pop",
            Action::StashDrop { .. } => "stash-drop",
            Action::StashClear => "stash-clear",
            Action::Fetch { .. } => "fetch",
            Action::Pull { rebase: false } => "pull",
            Action::Pull { rebase: true } => "pull-rebase",
            Action::Push { force: false, .. } => "push",
            Action::Push { force: true, .. } => "force-push",
            Action::PushTag { .. } => "push-tag",
        }
    }
}

/// What an action will do, for the UI to show before running it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// e.g. `Reset main to 1a2b3c4 (hard)`.
    pub title: String,
    /// A backup is taken first and the UI must confirm (passing `token` to the run).
    pub dangerous: bool,
    /// What changes, one point per entry. Shown in the confirmation dialog.
    pub impact: Vec<String>,
    /// The git commands that will run, in order.
    pub commands: Vec<String>,
    /// Ties a confirmation to the repository as previewed. A check value, not a secret.
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ActionOutcome {
    pub title: String,
    /// Taken before a dangerous action; restoring it undoes the action.
    pub backup: Option<BackupRecord>,
    /// Files left with conflicts when a merge, cherry-pick, revert or rebase stopped on them.
    /// It is then in progress: settle the files and commit or continue, or abort it.
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// The user confirmed running git in a repository owned by someone else.
    pub allow_untrusted: bool,
    /// Take a backup before dangerous and auto-backup actions.
    pub backups: bool,
    pub backup: BackupOptions,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            allow_untrusted: false,
            backups: true,
            backup: BackupOptions::default(),
        }
    }
}

/// A repository that actions run in.
#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
    /// Extra environment for every git command (tests isolate git from the user's config).
    env: Vec<(OsString, OsString)>,
}

impl Repo {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Repo {
            root: root.into(),
            env: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn with_env(root: impl Into<PathBuf>, env: Vec<(OsString, OsString)>) -> Self {
        Repo {
            root: root.into(),
            env,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn label(&self) -> String {
        self.root.display().to_string()
    }

    fn git(&self) -> GitCommand {
        // Paths are always literal: `*`, `:` and other pathspec magic in a file name mean
        // nothing, so a path can only ever name itself.
        let cmd = GitCommand::new(&self.root).env("GIT_LITERAL_PATHSPECS", "1");
        self.env.iter().fold(cmd, |c, (k, v)| c.env(k, v))
    }

    /// A read-only query, not logged. Returns the output whatever the exit code.
    fn query<I, S>(&self, args: I) -> Result<GitOutput, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.git().args(args).output()?)
    }

    /// A read-only query that must succeed; its trimmed stdout.
    fn text<I, S>(&self, args: I) -> Result<String, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Ok(self.git().args(args).run()?.stdout_str())
    }

    fn has_ref(&self, full: &str) -> Result<bool, AppError> {
        Ok(self
            .query(["show-ref", "--verify", "--quiet", full])?
            .success())
    }

    fn head_commit(&self) -> Result<Option<String>, AppError> {
        let o = self.query(["rev-parse", "-q", "--verify", "HEAD^{commit}"])?;
        Ok(o.success().then(|| o.stdout_str()))
    }

    fn head_branch(&self) -> Result<Option<String>, AppError> {
        let o = self.query(["symbolic-ref", "-q", "--short", "HEAD"])?;
        Ok(o.success().then(|| o.stdout_str()))
    }

    fn merging(&self) -> Result<bool, AppError> {
        Ok(self
            .query(["rev-parse", "-q", "--verify", "MERGE_HEAD"])?
            .success())
    }

    /// `1a2b3c4 subject` for a commit.
    fn describe(&self, commit: &str) -> Result<String, AppError> {
        self.text(["log", "-1", "--format=%h %s", commit, "--"])
    }

    fn count(&self, args: &[&str]) -> Result<u64, AppError> {
        let mut all = vec!["rev-list", "--count"];
        all.extend_from_slice(args);
        self.text(all)?
            .parse()
            .map_err(|_| AppError::internal("unexpected `git rev-list --count` output"))
    }

    fn status(&self) -> Result<Vec<StatusEntry>, AppError> {
        let out = self.git().args(STATUS_ARGS).run()?;
        Ok(parse_status(&out.stdout))
    }

    fn conflicted_files(&self) -> Result<Vec<String>, AppError> {
        let out = self
            .git()
            .args(["diff", "--name-only", "--diff-filter=U", "-z"])
            .run()?;
        Ok(split_nul(&out.stdout))
    }
}

const STATUS_ARGS: [&str; 5] = [
    "--no-optional-locks",
    "status",
    "--porcelain=v1",
    "-z",
    "--untracked-files=all",
];

/// One `git status --porcelain=v1` entry: index state, working tree state, path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusEntry {
    x: u8,
    y: u8,
    path: String,
}

impl StatusEntry {
    fn untracked(&self) -> bool {
        self.x == b'?'
    }
    fn conflicted(&self) -> bool {
        self.x == b'U'
            || self.y == b'U'
            || (self.x == b'A' && self.y == b'A')
            || (self.x == b'D' && self.y == b'D')
    }
    fn staged(&self) -> bool {
        !self.untracked() && !self.conflicted() && self.x != b' '
    }
    fn unstaged(&self) -> bool {
        !self.untracked() && !self.conflicted() && self.y != b' '
    }
}

fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn parse_status(bytes: &[u8]) -> Vec<StatusEntry> {
    let mut out = Vec::new();
    let mut fields = bytes.split(|&b| b == 0).filter(|s| !s.is_empty());
    while let Some(f) = fields.next() {
        if f.len() < 4 {
            continue;
        }
        let (x, y) = (f[0], f[1]);
        out.push(StatusEntry {
            x,
            y,
            path: String::from_utf8_lossy(&f[3..]).into_owned(),
        });
        // Renames and copies are followed by the original path.
        if x == b'R' || x == b'C' {
            fields.next();
        }
    }
    out
}

/// A repository-relative path from the UI: `/` separators, no `..`, `.` or empty parts, not
/// absolute, and nothing inside `.git`.
pub(crate) fn check_path(p: &str) -> Result<(), AppError> {
    let bad = |why: &str| Err(AppError::invalid(format!("{p:?} {why}")));
    if p.is_empty() {
        return Err(AppError::invalid("a path is empty"));
    }
    if p.contains(['\0', '\\']) {
        return bad("contains a backslash or NUL; use / between folders");
    }
    if p.starts_with('/') || (p.len() > 1 && p.as_bytes()[1] == b':') {
        return bad("must be relative to the repository");
    }
    for part in p.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return bad("must not contain empty, \".\" or \"..\" parts");
        }
        // Windows ignores trailing dots and spaces, so `.git.` is `.git` there.
        if part
            .trim_end_matches(['.', ' '])
            .eq_ignore_ascii_case(".git")
        {
            return bad("is inside .git");
        }
    }
    Ok(())
}

fn check_paths(paths: &[String]) -> Result<Vec<String>, AppError> {
    if paths.is_empty() {
        return Err(AppError::invalid("no files were given"));
    }
    let mut seen = BTreeSet::new();
    for p in paths {
        check_path(p)?;
        seen.insert(p.clone());
    }
    Ok(seen.into_iter().collect())
}

fn check_branch(name: &str) -> Result<(), AppError> {
    if name == "HEAD" {
        return Err(AppError::invalid("\"HEAD\" is not a valid branch name"));
    }
    check_ref_name(name).map_err(AppError::invalid)
}

impl Repo {
    fn local_branch(&self, name: &str) -> Result<String, AppError> {
        check_branch(name)?;
        let full = format!("refs/heads/{name}");
        if !self.has_ref(&full)? {
            return Err(AppError::not_found(format!("There is no branch {name:?}")));
        }
        Ok(full)
    }

    /// A revision from the UI (commit id, branch, tag, `HEAD~2`) resolved to a commit id.
    fn resolve_commit(&self, rev: &str) -> Result<String, AppError> {
        if rev.is_empty() || rev.starts_with('-') || rev.contains(|c: char| c.is_control()) {
            return Err(AppError::invalid(format!("{rev:?} is not a revision")));
        }
        let o = self.query([
            "rev-parse",
            "-q",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ])?;
        if o.success() {
            Ok(o.stdout_str())
        } else {
            Err(AppError::not_found(format!(
                "{rev:?} does not name a commit"
            )))
        }
    }

    fn require_head(&self, what: &str) -> Result<String, AppError> {
        self.head_commit()?.ok_or_else(|| {
            AppError::invalid(format!(
                "There are no commits yet, so {what} is not possible"
            ))
        })
    }

    /// Refuse `what` while a merge, cherry-pick, revert, rebase or other operation is in
    /// progress.
    fn require_no_merge(&self, what: &str) -> Result<(), AppError> {
        self.require_idle(what)
    }
}

fn list(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(LIST_LIMIT).map(String::as_str).collect();
    let more = names.len().saturating_sub(LIST_LIMIT);
    if more == 0 {
        shown.join(", ")
    } else {
        format!("{} and {more} more", shown.join(", "))
    }
}

fn files(n: usize) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
    }
}

fn commits(n: u64) -> String {
    if n == 1 {
        "1 commit".into()
    } else {
        format!("{n} commits")
    }
}

/// One git command of a plan.
struct Step {
    cmd: GitCommand,
    /// A failure that leaves conflicted files is an outcome (the merge stopped), not an error.
    may_conflict: bool,
}

struct Plan {
    title: String,
    impact: Vec<String>,
    steps: Vec<Step>,
    /// Files written (path relative to the repository, contents) before the steps run.
    writes: Vec<(String, String)>,
}

impl Plan {
    fn new(title: impl Into<String>) -> Self {
        Plan {
            title: title.into(),
            impact: Vec::new(),
            steps: Vec::new(),
            writes: Vec::new(),
        }
    }
    fn write(mut self, path: &str, content: &str) -> Self {
        self.writes.push((path.to_string(), content.to_string()));
        self
    }
    fn step(mut self, cmd: GitCommand) -> Self {
        self.steps.push(Step {
            cmd,
            may_conflict: false,
        });
        self
    }
    fn impact(mut self, line: impl Into<String>) -> Self {
        self.impact.push(line.into());
        self
    }
}

/// Append `pattern` to `.gitignore`. A pattern that is already a line of it is refused.
fn plan_ignore(repo: &Repo, pattern: &str) -> Result<Plan, AppError> {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern.contains(['\n', '\r', '\0']) {
        return Err(AppError::invalid(
            "the pattern is empty or has more than one line",
        ));
    }
    if pattern.starts_with('#') || pattern.starts_with('!') {
        return Err(AppError::invalid("the pattern must not start with # or !"));
    }
    let file = repo.root().join(".gitignore");
    let mut text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(AppError::io(&file, &e)),
    };
    if text.lines().any(|l| l.trim() == pattern) {
        return Err(AppError::invalid(format!(
            "{pattern} is already in .gitignore"
        )));
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(pattern);
    text.push('\n');
    Ok(Plan::new(format!("Ignore {pattern}"))
        .impact(format!("Adds {pattern} to .gitignore"))
        .write(".gitignore", &text))
}

/// Settle one conflicted file. Only a file that is in conflict can be settled this way.
fn plan_resolve(repo: &Repo, path: &str, keep: &Keep) -> Result<Plan, AppError> {
    check_path(path)?;
    let out = repo
        .git()
        .args(["ls-files", "-u", "-z", "--", path])
        .run()?
        .into_result()?;
    let mut have = [false; 3];
    for entry in out.stdout.split(|b| *b == 0) {
        // "<mode> <sha> <stage>\t<path>"
        if let Some(stage) = std::str::from_utf8(entry)
            .ok()
            .and_then(|e| e.split('\t').next())
            .and_then(|m| m.split(' ').nth(2))
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| (1..=3).contains(n))
        {
            have[stage - 1] = true;
        }
    }
    if !have.iter().any(|h| *h) {
        return Err(AppError::invalid(format!("{path} is not in conflict")));
    }
    let side = |n: usize, name: &str, flag: &str| -> Result<Plan, AppError> {
        if !have[n] {
            return Err(AppError::invalid(format!(
                "{path} does not exist in {name}'s version: delete it instead"
            )));
        }
        Ok(Plan::new(format!("Keep {name}'s version of {path}"))
            .impact(format!("The file becomes {name}'s version"))
            .step(repo.git().args(["checkout", flag, "--", path]))
            .step(repo.git().args(["add", "--", path])))
    };
    // During a rebase git's "ours" is the branch being rebased onto; turn it round.
    let rebasing = repo.in_progress()? == Some(replay::InProgress::Rebase);
    let (mine, theirs) = if rebasing {
        ((2, "--theirs"), (1, "--ours"))
    } else {
        ((1, "--ours"), (2, "--theirs"))
    };
    match keep {
        Keep::Ours => side(mine.0, "your", mine.1),
        Keep::Theirs => side(theirs.0, "their", theirs.1),
        Keep::Delete => Ok(Plan::new(format!("Delete {path}"))
            .impact("The file is removed")
            .step(repo.git().args(["rm", "-q", "--", path]))),
        Keep::Edited { content } => Ok(Plan::new(format!("Save {path} as resolved"))
            .impact("The file is saved as you edited it")
            .write(path, content)
            .step(repo.git().args(["add", "--", path]))),
    }
}

fn paths_title(verb: &str, paths: &[String]) -> String {
    match paths {
        [one] => format!("{verb} {one}"),
        _ => format!("{verb} {}", files(paths.len())),
    }
}

/// Validate `action` against the repository and work out the exact commands.
fn plan(repo: &Repo, action: &Action) -> Result<Plan, AppError> {
    match action {
        Action::Stage { paths } => {
            let paths = check_paths(paths)?;
            Ok(Plan::new(paths_title("Stage", &paths))
                .step(repo.git().args(["add", "-A", "--"]).args(&paths)))
        }
        Action::Unstage { paths } => {
            let paths = check_paths(paths)?;
            Ok(Plan::new(paths_title("Unstage", &paths))
                .step(repo.git().args(["reset", "-q", "--"]).args(&paths)))
        }
        Action::StageHunks { path, hunks } => {
            hunks::plan_hunks(repo, path, hunks, crate::history::diff::WorkSide::Unstaged)
        }
        Action::UnstageHunks { path, hunks } => {
            hunks::plan_hunks(repo, path, hunks, crate::history::diff::WorkSide::Staged)
        }
        Action::Resolve { path, keep } => plan_resolve(repo, path, keep),
        Action::Ignore { pattern } => plan_ignore(repo, pattern),
        Action::Discard { paths } => plan_discard(repo, &check_paths(paths)?),
        Action::Commit { message, amend } => plan_commit(repo, message, *amend),
        Action::CreateBranch {
            name,
            start,
            checkout,
        } => {
            check_branch(name)?;
            if repo.has_ref(&format!("refs/heads/{name}"))? {
                return Err(AppError::new(
                    ErrorKind::AlreadyExists,
                    format!("A branch named {name:?} already exists"),
                ));
            }
            let at = match start {
                Some(rev) => repo.resolve_commit(rev)?,
                None => repo.require_head("creating a branch")?,
            };
            let plan = Plan::new(format!("Create branch {name}"))
                .impact(format!("Starts at {}", repo.describe(&at)?));
            Ok(if *checkout {
                plan.impact(format!("Switches to {name}"))
                    .step(repo.git().args(["checkout", "-q", "-b", name, &at]))
            } else {
                plan.step(repo.git().args(["branch", name, &at]))
            })
        }
        Action::RenameBranch { from, to } => {
            repo.local_branch(from)?;
            check_branch(to)?;
            if repo.has_ref(&format!("refs/heads/{to}"))? {
                return Err(AppError::new(
                    ErrorKind::AlreadyExists,
                    format!("A branch named {to:?} already exists"),
                ));
            }
            Ok(Plan::new(format!("Rename branch {from} to {to}"))
                .step(repo.git().args(["branch", "-m", from, to])))
        }
        Action::DeleteBranch { name } => {
            let full = repo.local_branch(name)?;
            if repo.head_branch()?.as_deref() == Some(name) {
                return Err(AppError::invalid(format!(
                    "{name} is the current branch: switch to another branch before deleting it"
                )));
            }
            let tip = repo.resolve_commit(&full)?;
            // With --branches, git matches the pattern against the name without refs/heads/.
            let exclude = format!("--exclude={name}");
            let only_here = repo.count(&[
                &full,
                "--not",
                &exclude,
                "--branches",
                "--tags",
                "--remotes",
            ])?;
            let mut plan = Plan::new(format!("Delete branch {name}"))
                .impact(format!("Deletes {name}, at {}", repo.describe(&tip)?));
            if only_here > 0 {
                plan = plan.impact(format!(
                    "{} only on this branch will no longer be on any branch or tag (the backup keeps them)",
                    commits(only_here)
                ));
            }
            Ok(plan.step(repo.git().args(["branch", "-D", "-q", name])))
        }
        Action::Checkout { branch } => {
            repo.local_branch(branch)?;
            if repo.head_branch()?.as_deref() == Some(branch) {
                return Err(AppError::invalid(format!("Already on {branch}")));
            }
            let mut plan = Plan::new(format!("Switch to {branch}"));
            if repo.status()?.iter().any(|e| !e.untracked()) {
                plan = plan.impact(
                    "Uncommitted changes are carried over; git stops without changing anything if they would be overwritten",
                );
            }
            Ok(plan.step(repo.git().args(["switch", "-q", "--no-guess", branch])))
        }
        Action::Merge { branch } => plan_merge(repo, branch),
        Action::AbortMerge => {
            if !repo.merging()? {
                return Err(AppError::invalid("No merge is in progress"));
            }
            Ok(Plan::new("Abort the merge")
                .impact("Puts the branch, the index and tracked files back to how they were before the merge")
                .impact("Changes made since the merge started, resolved conflicts included, are discarded (the backup keeps them)")
                .step(repo.git().args(["merge", "--abort"])))
        }
        Action::Reset { target, mode } => plan_reset(repo, target, *mode),
        Action::CherryPick { commit } => replay::plan_cherry_pick(repo, commit),
        Action::Revert { commit } => replay::plan_revert(repo, commit),
        Action::Rebase { onto } => replay::plan_rebase(repo, onto),
        Action::RebaseInteractive { base, todo } => {
            interactive::plan_rebase_interactive(repo, base.as_deref(), todo)
        }
        Action::AbortCherryPick => replay::plan_abort(repo, replay::InProgress::CherryPick),
        Action::AbortRevert => replay::plan_abort(repo, replay::InProgress::Revert),
        Action::AbortRebase => replay::plan_abort(repo, replay::InProgress::Rebase),
        Action::Continue => replay::plan_continue(repo),
        Action::StashPush {
            message,
            include_untracked,
            staged_only,
        } => stash::plan_push(repo, message, *include_untracked, *staged_only),
        Action::StashApply { stash, index } => stash::plan_apply(repo, stash, *index, false),
        Action::StashPop { stash, index } => stash::plan_apply(repo, stash, *index, true),
        Action::StashDrop { stash } => stash::plan_drop(repo, stash),
        Action::StashClear => stash::plan_clear(repo),
        Action::Fetch { remote } => remote::plan_fetch(repo, remote.as_deref()),
        Action::Pull { rebase } => remote::plan_pull(repo, *rebase),
        Action::Push { remote, force } => remote::plan_push(repo, remote.as_deref(), *force),
        Action::PushTag { tag, remote } => remote::plan_push_tag(repo, tag, remote),
    }
}

fn plan_discard(repo: &Repo, paths: &[String]) -> Result<Plan, AppError> {
    let status = repo.status()?;
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    for p in paths {
        match status.iter().find(|e| &e.path == p) {
            None => {
                return Err(AppError::invalid(format!("{p} has no changes to discard")));
            }
            Some(e) if e.conflicted() => {
                return Err(AppError::invalid(format!(
                    "{p} has a conflict: resolve it, or abort the operation, instead"
                )));
            }
            Some(e) if e.untracked() => untracked.push(p.clone()),
            Some(e) if e.unstaged() => tracked.push(p.clone()),
            Some(_) => {
                return Err(AppError::invalid(format!(
                    "{p} has only staged changes: unstage them first"
                )));
            }
        }
    }
    let mut plan = Plan::new(paths_title("Discard changes to", paths));
    if !tracked.is_empty() {
        plan = plan
            .impact(format!(
                "Throws away unstaged changes to {}: {}",
                files(tracked.len()),
                list(&tracked)
            ))
            .step(repo.git().args(["checkout", "-q", "--"]).args(&tracked));
    }
    if !untracked.is_empty() {
        plan = plan
            .impact(format!(
                "Deletes untracked {}: {}",
                files(untracked.len()),
                list(&untracked)
            ))
            .step(
                repo.git()
                    .args(["clean", "-f", "-q", "--"])
                    .args(&untracked),
            );
    }
    Ok(plan.impact("Staged changes are kept"))
}

fn plan_commit(repo: &Repo, message: &str, amend: bool) -> Result<Plan, AppError> {
    if message.trim().is_empty() {
        return Err(AppError::invalid("The commit message is empty"));
    }
    if message.contains('\0') {
        return Err(AppError::invalid(
            "The commit message contains a NUL character",
        ));
    }
    let status = repo.status()?;
    if status.iter().any(StatusEntry::conflicted) {
        return Err(AppError::invalid(
            "Some files still have conflicts: resolve and stage them first",
        ));
    }
    let staged = status.iter().filter(|e| e.staged()).count();
    let cmd = repo
        .git()
        .args(["commit", "-q", "--cleanup=whitespace", "-F", "-"])
        .args(amend.then_some("--amend"))
        .stdin(message.as_bytes().to_vec());
    if !amend {
        if staged == 0 && !repo.merging()? {
            return Err(AppError::invalid("Nothing is staged to commit"));
        }
        let plan = Plan::new("Commit");
        return Ok(if staged > 0 {
            plan.impact(format!("Commits staged changes to {}", files(staged)))
        } else {
            plan.impact("Concludes the merge")
        }
        .step(cmd));
    }
    let head = repo.require_head("amending")?;
    repo.require_no_merge("amending")?;
    let mut plan = Plan::new("Amend the last commit").impact(format!(
        "Replaces commit {} with a new commit",
        repo.describe(&head)?
    ));
    if staged > 0 {
        plan = plan.impact(format!("Adds staged changes to {}", files(staged)));
    }
    let pushed = repo.text([
        "for-each-ref",
        "--format=%(refname:short)",
        "--contains",
        &head,
        "refs/remotes",
    ])?;
    let pushed: Vec<String> = pushed.lines().map(str::to_string).collect();
    if !pushed.is_empty() {
        plan = plan.impact(format!(
            "The commit is already on {}: publishing the new one will need a force push",
            list(&pushed)
        ));
    }
    Ok(plan.step(cmd))
}

fn plan_merge(repo: &Repo, branch: &str) -> Result<Plan, AppError> {
    check_branch(branch)?;
    let local = format!("refs/heads/{branch}");
    let remote = format!("refs/remotes/{branch}");
    let full = if repo.has_ref(&local)? {
        local
    } else if repo.has_ref(&remote)? {
        remote
    } else {
        return Err(AppError::not_found(format!(
            "There is no branch {branch:?}"
        )));
    };
    let head = repo.require_head("merging")?;
    repo.require_no_merge("starting another one")?;
    if repo.head_branch()?.as_deref() == Some(branch) {
        return Err(AppError::invalid(format!("{branch} is the current branch")));
    }
    let tip = repo.resolve_commit(&full)?;
    // The short name gives git's usual merge message ("Merge branch 'x'"); the full name is
    // used when the short one could mean something else, such as a tag.
    let arg = if repo.resolve_commit(branch).ok().as_deref() == Some(&tip)
        && !repo.has_ref(&format!("refs/tags/{branch}"))?
    {
        branch.to_string()
    } else {
        full
    };
    let incoming = repo.count(&[&tip, "--not", &head])?;
    let into = repo
        .head_branch()?
        .unwrap_or_else(|| "the detached HEAD".into());
    let mut plan = Plan::new(format!("Merge {branch} into {into}"));
    if incoming == 0 {
        plan = plan.impact(format!("{into} already contains {branch}; nothing changes"));
    } else if repo
        .query(["merge-base", "--is-ancestor", &head, &tip])?
        .success()
    {
        plan = plan.impact(format!(
            "Fast-forwards {into} by {} to {}",
            commits(incoming),
            repo.describe(&tip)?
        ));
    } else {
        plan = plan
            .impact(format!(
                "Creates a merge commit bringing in {}",
                commits(incoming)
            ))
            .impact("If there are conflicts the merge stops so you can resolve or abort it");
    }
    plan.steps.push(Step {
        cmd: repo.git().args(["merge", "--no-edit", "-q", &arg]),
        may_conflict: true,
    });
    Ok(plan)
}

fn plan_reset(repo: &Repo, target: &str, mode: ResetMode) -> Result<Plan, AppError> {
    let head = repo.require_head("resetting")?;
    let to = repo.resolve_commit(target)?;
    let branch = repo.head_branch()?;
    let what = branch.clone().unwrap_or_else(|| "HEAD".into());
    let (flag, mode_name) = match mode {
        ResetMode::Soft => ("--soft", "soft"),
        ResetMode::Mixed => ("--mixed", "mixed"),
        ResetMode::Hard => ("--hard", "hard"),
    };
    let short = &to[..to.len().min(7)];
    let mut plan = Plan::new(format!("Reset {what} to {short} ({mode_name})"));
    if head == to {
        plan = plan.impact(format!("{what} stays at {}", repo.describe(&head)?));
    } else {
        plan = plan.impact(format!(
            "Moves {what} from {} to {}",
            repo.describe(&head)?,
            repo.describe(&to)?
        ));
    }
    let leaving = repo.count(&[&head, "--not", &to])?;
    if leaving > 0 {
        // With --branches, git matches the pattern against the name without refs/heads/.
        let exclude = branch.as_ref().map(|b| format!("--exclude={b}"));
        let mut args = vec![head.as_str(), "--not", to.as_str()];
        args.extend(exclude.as_deref());
        args.extend(["--branches", "--tags", "--remotes"]);
        let lost = repo.count(&args)?;
        plan = plan.impact(format!("{} will no longer be on {what}", commits(leaving)));
        if lost > 0 {
            plan = plan.impact(format!(
                "{} of them will be on no branch or tag (the backup keeps them)",
                commits(lost)
            ));
        }
    }
    let status = repo.status()?;
    plan = match mode {
        ResetMode::Soft => plan.impact("The index and working tree are kept"),
        ResetMode::Mixed => plan.impact("Staged changes become unstaged; the working tree is kept"),
        ResetMode::Hard => {
            let changed = status.iter().filter(|e| !e.untracked()).count();
            if changed > 0 {
                plan = plan.impact(format!(
                    "Throws away uncommitted changes to {} (the backup keeps them)",
                    files(changed)
                ));
            }
            plan.impact("Untracked files are kept")
        }
    };
    Ok(plan.step(repo.git().args(["reset", "-q", flag, &to, "--"])))
}

/// A check value for the repository as an action sees it: the action itself, HEAD, every
/// branch, tag and remote-tracking ref, any merge in progress, and the status of every file.
fn fingerprint(repo: &Repo, action: &Action) -> Result<String, AppError> {
    let mut h = DefaultHasher::new();
    action.hash(&mut h);
    for args in [
        &["symbolic-ref", "-q", "HEAD"][..],
        &["rev-parse", "-q", "--verify", "HEAD"],
        &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
        &["rev-parse", "-q", "--verify", "CHERRY_PICK_HEAD"],
        &["rev-parse", "-q", "--verify", "REVERT_HEAD"],
        &["rev-parse", "-q", "--verify", "REBASE_HEAD"],
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/heads",
            "refs/tags",
            "refs/remotes",
        ],
        &["stash", "list", "--format=%H"],
        &STATUS_ARGS,
    ] {
        let o = repo.query(args)?;
        o.code.hash(&mut h);
        o.stdout.hash(&mut h);
    }
    Ok(format!("{:016x}", h.finish()))
}

/// Check and describe `action` without changing anything.
pub fn preview(repo: &Repo, action: &Action, allow_untrusted: bool) -> Result<Preview, AppError> {
    ensure_trusted(repo.root(), allow_untrusted)?;
    let plan = plan(repo, action)?;
    Ok(Preview {
        title: plan.title,
        dangerous: action.is_dangerous(),
        impact: plan.impact,
        commands: plan.steps.iter().map(|s| s.cmd.display()).collect(),
        token: fingerprint(repo, action)?,
    })
}

/// Run `action`. A dangerous action needs `token` from a [`preview`] of the same action on the
/// unchanged repository, and runs only once its backup has been taken.
pub fn run(
    repo: &Repo,
    action: &Action,
    token: Option<&str>,
    opts: &RunOptions,
    log: &OpLog,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<ActionOutcome, AppError> {
    ensure_trusted(repo.root(), opts.allow_untrusted)?;
    let plan = plan(repo, action)?;
    let name = action.name();
    let backup = match action.danger().or_else(|| action.auto_backup()) {
        None => None,
        Some(backup_action) => {
            if action.is_dangerous() {
                match token {
                    None => {
                        return Err(AppError::invalid(format!(
                            "\"{}\" needs confirmation: preview it first",
                            plan.title
                        )));
                    }
                    Some(t) if t != fingerprint(repo, action)? => {
                        return Err(AppError::new(
                            ErrorKind::Stale,
                            "The repository changed since this was confirmed. Review it again.",
                        ));
                    }
                    Some(_) => {}
                }
            }
            if cancel.is_cancelled() {
                return Err(AppError::new(ErrorKind::Cancelled, "Cancelled"));
            }
            if !opts.backups {
                // Confirmation and cancellation above are kept: they gate the run,
                // not the backup.
                None
            } else {
                let started = Instant::now();
                let result =
                    Backups::open(repo.root()).and_then(|b| b.create(backup_action, &opts.backup));
                match result {
                    Ok(record) => {
                        log.backup(&repo.label(), name, Ok(&record.id), started.elapsed());
                        Some(record)
                    }
                    Err(e) => {
                        log.backup(&repo.label(), name, Err(e.to_string()), started.elapsed());
                        let mut err = AppError::from(e);
                        err.message = format!(
                            "No backup could be taken, so \"{}\" did not run: {}",
                            plan.title, err.message
                        );
                        return Err(err);
                    }
                }
            }
        }
    };

    for (path, content) in &plan.writes {
        if let Err(e) = std::fs::write(repo.root().join(path), content) {
            let err = AppError::new(ErrorKind::Io, format!("Could not write {path}: {e}"));
            return Err(after_backup(err, &backup));
        }
    }
    let mut conflicts = Vec::new();
    for step in plan.steps {
        let out = match step.cmd.output_with(cancel, &mut on_progress) {
            Ok(out) => out,
            Err(e) => {
                log.git_error(&repo.label(), name, step.cmd.display(), &e);
                return Err(after_backup(e.into(), &backup));
            }
        };
        log.git(&repo.label(), name, &out);
        if !out.success() {
            if step.may_conflict {
                conflicts = repo.conflicted_files()?;
                if !conflicts.is_empty() {
                    break;
                }
            }
            let mut err: AppError = out.into_result().expect_err("the command failed").into();
            if let Some(m) = err
                .detail
                .as_deref()
                .and_then(|d| remote::rejection(action, d))
            {
                err.message = m;
            }
            return Err(after_backup(err, &backup));
        }
    }
    Ok(ActionOutcome {
        title: plan.title,
        backup,
        conflicts,
    })
}

/// Point a failure after the backup at that backup, since a step may have changed things.
fn after_backup(mut err: AppError, backup: &Option<BackupRecord>) -> AppError {
    if let Some(b) = backup {
        let note = format!(
            "A backup was taken first ({}); restore it to undo anything this changed.",
            b.id
        );
        err.detail = Some(match err.detail.take() {
            Some(d) => format!("{d}\n\n{note}"),
            None => note,
        });
    }
    err
}
