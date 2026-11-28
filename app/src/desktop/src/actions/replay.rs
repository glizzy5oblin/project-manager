//! Actions that replay commits: cherry-pick, revert and a non-interactive rebase, plus abort and
//! continue for each once it has stopped on a conflict.
//!
//! A cherry-pick or revert of one commit keeps its state in `CHERRY_PICK_HEAD` or `REVERT_HEAD`
//! (git writes no sequencer folder for a single commit), which backups record and restore. A
//! rebase keeps its state in `rebase-merge/`, which backups do not record: restoring a backup
//! stops a rebase in progress first, so undoing "Abort the rebase" brings back the files and
//! branches, not the rebase itself. The abort's impact line says so.

use super::{AppError, Plan, Repo, Step, commits, files, list};

/// An operation git has in progress, read from the git folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InProgress {
    Merge,
    CherryPick,
    Revert,
    Rebase,
    /// `git am`, or a bisect: started outside the app, and left to finish there.
    Other(&'static str),
}

impl InProgress {
    pub(super) fn noun(self) -> &'static str {
        match self {
            InProgress::Merge => "merge",
            InProgress::CherryPick => "cherry-pick",
            InProgress::Revert => "revert",
            InProgress::Rebase => "rebase",
            InProgress::Other(what) => what,
        }
    }

    /// What finishes it, for "commit or abort it before ...".
    fn finish(self) -> &'static str {
        match self {
            InProgress::Merge => "commit or abort it",
            InProgress::CherryPick | InProgress::Revert | InProgress::Rebase => {
                "continue or abort it"
            }
            InProgress::Other(_) => "finish it with git",
        }
    }
}

impl Repo {
    fn git_dir(&self) -> Result<std::path::PathBuf, AppError> {
        Ok(self.text(["rev-parse", "--absolute-git-dir"])?.into())
    }

    /// The operation in progress, if any. A rebase is checked first: while one is stopped,
    /// git may also have other state files.
    pub(super) fn in_progress(&self) -> Result<Option<InProgress>, AppError> {
        let gd = self.git_dir()?;
        if gd.join("rebase-merge").is_dir() {
            return Ok(Some(InProgress::Rebase));
        }
        if gd.join("rebase-apply").is_dir() {
            return Ok(Some(if gd.join("rebase-apply/rebasing").exists() {
                InProgress::Rebase
            } else {
                InProgress::Other("git am")
            }));
        }
        let has = |name: &str| -> Result<bool, AppError> {
            Ok(self
                .query(["rev-parse", "-q", "--verify", &format!("{name}^{{commit}}")])?
                .success())
        };
        if has("MERGE_HEAD")? {
            return Ok(Some(InProgress::Merge));
        }
        if has("CHERRY_PICK_HEAD")? {
            return Ok(Some(InProgress::CherryPick));
        }
        if has("REVERT_HEAD")? {
            return Ok(Some(InProgress::Revert));
        }
        // A cherry-pick or revert of several commits that stopped between them.
        if gd.join("sequencer").is_dir() {
            return Ok(Some(InProgress::Other("cherry-pick or revert sequence")));
        }
        if gd.join("BISECT_LOG").exists() {
            return Ok(Some(InProgress::Other("bisect")));
        }
        Ok(None)
    }

