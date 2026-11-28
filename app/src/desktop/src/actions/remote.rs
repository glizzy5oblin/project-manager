//! Actions that talk to a remote: fetch, pull, push, force push with a lease, and pushing a tag.
//!
//! What a preview says about a remote comes from the remote-tracking branches as of the last
//! fetch; the remote itself is only contacted when the action runs. A force push is leased on
//! the remote-tracking commit shown in the preview, so it fails instead of overwriting work
//! that reached the remote since (and the preview token goes stale when that ref moves).
//! Backups keep every remote-tracking commit, so a force push never loses the commits it
//! replaced on the remote: the backup still holds them and they can be pushed back.

use super::{AppError, Plan, Repo, Step, check_branch, commits, files, list};

/// A branch's upstream, as `branch.<name>.remote` and `branch.<name>.merge` give it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Upstream {
    remote: String,
    /// The branch on the remote, without `refs/heads/`.
    branch: String,
}

impl Upstream {
    fn tracking(&self) -> String {
        format!("refs/remotes/{}/{}", self.remote, self.branch)
    }
    fn short(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }
}

impl Repo {
    fn remotes(&self) -> Result<Vec<String>, AppError> {
        Ok(self.text(["remote"])?.lines().map(str::to_string).collect())
    }

    /// `name` checked against the configured remotes.
    fn remote(&self, name: &str) -> Result<String, AppError> {
        if self.remotes()?.iter().any(|r| r == name) {
            Ok(name.to_string())
        } else {
            Err(AppError::not_found(format!("There is no remote {name:?}")))
        }
    }

    /// The upstream of local branch `branch`, if it has one on a remote (not `.`).
    fn upstream(&self, branch: &str) -> Result<Option<Upstream>, AppError> {
        let out = self.text([
            "for-each-ref",
            "--format=%(upstream:remotename)%00%(upstream:remoteref)",
            &format!("refs/heads/{branch}"),
        ])?;
        let mut parts = out.split('\0');
        let (remote, merge) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let Some(rb) = merge.strip_prefix("refs/heads/") else {
            return Ok(None);
        };
        if remote.is_empty() || remote == "." || !self.remotes()?.iter().any(|r| r == remote) {
            return Ok(None);
        }
        Ok(Some(Upstream {
            remote: remote.to_string(),
            branch: rb.to_string(),
        }))
    }

    fn tip(&self, full: &str) -> Result<Option<String>, AppError> {
        let o = self.query(["rev-parse", "-q", "--verify", &format!("{full}^{{commit}}")])?;
        Ok(o.success().then(|| o.stdout_str()))
    }
}

fn may_conflict(mut plan: Plan, cmd: crate::git::GitCommand) -> Plan {
    plan.steps.push(Step {
        cmd,
        may_conflict: true,
    });
    plan
}

/// Download new commits, branches and tags. Remote-tracking branches deleted on the remote are
/// removed (`--prune`); local branches and tags are never touched.
pub(super) fn plan_fetch(repo: &Repo, remote: Option<&str>) -> Result<Plan, AppError> {
    let remotes = repo.remotes()?;
    if remotes.is_empty() {
        return Err(AppError::invalid(
            "This project has no remote to fetch from",
        ));
    }
    let cmd = repo.git().args(["fetch", "--progress", "--prune"]);
    Ok(match remote {
        Some(r) => {
            let r = repo.remote(r)?;
            Plan::new(format!("Fetch from {r}"))
                .impact(format!("Downloads new commits, branches and tags from {r}"))
                .impact(format!(
                    "Branches deleted on {r} are removed from the remote branch list; your own branches are not changed"
                ))
                .step(cmd.arg(&r))
        }
        None => Plan::new("Fetch from all remotes")
            .impact(format!(
                "Downloads new commits, branches and tags from {}",
                list(&remotes)
            ))
            .impact("Branches deleted on a remote are removed from the remote branch list; your own branches are not changed")
            .step(cmd.arg("--all")),
    })
}

/// The current branch and its upstream, for pull and push.
fn branch_and_upstream(
    repo: &Repo,
    what: &str,
) -> Result<(String, String, Option<Upstream>), AppError> {
    let head = repo.require_head(what)?;
    let branch = repo.on_branch(what)?;
    let up = repo.upstream(&branch)?;
    Ok((head, branch, up))
}

