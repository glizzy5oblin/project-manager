//! Stash: set uncommitted work aside and bring it back.
//!
//! A stash is named by its commit id, never by its position (`stash@{0}`): positions shift when
//! another stash is made or dropped, so a position chosen in the UI could name a different stash
//! by the time the action runs. The position is looked up here, when the commands are planned.

use super::*;
use crate::git::GitVersion;

/// `git stash push --staged` needs this git.
const STAGED_MIN_GIT: GitVersion = GitVersion {
    major: 2,
    minor: 35,
    patch: 0,
};

/// How many stashes the list returns.
const LIST_CAP: usize = 200;

/// One entry of the stash list, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StashEntry {
    /// Position in the list now (0 is the newest). Do not keep it: name the stash by `commit`.
    pub index: u32,
    pub commit: String,
    /// What the user typed, or git's `<short id> <subject>` for a stash made without a message.
    pub message: String,
    /// The branch it was made on; `None` when HEAD was detached.
    pub branch: Option<String>,
    /// Unix seconds.
    #[ts(type = "number")]
    pub created: u64,
    /// Tracked files it changes.
    pub files: u32,
    /// Untracked files it holds.
    pub untracked: u32,
    /// The commit that holds the untracked files (a root commit: `commit_detail` and `file_diff`
    /// show them as added). `None` when the stash has none.
    pub untracked_commit: Option<String>,
}

/// What the reflog subject of a stash says: the branch and the rest. `On main: wip` and
/// `WIP on main: 1a2b3c4 subject`; anything else is all message.
fn split_subject(subject: &str) -> (Option<String>, String) {
    let rest = subject
        .strip_prefix("WIP on ")
        .or_else(|| subject.strip_prefix("On "));
    match rest.and_then(|r| r.split_once(": ")) {
        Some((branch, message)) => (
            (branch != "(no branch)").then(|| branch.to_string()),
            message.to_string(),
        ),
        None => (None, subject.to_string()),
    }
}

/// A full commit id: 40 hex digits (SHA-1) or 64 (SHA-256).
fn check_commit_id(id: &str) -> Result<(), AppError> {
    if matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::invalid(format!("{id:?} is not a stash id")))
    }
}

struct StashFiles {
    tracked: Vec<String>,
    untracked: Vec<String>,
    untracked_commit: Option<String>,
}

struct Listed {
    commit: String,
    subject: String,
    created: u64,
}

impl Repo {
    fn stash_list(&self) -> Result<Vec<Listed>, AppError> {
        let out = self
            .git()
            .args(["stash", "list", "-z", "--format=%H%x1f%gs%x1f%ct"])
            .run()?;
        Ok(out
            .stdout
            .split(|&b| b == 0)
            .filter(|e| !e.is_empty())
            .filter_map(|e| {
                let text = String::from_utf8_lossy(e);
                let mut parts = text.splitn(3, '\u{1f}');
                Some(Listed {
                    commit: parts.next()?.to_string(),
                    subject: parts.next()?.to_string(),
                    created: parts.next()?.trim().parse().ok()?,
                })
            })
            .collect())
    }

    /// The stash with this commit id and its position in the list now.
    fn find_stash(&self, commit: &str) -> Result<(usize, Listed), AppError> {
        check_commit_id(commit)?;
        self.stash_list()?
            .into_iter()
            .enumerate()
            .find(|(_, s)| s.commit == commit)
            .ok_or_else(|| {
                AppError::not_found(
                    "That stash is no longer in the list: it was applied or dropped",
                )
            })
    }

    /// The commit holding a stash's untracked files (its third parent), when it has one.
    fn stash_untracked_commit(&self, commit: &str) -> Result<Option<String>, AppError> {
        let third = self.query([
            "rev-parse",
            "-q",
            "--verify",
            "--end-of-options",
            &format!("{commit}^3"),
        ])?;
        Ok(third.success().then(|| third.stdout_str()))
    }

