//! Interactive rebase: rewrite the current branch's recent commits in one go. They can be put in
//! a new order, folded into the commit before them (squash keeps the folded commit's message,
//! fixup leaves it out), left out, or given a new message.
//!
//! `git rebase -i` asks an editor for its list of steps (the "todo"). Here the list is built up
//! front and stored as a blob, and the editor git runs is `git cat-file blob <id> >`, which
//! writes the list over git's own: no file is written and nothing waits for a person.
//!
//! git's own message steps (`reword`, `squash`) also want an editor, and a message typed for one
//! would be lost if its commit stopped on a conflict and was continued later. So a commit whose
//! message changes is replaced by a copy carrying the new message (same tree, parent, author and
//! committer), written as an object before the rebase starts, and the list picks the copy. A
//! folded group is a pick of its first commit, or of that copy with the group's message, followed
//! by `fixup` steps.
//!
//! When a commit stops on a conflict, git tidies its message as the rebase continues, removing
//! lines that start with `#`, as it does for every rebase.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::replay::{may_conflict, short};
use super::{AppError, Plan, Repo, commits, list};

/// The most commits one interactive rebase edits.
const MAX_COMMITS: u64 = 500;

/// What happens to one commit in an interactive rebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum TodoOp {
    /// Keep the commit.
    Pick,
    /// Fold the commit into the kept commit before it, adding its message to that commit's.
    Squash,
    /// Fold the commit into the kept commit before it, leaving its message out.
    Fixup,
    /// Leave the commit out.
    Drop,
}

/// One commit of an interactive rebase, in its place in the new order (oldest first).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    /// The full id of one of the commits being edited.
    pub commit: String,
    pub op: TodoOp,
    /// Only on a pick: the commit's new message, which is also the whole message of a group
    /// folded into it. `None` keeps its message, with the messages of squashed commits added.
    pub message: Option<String>,
}

/// A commit an interactive rebase can edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RebaseCommit {
    pub id: String,
    pub short_id: String,
    pub subject: String,
    /// The whole message.
    pub message: String,
    pub author: String,
    /// Already on a remote-tracking branch: publishing a rewritten copy needs a force push.
    pub pushed: bool,
}

/// The commits an interactive rebase would edit, for the editor to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RebaseRange {
    /// The branch being edited.
    pub branch: String,
    /// The commit they sit on, to pass to the action; `None` when the first is the root.
    pub base: Option<String>,
    /// Oldest first.
    pub commits: Vec<RebaseCommit>,
}

/// The commits of the current branch from `from` (a revision) to HEAD, for an interactive
/// rebase. Refused for the same reasons the rebase itself would be.
pub fn rebase_range(repo: &Repo, from: &str) -> Result<RebaseRange, AppError> {
    let first = repo.resolve_commit(from)?;
    let head = repo.require_head("editing commits")?;
    if !repo.is_ancestor(&first, &head)? {
        return Err(AppError::invalid(format!(
            "{} is not part of the current branch",
            repo.describe(&first)?
        )));
    }
    let parents = repo
        .text(["log", "-1", "--format=%P", &first, "--"])?
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let base = match parents.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        _ => {
            return Err(AppError::invalid(
                "That is a merge commit: editing history across a merge is not possible here. Choose a commit after it",
            ));
        }
    };
    let range = Range::read(repo, base.as_deref())?;
    Ok(RebaseRange {
        branch: range.branch,
        base,
        commits: range
            .commits
            .into_iter()
            .map(|c| RebaseCommit {
                id: c.id,
                short_id: c.short,
                subject: c.subject,
                message: c.message,
                author: c.author,
                pushed: c.pushed,
            })
            .collect(),
    })
}

/// One commit of the range.
struct Listed {
    id: String,
    short: String,
    subject: String,
    message: String,
    author: String,
    pushed: bool,
}

/// The current branch's commits after a base, checked for an interactive rebase.
struct Range {
    branch: String,
    /// The resolved base; `None` for all of the branch's commits.
    base: Option<String>,
    /// Oldest first.
    commits: Vec<Listed>,
}