/// Fetch the current branch's upstream and merge it in, or with `rebase`, replay the branch's
/// own commits on top of it.
pub(super) fn plan_pull(repo: &Repo, rebase: bool) -> Result<Plan, AppError> {
    let (head, branch, up) = branch_and_upstream(repo, "pulling")?;
    repo.require_idle("pulling")?;
    let Some(up) = up else {
        return Err(AppError::invalid(format!(
            "{branch} has no upstream branch to pull from: push it first, or merge a remote branch"
        )));
    };
    if rebase {
        let changed: Vec<String> = repo
            .status()?
            .into_iter()
            .filter(|e| !e.untracked())
            .map(|e| e.path)
            .collect();
        if !changed.is_empty() {
            return Err(AppError::invalid(format!(
                "{} uncommitted changes ({}): commit or stash them before pulling with rebase",
                files(changed.len()),
                list(&changed)
            )));
        }
    }
    let mut plan = Plan::new(format!(
        "Pull {} into {branch}{}",
        up.short(),
        if rebase { " (rebase)" } else { "" }
    ))
    .impact(format!("Fetches {} from {}", up.branch, up.remote));
    if let Some(theirs) = repo.tip(&up.tracking())? {
        let incoming = repo.count(&[&theirs, "--not", &head])?;
        let own = repo.count(&[&head, "--not", &theirs])?;
        plan = plan.impact(format!(
            "As of the last fetch, {} {} to come in and {branch} has {} of its own",
            commits(incoming),
            if incoming == 1 { "is" } else { "are" },
            commits(own)
        ));
        if rebase && own > 0 {
            plan = plan.impact(format!(
                "{branch}'s own commits are replayed on top and get new ids; the old ones are kept in the backup"
            ));
        }
    }
    plan = plan.impact(if rebase {
        "If a commit conflicts, it stops so you can settle the files and continue, or abort"
    } else {
        "If the changes conflict, it stops so you can settle the files and commit, or abort the merge"
    });
    let cmd = repo.git().args(["pull", "--progress", "--no-edit"]);
    let cmd = if rebase {
        cmd.args(["--rebase", "--no-autostash"])
    } else {
        // Explicit, so `pull.rebase` or `pull.ff=only` in the user's config cannot change what
        // the preview said.
        cmd.args(["--no-rebase", "--ff"])
    };
    Ok(may_conflict(
        plan,
        cmd.args([up.remote.as_str(), &format!("refs/heads/{}", up.branch)]),
    ))
}

/// The remote and remote branch a push of `branch` goes to, and whether it sets the upstream.
fn push_target(
    repo: &Repo,
    branch: &str,
    up: Option<Upstream>,
    remote: Option<&str>,
) -> Result<(Upstream, bool), AppError> {
    match (up, remote) {
        (Some(up), None) => Ok((up, false)),
        (Some(up), Some(r)) if up.remote == r => Ok((up, false)),
        (up, r) => {
            let remote = match r {
                Some(r) => repo.remote(r)?,
                None => match repo.remotes()?.as_slice() {
                    [] => {
                        return Err(AppError::invalid("This project has no remote to push to"));
                    }
                    [one] => one.clone(),
                    _ => {
                        return Err(AppError::invalid(format!(
                            "{branch} has no upstream branch: choose which remote to push it to"
                        )));
                    }
                },
            };
            Ok((
                Upstream {
                    remote,
                    branch: branch.to_string(),
                },
                up.is_none(),
            ))
        }
    }
}