    /// Tracked files a stash changes, and the untracked files it holds.
    fn stash_files(&self, commit: &str) -> Result<StashFiles, AppError> {
        let tracked = self
            .git()
            .args([
                "diff-tree",
                "-r",
                "-z",
                "--name-only",
                "--no-renames",
                &format!("{commit}^1"),
                commit,
            ])
            .run()?;
        let untracked_commit = self.stash_untracked_commit(commit)?;
        let untracked = match &untracked_commit {
            Some(third) => split_nul(
                &self
                    .git()
                    .args(["ls-tree", "-r", "-z", "--name-only", third])
                    .run()?
                    .stdout,
            ),
            None => Vec::new(),
        };
        Ok(StashFiles {
            tracked: split_nul(&tracked.stdout),
            untracked,
            untracked_commit,
        })
    }

    fn git_version(&self) -> Result<Option<GitVersion>, AppError> {
        Ok(GitVersion::parse(&self.query(["--version"])?.stdout_str()))
    }
}

/// A stash command. Literal pathspecs are switched off for it: `git stash push
/// --include-untracked` builds pathspecs of its own for the files it removes afterwards, and
/// with `GIT_LITERAL_PATHSPECS=1` it stashed them but left them in the working tree (and a later
/// pop then failed with "already exists"). No stash command takes a path from the UI.
fn stash_git(repo: &Repo) -> GitCommand {
    repo.git().env("GIT_LITERAL_PATHSPECS", "0")
}

/// The stash list for the UI, newest first.
pub fn list_stashes(repo: &Repo) -> Result<Vec<StashEntry>, AppError> {
    repo.stash_list()?
        .into_iter()
        .take(LIST_CAP)
        .enumerate()
        .map(|(i, s)| {
            let found = repo.stash_files(&s.commit)?;
            let (branch, message) = split_subject(&s.subject);
            Ok(StashEntry {
                index: i as u32,
                commit: s.commit,
                message,
                branch,
                created: s.created,
                files: found.tracked.len() as u32,
                untracked: found.untracked.len() as u32,
                untracked_commit: found.untracked_commit,
            })
        })
        .collect()
}

/// `"before the merge"`: how a stash is named in titles and impact lines.
fn label(subject: &str) -> String {
    format!("\"{}\"", split_subject(subject).1)
}

pub(super) fn plan_push(
    repo: &Repo,
    message: &Option<String>,
    include_untracked: bool,
    staged_only: bool,
) -> Result<Plan, AppError> {
    repo.require_head("stashing")?;
    repo.require_no_merge("stashing")?;
    let message = match message.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(m) if m.chars().any(char::is_control) => {
            return Err(AppError::invalid(
                "The stash message must be one line without control characters",
            ));
        }
        Some(m) => Some(m),
    };
    if staged_only && include_untracked {
        return Err(AppError::invalid(
            "Stashing only the staged changes cannot also include untracked files",
        ));
    }
    if staged_only && repo.git_version()?.is_some_and(|v| v < STAGED_MIN_GIT) {
        return Err(AppError::invalid(format!(
            "Stashing only the staged changes needs git {STAGED_MIN_GIT} or newer"
        )));
    }
    let status = repo.status()?;
    if status.iter().any(StatusEntry::conflicted) {
        return Err(AppError::invalid(
            "Some files still have conflicts: resolve them before stashing",
        ));
    }
    let staged: Vec<&StatusEntry> = status.iter().filter(|e| e.staged()).collect();
    let tracked = status.iter().filter(|e| !e.untracked()).count();
    let untracked = status.iter().filter(|e| e.untracked()).count();
    let taken = match (staged_only, include_untracked) {
        (true, _) => staged.len(),
        (false, true) => tracked + untracked,
        (false, false) => tracked,
    };
    if taken == 0 {
        return Err(AppError::invalid(if staged_only {
            "Nothing is staged to stash"
        } else if untracked > 0 {
            "Only untracked files have changed: include them to stash them"
        } else {
            "There is nothing to stash"
        }));
    }

    let mut cmd = stash_git(repo).args(["stash", "push", "-q"]);
    if include_untracked {
        cmd = cmd.arg("--include-untracked");
    }
    if staged_only {
        cmd = cmd.arg("--staged");
    }
    if let Some(m) = message {
        cmd = cmd.args(["-m", m]);
    }
    let mut plan = Plan::new(match message {
        Some(m) => format!("Stash \"{m}\""),
        None => "Stash changes".into(),
    });
    plan = plan.impact(format!(
        "Sets aside {}{}",
        files(taken),
        if staged_only {
            " (the staged changes only)"
        } else if include_untracked && untracked > 0 {
            " (untracked files included)"
        } else {
            ""
        }
    ));
    plan = plan.impact(if staged_only {
        "The unstaged changes stay in the working tree"
    } else {
        "The working tree goes back to the last commit"
    });
    if !include_untracked && !staged_only && untracked > 0 {
        plan = plan.impact(format!(
            "{} stay in place",
            if untracked == 1 {
                "1 untracked file".to_string()
            } else {
                format!("{untracked} untracked files")
            }
        ));
    }
    Ok(plan
        .impact("Apply or pop the stash to bring the changes back")
        .step(cmd))
}