    /// Refuse `what` while any operation is in progress.
    pub(super) fn require_idle(&self, what: &str) -> Result<(), AppError> {
        match self.in_progress()? {
            None => Ok(()),
            Some(op) => {
                let noun = op.noun();
                let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) {
                    "An"
                } else {
                    "A"
                };
                Err(AppError::invalid(format!(
                    "{article} {noun} is in progress: {} before {what}",
                    op.finish()
                )))
            }
        }
    }

    fn parent_count(&self, commit: &str) -> Result<usize, AppError> {
        // `%P` lists the parents separated by spaces; a root commit has none.
        Ok(self
            .text(["log", "-1", "--format=%P", commit, "--"])?
            .split_whitespace()
            .count())
    }

    pub(super) fn is_ancestor(&self, a: &str, b: &str) -> Result<bool, AppError> {
        Ok(self.query(["merge-base", "--is-ancestor", a, b])?.success())
    }

    fn require_no_conflicts(&self) -> Result<(), AppError> {
        let conflicted = self.conflicted_files()?;
        if conflicted.is_empty() {
            Ok(())
        } else {
            Err(AppError::invalid(format!(
                "{} still in conflict: {}. Settle them first",
                if conflicted.len() == 1 {
                    "1 file is".to_string()
                } else {
                    format!("{} are", files(conflicted.len()))
                },
                list(&conflicted)
            )))
        }
    }

    /// Refuse `what` while tracked files have uncommitted changes. Untracked files are fine.
    pub(super) fn require_no_tracked_changes(&self, what: &str) -> Result<(), AppError> {
        let changed: Vec<String> = self
            .status()?
            .into_iter()
            .filter(|e| !e.untracked())
            .map(|e| e.path)
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        Err(AppError::invalid(format!(
            "{} uncommitted changes ({}): commit or stash them before {what}",
            if changed.len() == 1 {
                "1 file has".to_string()
            } else {
                format!("{} have", files(changed.len()))
            },
            list(&changed)
        )))
    }

    pub(super) fn on_branch(&self, what: &str) -> Result<String, AppError> {
        self.head_branch()?.ok_or_else(|| {
            AppError::invalid(format!(
                "HEAD is not on a branch: check out a branch before {what}"
            ))
        })
    }
}

pub(super) fn may_conflict(mut plan: Plan, cmd: crate::git::GitCommand) -> Plan {
    plan.steps.push(Step {
        cmd,
        may_conflict: true,
    });
    plan
}

/// Copy one commit onto the current branch as a new commit.
pub(super) fn plan_cherry_pick(repo: &Repo, rev: &str) -> Result<Plan, AppError> {
    let head = repo.require_head("cherry-picking")?;
    repo.require_idle("cherry-picking")?;
    let commit = repo.resolve_commit(rev)?;
    if repo.is_ancestor(&commit, &head)? {
        return Err(AppError::invalid(format!(
            "{} is already part of the current branch",
            repo.describe(&commit)?
        )));
    }
    let parents = repo.parent_count(&commit)?;
    if parents > 1 {
        return Err(AppError::invalid(
            "That is a merge commit: cherry-pick the commits it brought in instead",
        ));
    }
    if parents == 0 {
        return Err(AppError::invalid(
            "That is the first commit of its history and has nothing to copy a change from",
        ));
    }
    let into = repo
        .head_branch()?
        .unwrap_or_else(|| "the detached HEAD".into());
    let plan = Plan::new(format!("Cherry-pick {} onto {into}", short(&commit)))
        .impact(format!(
            "Adds a copy of {} on top of {into}",
            repo.describe(&commit)?
        ))
        .impact("If it conflicts, it stops so you can settle the files and continue, or abort");
    Ok(may_conflict(
        plan,
        repo.git()
            .args(["cherry-pick", "--no-rerere-autoupdate", &commit]),
    ))
}

/// Add a commit that undoes another one. A merge is undone against its first parent.
pub(super) fn plan_revert(repo: &Repo, rev: &str) -> Result<Plan, AppError> {
    let head = repo.require_head("reverting")?;
    repo.require_idle("reverting")?;
    let commit = repo.resolve_commit(rev)?;
    if !repo.is_ancestor(&commit, &head)? {
        return Err(AppError::invalid(format!(
            "{} is not part of the current branch, so there is nothing to revert here",
            repo.describe(&commit)?
        )));
    }
    let parents = repo.parent_count(&commit)?;
    if parents == 0 {
        return Err(AppError::invalid(
            "That is the first commit: reverting it would remove everything",
        ));
    }
    let into = repo
        .head_branch()?
        .unwrap_or_else(|| "the detached HEAD".into());
    let mut plan = Plan::new(format!("Revert {}", short(&commit))).impact(format!(
        "Adds a commit on {into} that undoes {}",
        repo.describe(&commit)?
    ));
    let mut cmd = repo
        .git()
        .args(["revert", "--no-edit", "--no-rerere-autoupdate"]);
    if parents > 1 {
        plan = plan.impact("It is a merge: the changes it brought in are undone, and the branch stays merged in history");
        cmd = cmd.args(["-m", "1"]);
    }
    plan = plan
        .impact("The original commit stays in history")
        .impact("If it conflicts, it stops so you can settle the files and continue, or abort");
    Ok(may_conflict(plan, cmd.arg(&commit)))
}