impl Range {
    fn read(repo: &Repo, base: Option<&str>) -> Result<Range, AppError> {
        let what = "editing commits";
        let head = repo.require_head(what)?;
        repo.require_idle(what)?;
        let branch = repo.on_branch(what)?;
        repo.require_no_tracked_changes(what)?;
        let base = base.map(|b| repo.resolve_commit(b)).transpose()?;
        let mut range = vec![head.as_str()];
        if let Some(b) = &base {
            if *b == head {
                return Err(AppError::invalid(format!(
                    "{branch} has no commits after {} to edit",
                    short(b)
                )));
            }
            if !repo.is_ancestor(b, &head)? {
                return Err(AppError::invalid(format!(
                    "{} is not part of {branch}",
                    repo.describe(b)?
                )));
            }
            range.extend(["--not", b.as_str()]);
        }
        let n = repo.count(&range)?;
        if n > MAX_COMMITS {
            return Err(AppError::invalid(format!(
                "That is {n} commits; up to {MAX_COMMITS} can be edited at once. Start from a later commit"
            )));
        }
        let mut merges = vec!["rev-list", "--merges", "-n", "1"];
        merges.extend(&range);
        let merge = repo.text(&merges)?;
        if !merge.is_empty() {
            return Err(AppError::invalid(format!(
                "{} is a merge commit: editing history across a merge is not possible here. Start from a commit after it",
                repo.describe(&merge)?
            )));
        }
        let mut unpushed_args = vec!["rev-list"];
        unpushed_args.extend(&range);
        if base.is_none() {
            unpushed_args.push("--not");
        }
        unpushed_args.push("--remotes");
        let unpushed: HashSet<String> = repo
            .text(&unpushed_args)?
            .lines()
            .map(str::to_string)
            .collect();
        // Fields split by \x1f; commits ended by NUL (`-z`). The message is last, so a \x1f
        // inside it stays in it.
        let mut log = vec![
            "log",
            "--reverse",
            "-z",
            "--no-show-signature",
            "--format=%H%x1f%h%x1f%an%x1f%s%x1f%B",
        ];
        log.extend(&range);
        log.push("--");
        let out = repo.git().args(&log).run()?;
        let mut listed = Vec::new();
        for record in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let record = String::from_utf8_lossy(record);
            let mut f = record.splitn(5, '\x1f');
            let mut next = || f.next().unwrap_or_default().to_string();
            let (id, short, author, subject, message) = (next(), next(), next(), next(), next());
            listed.push(Listed {
                pushed: !unpushed.contains(&id),
                id,
                short,
                subject,
                message,
                author,
            });
        }
        Ok(Range {
            branch,
            base,
            commits: listed,
        })
    }
}