pub(super) fn plan_apply(
    repo: &Repo,
    commit: &str,
    restore_index: bool,
    pop: bool,
) -> Result<Plan, AppError> {
    let (n, stash) = repo.find_stash(commit)?;
    repo.require_head("applying a stash")?;
    repo.require_no_merge("applying a stash")?;
    if repo.status()?.iter().any(StatusEntry::conflicted) {
        return Err(AppError::invalid(
            "Some files still have conflicts: resolve them before applying a stash",
        ));
    }
    let StashFiles {
        tracked, untracked, ..
    } = repo.stash_files(commit)?;
    let (made_on, _) = split_subject(&stash.subject);
    let here = repo.head_branch()?;
    let verb = if pop { "Pop" } else { "Apply" };
    let mut plan = Plan::new(format!("{verb} stash {}", label(&stash.subject))).impact(format!(
        "Brings back changes to {}{}",
        files(tracked.len()),
        if untracked.is_empty() {
            String::new()
        } else {
            format!(" and {} untracked", files(untracked.len()))
        }
    ));
    if let (Some(a), Some(b)) = (&made_on, &here)
        && a != b
    {
        plan = plan.impact(format!("The stash was made on {a}; you are on {b}"));
    }
    plan = plan.impact(if restore_index {
        "Staged changes are staged again"
    } else {
        "Everything comes back unstaged (new files stay staged)"
    });
    plan = plan.impact(
        "If a file would be overwritten, git stops without changing anything; if the changes conflict, the conflicts are left to resolve",
    );
    plan = plan.impact(if pop {
        "The stash is removed once it applied cleanly; after a conflict it is kept"
    } else {
        "The stash stays in the list"
    });
    let mut cmd = stash_git(repo).args(["stash", if pop { "pop" } else { "apply" }, "-q"]);
    if restore_index {
        cmd = cmd.arg("--index");
    }
    plan.steps.push(Step {
        cmd: cmd.arg(format!("stash@{{{n}}}")),
        may_conflict: true,
    });
    Ok(plan)
}

pub(super) fn plan_drop(repo: &Repo, commit: &str) -> Result<Plan, AppError> {
    let (n, stash) = repo.find_stash(commit)?;
    let StashFiles {
        tracked, untracked, ..
    } = repo.stash_files(commit)?;
    Ok(Plan::new(format!("Drop stash {}", label(&stash.subject)))
        .impact(format!(
            "Deletes the stash, which holds changes to {}{}",
            files(tracked.len()),
            if untracked.is_empty() {
                String::new()
            } else {
                format!(" and {} untracked", files(untracked.len()))
            }
        ))
        .impact("The backup keeps it: restore the backup to get it back in the list")
        .step(
            repo.git()
                .args(["stash", "drop", "-q", &format!("stash@{{{n}}}")]),
        ))
}

pub(super) fn plan_clear(repo: &Repo) -> Result<Plan, AppError> {
    let all = repo.stash_list()?;
    if all.is_empty() {
        return Err(AppError::invalid("There are no stashes"));
    }
    let plan = Plan::new("Clear all stashes").impact(format!(
        "Deletes {}: {}",
        if all.len() == 1 {
            "the stash".to_string()
        } else {
            format!("all {} stashes", all.len())
        },
        list(
            &all.iter()
                .map(|s| label(&s.subject))
                .collect::<Vec<String>>()
        )
    ));
    Ok(plan
        .impact("The backup keeps them: restore the backup to get them back in the list")
        .step(repo.git().args(["stash", "clear"])))
}