/// Send the current branch to its upstream (or to `remote`, which becomes its upstream when it
/// has none). With `force`, replace the remote branch even when it has commits the local one
/// does not, leased on the remote-tracking commit as of the preview.
pub(super) fn plan_push(repo: &Repo, remote: Option<&str>, force: bool) -> Result<Plan, AppError> {
    let (head, branch, up) = branch_and_upstream(repo, "pushing")?;
    let (to, set_upstream) = push_target(repo, &branch, up, remote)?;
    check_branch(&to.branch)?;
    let theirs = repo.tip(&to.tracking())?;
    let dest = format!("refs/heads/{}", to.branch);
    let refspec = format!("refs/heads/{branch}:{dest}");
    let mut cmd = repo.git().args(["push", "--progress"]);
    let mut plan;
    match (&theirs, force) {
        (None, true) => {
            return Err(AppError::invalid(format!(
                "{} is not on {} yet, so there is nothing to force: use Push",
                to.branch, to.remote
            )));
        }
        (None, false) => {
            let new = repo.count(&[&head, "--not", &format!("--remotes={}", to.remote)])?;
            plan = Plan::new(format!("Push {branch} to {}", to.remote)).impact(format!(
                "Creates {} on {}, at {}{}",
                to.branch,
                to.remote,
                repo.describe(&head)?,
                if new > 0 {
                    format!(", sending {}", commits(new))
                } else {
                    String::new()
                }
            ));
        }
        (Some(theirs), false) => {
            let own = repo.count(&[&head, "--not", theirs])?;
            let missing = repo.count(&[theirs, "--not", &head])?;
            if missing > 0 {
                return Err(AppError::invalid(format!(
                    "{} has {} that {branch} does not: pull first, or force push to replace them",
                    to.short(),
                    commits(missing)
                )));
            }
            if own == 0 {
                return Err(AppError::invalid(format!(
                    "Nothing to push: {} already has every commit of {branch}",
                    to.short()
                )));
            }
            plan = Plan::new(format!("Push {branch} to {}", to.short())).impact(format!(
                "Sends {} to {}",
                commits(own),
                to.short()
            ));
        }
        (Some(theirs), true) => {
            let own = repo.count(&[&head, "--not", theirs])?;
            let replaced = repo.count(&[theirs, "--not", &head])?;
            if replaced == 0 {
                return Err(AppError::invalid(format!(
                    "{branch} already has every commit of {}: a normal Push is enough",
                    to.short()
                )));
            }
            plan = Plan::new(format!("Force push {branch} to {}", to.short()))
                .impact(format!(
                    "Replaces {} on {}: {} that are there and not on {branch} are removed from it",
                    to.branch,
                    to.remote,
                    commits(replaced)
                ))
                .impact(
                    "Anyone who already has those commits will need to fix their copy; the backup keeps them, so they can be pushed back",
                );
            if own > 0 {
                plan = plan.impact(format!("Sends {}", commits(own)));
            }
            if is_main_branch(repo, &to)? {
                plan = plan.impact(format!(
                    "Warning: {} is {}'s main branch, which other people usually build on",
                    to.branch, to.remote
                ));
            }
            plan = plan.impact(format!(
                "Safe against newer work: it stops if {} changed on {} since the last fetch",
                to.branch, to.remote
            ));
            cmd = cmd.arg(format!("--force-with-lease={dest}:{theirs}"));
        }
    }
    if set_upstream {
        plan = plan.impact(format!("{branch} then tracks {}", to.short()));
        cmd = cmd.arg("--set-upstream");
    }
    Ok(plan.step(cmd.args([to.remote.as_str(), &refspec])))
}

/// The remote's default branch (`refs/remotes/<remote>/HEAD`), or `main`/`master` when unknown.
fn is_main_branch(repo: &Repo, to: &Upstream) -> Result<bool, AppError> {
    let o = repo.query([
        "symbolic-ref",
        "-q",
        "--short",
        &format!("refs/remotes/{}/HEAD", to.remote),
    ])?;
    Ok(if o.success() {
        o.stdout_str() == to.short()
    } else {
        to.branch == "main" || to.branch == "master"
    })
}

/// Send one tag to a remote. git refuses when the remote has a different tag of that name.
pub(super) fn plan_push_tag(repo: &Repo, tag: &str, remote: &str) -> Result<Plan, AppError> {
    crate::proj::check_ref_name(tag).map_err(AppError::invalid)?;
    let full = format!("refs/tags/{tag}");
    if !repo.has_ref(&full)? {
        return Err(AppError::not_found(format!("There is no tag {tag:?}")));
    }
    let remote = repo.remote(remote)?;
    let at = repo.resolve_commit(&full)?;
    Ok(Plan::new(format!("Push tag {tag} to {remote}"))
        .impact(format!("Sends {tag}, at {}", repo.describe(&at)?))
        .impact(format!(
            "If {remote} already has a different {tag}, git refuses and nothing changes"
        ))
        .step(
            repo.git()
                .args(["push", "--progress", &remote, &format!("{full}:{full}")]),
        ))
}

/// A plain-words message for a push the remote turned down, from git's report in `detail`.
pub(super) fn rejection(action: &super::Action, detail: &str) -> Option<String> {
    use super::Action;
    if !matches!(action, Action::Push { .. } | Action::PushTag { .. }) {
        return None;
    }
    if detail.contains("(stale info)") {
        Some(
            "The remote branch changed since your last fetch, so the force push stopped and nothing was replaced. Fetch, look at the new commits, then decide again."
                .into(),
        )
    } else if detail.contains("(fetch first)") || detail.contains("(non-fast-forward)") {
        Some("The remote has commits you do not have yet: pull first, then push.".into())
    } else if detail.contains("(already exists)") {
        Some("The remote already has a different tag of that name; nothing was changed.".into())
    } else if detail.contains("protected branch") || detail.contains("pre-receive hook declined") {
        Some(
            "The remote refused the push (the branch may be protected); nothing was changed."
                .into(),
        )
    } else {
        None
    }
}