/// A message tidied as `git commit --cleanup=whitespace` would: trailing whitespace and blank
/// lines at the start and end removed, runs of blank lines made one, and a final newline.
fn clean_message(m: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in m.lines().map(str::trim_end) {
        if line.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push('\n');
            blank = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The first line of a message, safe to put on a todo line.
fn todo_subject(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(100)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// A copy of `commit` with another message: the same tree, parent, author and committer. Other
/// headers are left out: a signature would no longer match, and the message is UTF-8.
fn copy_with_message(repo: &Repo, commit: &str, message: &str) -> Result<Vec<u8>, AppError> {
    let raw = repo
        .git()
        .args(["cat-file", "commit", commit])
        .run()?
        .stdout;
    let end = raw
        .windows(2)
        .position(|w| w == b"\n\n")
        .unwrap_or(raw.len());
    let mut obj = Vec::new();
    for line in raw[..end].split(|b| *b == b'\n') {
        let kept: [&[u8]; 4] = [b"tree ", b"parent ", b"author ", b"committer "];
        if kept.iter().any(|k| line.starts_with(k)) {
            obj.extend_from_slice(line);
            obj.push(b'\n');
        }
    }
    obj.push(b'\n');
    obj.extend_from_slice(message.as_bytes());
    Ok(obj)
}

/// The id `bytes` get as an object of `kind`, without writing it.
fn object_id(repo: &Repo, kind: &str, bytes: &[u8]) -> Result<String, AppError> {
    Ok(repo
        .git()
        .args(["hash-object", "--literally", "-t", kind, "--stdin"])
        .stdin(bytes.to_vec())
        .run()?
        .stdout_str())
}

/// A step that writes `bytes` as an object of `kind`.
fn write_object(repo: &Repo, kind: &str, bytes: Vec<u8>) -> crate::git::GitCommand {
    repo.git()
        .args(["hash-object", "--literally", "-w", "-t", kind, "--stdin"])
        .stdin(bytes)
}

fn check_message(m: &str, commit: &str) -> Result<(), AppError> {
    if m.trim().is_empty() {
        return Err(AppError::invalid(format!(
            "The new message for {} is empty",
            short(commit)
        )));
    }
    if m.contains('\0') {
        return Err(AppError::invalid(format!(
            "The new message for {} contains a NUL character",
            short(commit)
        )));
    }
    Ok(())
}

/// Rewrite the current branch's commits after `base` (all of them when `None`) as `todo` says.
pub(super) fn plan_rebase_interactive(
    repo: &Repo,
    base: Option<&str>,
    todo: &[TodoItem],
) -> Result<Plan, AppError> {
    let range = Range::read(repo, base)?;
    let branch = &range.branch;
    let by_id: HashMap<&str, &Listed> = range.commits.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut seen = HashSet::new();
    for item in todo {
        if !by_id.contains_key(item.commit.as_str()) {
            return Err(AppError::invalid(format!(
                "{:?} is not one of the commits being edited",
                item.commit
            )));
        }
        if !seen.insert(item.commit.as_str()) {
            return Err(AppError::invalid(format!(
                "{} is listed twice",
                short(&item.commit)
            )));
        }
        if let Some(m) = &item.message {
            if item.op != TodoOp::Pick {
                return Err(AppError::invalid(format!(
                    "{} is squashed, fixed up or dropped, so it cannot get a new message",
                    short(&item.commit)
                )));
            }
            check_message(m, &item.commit)?;
        }
    }
    if let Some(missing) = range.commits.iter().find(|c| !seen.contains(c.id.as_str())) {
        return Err(AppError::invalid(format!(
            "{} {} is missing: list every commit, marking the ones to leave out as dropped",
            missing.short, missing.subject
        )));
    }
    let kept: Vec<&TodoItem> = todo.iter().filter(|i| i.op != TodoOp::Drop).collect();
    match kept.first().map(|i| i.op) {
        None => {
            return Err(AppError::invalid(match base {
                Some(_) => format!(
                    "That drops every commit: to do that, reset {branch} to the commit before them instead"
                ),
                None => format!("That drops every commit, and {branch} cannot be left empty"),
            }));
        }
        Some(TodoOp::Squash | TodoOp::Fixup) => {
            return Err(AppError::invalid(format!(
                "{} cannot be squashed or fixed up: there is no kept commit before it to fold it into",
                short(&kept[0].commit)
            )));
        }
        Some(_) => {}
    }

    // The todo, the copies to write first, and what to say about it.
    let mut lines = Vec::new();
    let mut copies = Vec::new();
    let mut reworded = Vec::new();
    let mut folded = Vec::new();
    let mut dropped = Vec::new();
    // The first place the new list differs from the old one. Every commit from there on, in
    // either order, is rewritten or left out; those before it keep their ids.
    let mut first_change: Option<usize> = None;
    for (pos, item) in todo.iter().enumerate() {
        let c = by_id[item.commit.as_str()];
        let mut changed = range.commits[pos].id != c.id;
        match item.op {
            TodoOp::Drop => {
                changed = true;
                dropped.push(format!("{} {}", c.short, c.subject));
                lines.push(format!("drop {} {}", c.id, todo_subject(&c.message)));
            }
            TodoOp::Squash | TodoOp::Fixup => {
                changed = true;
                folded.push(c.short.clone());
                lines.push(format!("fixup {} {}", c.id, todo_subject(&c.message)));
            }
            TodoOp::Pick => {
                // The commits folded into this one: the next ones up to the next kept pick.
                let group: Vec<&TodoItem> = todo[pos + 1..]
                    .iter()
                    .filter(|i| i.op != TodoOp::Drop)
                    .take_while(|i| i.op != TodoOp::Pick)
                    .collect();
                changed |= !group.is_empty();
                let squashed: Vec<&str> = group
                    .iter()
                    .filter(|i| i.op == TodoOp::Squash)
                    .map(|i| by_id[i.commit.as_str()].message.as_str())
                    .collect();
                let own = clean_message(&c.message);
                let message = match &item.message {
                    Some(m) => Some(clean_message(m)),
                    None if !squashed.is_empty() => {
                        let mut all = vec![c.message.as_str()];
                        all.extend(&squashed);
                        Some(clean_message(&all.join("\n\n")))
                    }
                    None => None,
                }
                // An unchanged message needs no copy, unless it replaces a combined one.
                .filter(|m| *m != own || !squashed.is_empty());
                match message {
                    Some(m) => {
                        if item.message.is_some() && m != own {
                            reworded.push(format!("{} {}", c.short, c.subject));
                        }
                        let obj = copy_with_message(repo, &c.id, &m)?;
                        let id = object_id(repo, "commit", &obj)?;
                        changed = true;
                        lines.push(format!("pick {id} {}", todo_subject(&m)));
                        copies.push(obj);
                    }
                    None => lines.push(format!("pick {} {}", c.id, todo_subject(&c.message))),
                }
            }
        }
        if changed && first_change.is_none() {
            first_change = Some(pos);
        }
    }
    let Some(first_change) = first_change else {
        return Err(AppError::invalid(
            "Nothing would change: reorder, squash, drop or reword a commit first",
        ));
    };

    let mut plan = Plan::new(format!(
        "Edit {} of {branch}",
        commits(range.commits.len() as u64)
    ));
    let kept_order: Vec<&str> = kept.iter().map(|i| i.commit.as_str()).collect();
    let old_order: Vec<&str> = range
        .commits
        .iter()
        .map(|c| c.id.as_str())
        .filter(|id| kept_order.contains(id))
        .collect();
    if kept_order != old_order {
        plan = plan.impact("Puts the commits in the new order");
    }
    if !reworded.is_empty() {
        plan = plan.impact(format!(
            "Gives {} a new message: {}",
            commits(reworded.len() as u64),
            list(&reworded)
        ));
    }
    if !folded.is_empty() {
        plan = plan.impact(format!(
            "Folds {} into the kept commit before {}: {}",
            commits(folded.len() as u64),
            if folded.len() == 1 { "it" } else { "each" },
            list(&folded)
        ));
    }
    if !dropped.is_empty() {
        plan = plan.impact(format!(
            "Leaves out {}: {} (the backup keeps {})",
            commits(dropped.len() as u64),
            list(&dropped),
            if dropped.len() == 1 { "it" } else { "them" }
        ));
    }
    // Each pick from the first change on is a new commit.
    let renewed = todo[first_change..]
        .iter()
        .filter(|i| i.op == TodoOp::Pick)
        .count();
    if renewed > 0 {
        plan = plan.impact(format!(
            "{} {} new ids; the backup keeps the old ones",
            commits(renewed as u64),
            if renewed == 1 { "gets" } else { "get" }
        ));
    }
    // The oldest commit rewritten or left out: whatever else contains it keeps the old ones.
    let oldest = &range.commits[first_change].id;
    let containing = |refs: &str| -> Result<Vec<String>, AppError> {
        Ok(repo
            .text([
                "for-each-ref",
                "--format=%(refname:short)",
                "--contains",
                oldest,
                refs,
            ])?
            .lines()
            .filter(|r| r != branch)
            .map(str::to_string)
            .collect())
    };
    let pushed = containing("refs/remotes")?;
    if !pushed.is_empty() {
        plan = plan.impact(format!(
            "Some of these commits are already on {}: publishing the new ones will need a force push",
            list(&pushed)
        ));
    }
    let others = containing("refs/heads")?;
    if !others.is_empty() {
        plan = plan.impact(format!(
            "Other branches with these commits keep the old ones: {}",
            list(&others)
        ));
    }
    plan = plan.impact(
        "If a commit conflicts, it stops so you can settle the files and continue, or abort",
    );

    let mut todo_text = lines.join("\n");
    todo_text.push('\n');
    let todo_id = object_id(repo, "blob", todo_text.as_bytes())?;
    for obj in copies {
        plan = plan.step(write_object(repo, "commit", obj));
    }
    plan = plan.step(write_object(repo, "blob", todo_text.into_bytes()));
    let rebase = repo
        .git()
        .args([
            // Commits are left out with explicit `drop` lines, and replaced ones (copies with a
            // new message) not at all, which git would otherwise stop to ask about.
            "-c",
            "rebase.missingCommitsCheck=ignore",
            "-c",
            "rebase.updateRefs=false",
            "-c",
            "rebase.rebaseMerges=false",
            "rebase",
            "-i",
            "--no-autostash",
            "--no-rerere-autoupdate",
            "--empty=drop",
            "-q",
        ])
        .arg(range.base.as_deref().unwrap_or("--root"))
        // git runs this through its shell with the todo file's path as the argument.
        .env(
            "GIT_SEQUENCE_EDITOR",
            format!("git cat-file blob {todo_id} >"),
        );
    Ok(may_conflict(plan, rebase))
}