/// Replay the current branch's own commits on top of `onto` (a branch or revision).
pub(super) fn plan_rebase(repo: &Repo, onto: &str) -> Result<Plan, AppError> {
    let head = repo.require_head("rebasing")?;
    repo.require_idle("rebasing")?;
    let branch = repo.on_branch("rebasing")?;
    let target = repo.resolve_commit(onto)?;
    repo.require_no_tracked_changes("rebasing")?;
    if repo.is_ancestor(&target, &head)? {
        return Err(AppError::invalid(format!(
            "{branch} already contains {onto}: there is nothing to rebase onto"
        )));
    }
    let own = repo.count(&[&head, "--not", &target])?;
    let merges = repo.count(&["--merges", &head, "--not", &target])?;
    let mut plan = Plan::new(format!("Rebase {branch} onto {onto}"));
    if own == 0 {
        plan = plan.impact(format!(
            "{branch} has no commits of its own: it fast-forwards to {}",
            repo.describe(&target)?
        ));
    } else {
        plan = plan
            .impact(format!(
                "Replays {} of {branch} on top of {}",
                commits(own),
                repo.describe(&target)?
            ))
            .impact("They become new commits with new ids; the old ones are kept in the backup");
        if merges > 0 {
            plan = plan.impact(format!(
                "{} on {branch} are left out: their changes come in through the commits they joined",
                if merges == 1 {
                    "1 merge commit".to_string()
                } else {
                    format!("{merges} merge commits")
                }
            ));
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
                "{branch}'s commits are already on {}: publishing the rebased ones will need a force push",
                list(&pushed)
            ));
        }
        plan = plan.impact(
            "If a commit conflicts, it stops so you can settle the files and continue, or abort",
        );
    }
    Ok(may_conflict(
        plan,
        repo.git()
            .args(["rebase", "--no-autostash", "--no-rerere-autoupdate", "-q"])
            .arg(&target),
    ))
}

/// Stop a cherry-pick, revert or rebase and go back to where it started.
pub(super) fn plan_abort(repo: &Repo, op: InProgress) -> Result<Plan, AppError> {
    let now = repo.in_progress()?;
    if now != Some(op) {
        return Err(AppError::invalid(format!(
            "No {} is in progress",
            op.noun()
        )));
    }
    let noun = op.noun();
    let mut plan = Plan::new(format!("Abort the {noun}"));
    plan = match op {
        InProgress::Rebase => plan
            .impact("Puts the branch, the index and tracked files back to how they were before the rebase")
            .impact("Commits already replayed and conflicts already settled are discarded")
            .impact("The backup keeps your files and the replayed commits, but restoring it does not resume the rebase"),
        _ => plan
            .impact(format!("Puts the index and tracked files back to how they were before the {noun}"))
            .impact("Conflicts already settled are discarded (the backup keeps them)"),
    };
    let sub = match op {
        InProgress::CherryPick => "cherry-pick",
        InProgress::Revert => "revert",
        InProgress::Rebase => "rebase",
        InProgress::Merge | InProgress::Other(_) => {
            return Err(AppError::internal("abort is planned for replays only"));
        }
    };
    Ok(plan.step(repo.git().args([sub, "--abort"])))
}

/// Carry on with a cherry-pick, revert or rebase once every conflict is settled. A rebase can
/// stop again on a later commit.
pub(super) fn plan_continue(repo: &Repo) -> Result<Plan, AppError> {
    let op = repo
        .in_progress()?
        .ok_or_else(|| AppError::invalid("No cherry-pick, revert or rebase is in progress"))?;
    let sub = match op {
        InProgress::CherryPick => "cherry-pick",
        InProgress::Revert => "revert",
        InProgress::Rebase => "rebase",
        InProgress::Merge => {
            return Err(AppError::invalid(
                "A merge is finished by committing it, not by continuing",
            ));
        }
        InProgress::Other(what) => {
            return Err(AppError::invalid(format!(
                "A {what} is in progress: finish it with git"
            )));
        }
    };
    repo.require_no_conflicts()?;
    let plan = Plan::new(format!("Continue the {sub}")).impact(match op {
        InProgress::Rebase => {
            "Commits the settled files and replays the remaining commits; a settled commit with no changes left is dropped"
        }
        _ => "Commits the settled files with the original message",
    });
    Ok(may_conflict(plan, repo.git().args([sub, "--continue"])))
}

pub(super) fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}
