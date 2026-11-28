use std::path::Path;

use super::*;
use crate::backup::Backups;
use crate::history::diff::{Hunk, WorkSide, work_diff};
use crate::testing::FixtureRepo;
use crate::testing::shapes::*;

fn repo(r: &FixtureRepo) -> Repo {
    Repo::with_env(r.path(), r.isolated_env())
}

fn run_logged(
    r: &FixtureRepo,
    action: &Action,
    token: Option<&str>,
    log: &OpLog,
) -> Result<ActionOutcome, AppError> {
    run(
        &repo(r),
        action,
        token,
        &RunOptions::default(),
        log,
        &CancelToken::new(),
        |_| {},
    )
}

/// Preview, then run with the preview's token (what the UI does after the user confirms).
fn confirm_and_run(r: &FixtureRepo, action: &Action) -> Result<ActionOutcome, AppError> {
    let p = preview(&repo(r), action, false)?;
    assert_eq!(p.dangerous, action.is_dangerous());
    run_logged(r, action, Some(&p.token), &OpLog::new())
}

fn ok(r: &FixtureRepo, action: Action) -> ActionOutcome {
    confirm_and_run(r, &action).unwrap_or_else(|e| panic!("{action:?} failed: {e:?}"))
}

fn err(r: &FixtureRepo, action: Action) -> AppError {
    match confirm_and_run(r, &action) {
        Ok(o) => panic!("{action:?} should fail, got {o:?}"),
        Err(e) => e,
    }
}

fn git_out(r: &FixtureRepo, args: &[&str]) -> String {
    r.command(r.path(), args).output().unwrap().stdout_str()
}

/// Everything a restore must bring back: HEAD, branches and tags, the exact index (conflict
/// stages included), status, any merge in progress, and the bytes of every working-tree file.
fn snapshot(r: &FixtureRepo) -> String {
    let mut s = String::new();
    for args in [
        &["symbolic-ref", "-q", "HEAD"][..],
        &["rev-parse", "-q", "--verify", "HEAD"],
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "refs/heads",
            "refs/tags",
        ],
        &["ls-files", "-s"],
        &["status", "--porcelain=v1", "--untracked-files=all"],
        &["stash", "list", "--format=%H %gs"],
    ] {
        s.push_str(&git_out(r, args));
        s.push('\n');
    }
    for f in ["MERGE_HEAD", "MERGE_MSG", "CHERRY_PICK_HEAD", "REVERT_HEAD"] {
        s.push_str(&format!(
            "{f}: {:?}\n",
            std::fs::read_to_string(r.path().join(".git").join(f)).ok()
        ));
    }
    let mut files = Vec::new();
    walk(r.path(), r.path(), &mut files);
    files.sort();
    for (rel, bytes) in files {
        s.push_str(&format!("{rel}: {bytes:?}\n"));
    }
    s
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if p.is_dir() {
            walk(root, &p, out);
        } else {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, std::fs::read(&p).unwrap()));
        }
    }
}

/// Uncommitted work of every kind: staged, unstaged, staged-then-changed, untracked.
fn make_dirty(r: &FixtureRepo) {
    r.write("staged.txt", "staged\n");
    r.git(&["add", "staged.txt"]);
    r.write("file.txt", "unstaged edit\n");
    r.write("untracked/new.txt", "untracked\n");
}

/// Run a dangerous action, check it changed the repository and took a backup, then restore
/// the backup and check the repository is exactly as before.
fn assert_undoable(r: &FixtureRepo, action: Action) -> ActionOutcome {
    assert!(action.is_dangerous());
    let before = snapshot(r);
    let out = ok(r, action.clone());
    let backup = out
        .backup
        .clone()
        .expect("a dangerous action takes a backup");
    assert_eq!(Some(backup.action.as_str()), action.danger());
    assert_ne!(snapshot(r), before, "{action:?} changed nothing");
    Backups::open(r.path())
        .unwrap()
        .restore(&backup.id, &Default::default())
        .unwrap();
    assert_eq!(snapshot(r), before, "restoring after {action:?}");
    out
}

#[test]
fn backups_are_optional_and_confirmation_still_gates_the_run() {
    // With backups off a dangerous action runs with a token but records no backup.
    let r = linear(3);
    make_dirty(&r);
    let action = Action::Discard {
        paths: vec!["file.txt".into()],
    };
    let p = preview(&repo(&r), &action, false).unwrap();
    let out = run(
        &repo(&r),
        &action,
        Some(&p.token),
        &RunOptions {
            backups: false,
            ..Default::default()
        },
        &OpLog::new(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    assert!(out.backup.is_none(), "backups are off: {out:?}");

    // ... and without a token the confirmation gate still refuses the run.
    let r = linear(3);
    make_dirty(&r);
    let action = Action::Discard {
        paths: vec!["file.txt".into()],
    };
    let e = run(
        &repo(&r),
        &action,
        None,
        &RunOptions {
            backups: false,
            ..Default::default()
        },
        &OpLog::new(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidInput);
}

#[test]
fn every_dangerous_action_takes_a_backup_that_undoes_it() {
    // Discard: unstaged edits and an untracked file; staged work stays.
    let r = linear(3);
    make_dirty(&r);
    assert_undoable(
        &r,
        Action::Discard {
            paths: vec!["file.txt".into(), "untracked/new.txt".into()],
        },
    );

    // Amend.
    let r = linear(3);
    make_dirty(&r);
    assert_undoable(
        &r,
        Action::Commit {
            message: "amended".into(),
            amend: true,
        },
    );

    // Delete a branch with a commit found nowhere else.
    let r = linear(2);
    r.checkout_new("side");
    r.commit_file("side.txt", "side\n", "side work");
    r.checkout("main");
    make_dirty(&r);
    let out = assert_undoable(
        &r,
        Action::DeleteBranch {
            name: "side".into(),
        },
    );
    assert_eq!(out.title, "Delete branch side");

    // Reset, each mode, with every kind of uncommitted work present.
    for mode in [ResetMode::Soft, ResetMode::Mixed, ResetMode::Hard] {
        let r = linear(4);
        make_dirty(&r);
        assert_undoable(
            &r,
            Action::Reset {
                target: "HEAD~2".into(),
                mode,
            },
        );
    }

    // Drop one stash of two, and clear them all, with uncommitted work present too.
    let r = linear(2);
    let (first, second) = two_stashes(&r);
    make_dirty(&r);
    let out = assert_undoable(
        &r,
        Action::StashDrop {
            stash: first.clone(),
        },
    );
    assert!(out.title.starts_with("Drop stash"), "{}", out.title);
    assert_undoable(&r, Action::StashDrop { stash: second });
    assert_undoable(&r, Action::StashClear);

    // Abort a merge stopped on a conflict, with one conflict already resolved by hand.
    let r = conflict();
    start_conflict(&r);
    r.write("conflict.txt", "resolved by hand\n");
    r.write("untracked.txt", "x\n");
    assert_undoable(&r, Action::AbortMerge);
    let s = crate::git::state::read_state(r.path(), false).unwrap();
    assert_eq!(s.in_progress, Some(crate::git::state::Operation::Merge));

    // Rebase, finished cleanly and stopped on a conflict, with an untracked file present
    // (rebase refuses tracked changes).
    let r = diverged();
    r.write("untracked.txt", "x\n");
    let out = assert_undoable(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    assert!(out.conflicts.is_empty());
    let r = conflict();
    r.checkout("topic");
    r.write("untracked.txt", "x\n");
    let out = assert_undoable(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    assert_eq!(out.conflicts, vec!["conflict.txt".to_string()]);

    // Interactive rebase, finished cleanly (reorder, squash, drop, reword) and stopped on a
    // conflict, with an untracked file present.
    let r = stack();
    r.write("untracked.txt", "x\n");
    let action = edit(
        &r,
        Some("HEAD~4"),
        vec![
            todo_item(&r, "HEAD", TodoOp::Pick),
            reworded(&r, "HEAD~3", "A"),
            todo_item(&r, "HEAD~2", TodoOp::Squash),
            todo_item(&r, "HEAD~1", TodoOp::Drop),
        ],
    );
    let out = assert_undoable(&r, action);
    assert!(out.conflicts.is_empty());
    let r = clashing();
    r.write("untracked.txt", "x\n");
    let action = edit(
        &r,
        Some("HEAD~3"),
        vec![
            todo_item(&r, "HEAD", TodoOp::Pick),
            todo_item(&r, "HEAD~2", TodoOp::Pick),
            todo_item(&r, "HEAD~1", TodoOp::Pick),
        ],
    );
    let out = assert_undoable(&r, action);
    assert_eq!(out.conflicts, vec!["f.txt".to_string()]);

    // Abort a cherry-pick and a revert stopped on a conflict, one file settled by hand.
    let r = conflict();
    let _ = r.command(r.path(), &["cherry-pick", "topic"]).output();
    r.write("conflict.txt", "resolved by hand\n");
    r.write("untracked.txt", "x\n");
    assert_undoable(&r, Action::AbortCherryPick);
    let s = crate::git::state::read_state(r.path(), false).unwrap();
    assert_eq!(
        s.in_progress,
        Some(crate::git::state::Operation::CherryPick)
    );

    let r = linear(3);
    start_revert_conflict(&r);
    r.write("untracked.txt", "x\n");
    assert_undoable(&r, Action::AbortRevert);
    let s = crate::git::state::read_state(r.path(), false).unwrap();
    assert_eq!(s.in_progress, Some(crate::git::state::Operation::Revert));

    // Pull with rebase: the branch's own commits replayed on the upstream's.
    let (r, _remote) = ahead_behind();
    r.write("untracked.txt", "x\n");
    assert_undoable(&r, Action::Pull { rebase: true });

    // Force push changes nothing local; the backup keeps the commit it replaced on the
    // remote, and restoring leaves the repository as it was.
    let (r, remote) = ahead_behind();
    make_dirty(&r);
    let theirs = git_out(&r, &["rev-parse", "origin/main"]);
    let before = snapshot(&r);
    let out = ok(
        &r,
        Action::Push {
            remote: None,
            force: true,
        },
    );
    let backup = out.backup.expect("a force push takes a backup");
    assert_eq!(backup.action, "force-push");
    assert_eq!(git_out(&remote, &["rev-parse", "main"]), r.head());
    assert_eq!(
        git_out(
            &r,
            &[
                "rev-parse",
                &format!("refs/pm/backups/{}/remotes/origin/main", backup.id)
            ]
        ),
        theirs
    );
    assert!(backup.remotes.iter().any(|t| t.commit == theirs));
    Backups::open(r.path())
        .unwrap()
        .restore(&backup.id, &Default::default())
        .unwrap();
    assert_eq!(snapshot(&r), before);
    // Remote-tracking branches follow the remote, so a restore leaves them alone.
    assert_eq!(git_out(&r, &["rev-parse", "origin/main"]), r.head());
}

/// `main` and `side` each with a commit of their own touching different files.
fn diverged() -> FixtureRepo {
    let r = linear(2);
    r.checkout_new("side");
    r.commit_file("side.txt", "side\n", "side work");
    r.commit_file("side2.txt", "side 2\n", "more side work");
    r.checkout("main");
    r.commit_file("main.txt", "main\n", "main work");
    r.checkout("side");
    r
}

/// In a [`linear`] repo of 3 or more commits, revert `HEAD~1`, which conflicts with HEAD.
fn start_revert_conflict(r: &FixtureRepo) {
    let out = r
        .command(r.path(), &["revert", "--no-edit", "HEAD~1"])
        .output()
        .unwrap();
    assert!(!out.success(), "revert was expected to conflict");
}

/// A rebase of `topic` onto `main` in a [`conflict`] repo, stopped on its conflict.
fn start_rebase_conflict(r: &FixtureRepo) {
    r.checkout("topic");
    let out = r.command(r.path(), &["rebase", "main"]).output().unwrap();
    assert!(!out.success(), "rebase was expected to conflict");
}

#[test]
fn cherry_pick_revert_and_rebase_run_clean_and_stop_on_conflicts() {
    // A clean cherry-pick adds one commit with the same message.
    let r = diverged();
    r.checkout("main");
    let p = preview(
        &repo(&r),
        &Action::CherryPick {
            commit: "side".into(),
        },
        false,
    )
    .unwrap();
    assert!(!p.dangerous);
    assert!(p.impact[0].contains("more side work"), "{:?}", p.impact);
    let before = r.rev("HEAD");
    let out = ok(
        &r,
        Action::CherryPick {
            commit: "side".into(),
        },
    );
    assert!(out.conflicts.is_empty());
    assert_eq!(r.rev("HEAD~1"), before);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]), "more side work");
    // Picking it again, or anything already on the branch, is refused.
    let e = err(
        &r,
        Action::CherryPick {
            commit: "main~1".into(),
        },
    );
    assert!(e.message.contains("already part"), "{e:?}");

    // A clean revert adds a commit that undoes it.
    let out = ok(
        &r,
        Action::Revert {
            commit: "HEAD".into(),
        },
    );
    assert!(out.conflicts.is_empty());
    assert!(!r.path().join("side2.txt").exists());
    assert!(r.git(&["log", "-1", "--format=%s"]).starts_with("Revert"));
    // A commit not on the branch cannot be reverted here.
    r.checkout_new("elsewhere");
    r.commit_file("else.txt", "x\n", "elsewhere");
    r.checkout("main");
    let e = err(
        &r,
        Action::Revert {
            commit: "elsewhere".into(),
        },
    );
    assert!(e.message.contains("not part"), "{e:?}");

    // A conflicting cherry-pick stops; continue is refused until the file is settled, then
    // commits it with the original message.
    let r = conflict();
    let out = ok(
        &r,
        Action::CherryPick {
            commit: "topic".into(),
        },
    );
    assert_eq!(out.conflicts, vec!["conflict.txt".to_string()]);
    let e = err(
        &r,
        Action::Merge {
            branch: "topic".into(),
        },
    );
    assert!(e.message.contains("A cherry-pick is in progress"), "{e:?}");
    let e = err(&r, Action::Continue);
    assert!(e.message.contains("conflict"), "{e:?}");
    let info = crate::git::conflict::describe(r.path(), "conflict.txt").unwrap();
    assert!(!info.swapped);
    assert_eq!(info.during, Some(crate::git::state::Operation::CherryPick));
    assert_eq!(info.ours_name.as_deref(), Some("main"));
    assert!(
        info.theirs_name.as_deref().unwrap().starts_with("commit "),
        "{info:?}"
    );
    assert_eq!(info.regions[0].ours, ["main version"]);
    let e = err(&r, Action::AbortRevert);
    assert!(e.message.contains("No revert"), "{e:?}");
    ok(&r, resolve("conflict.txt", Keep::Theirs));
    ok(&r, Action::Continue);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]), "topic edit");
    assert_eq!(r.parents("HEAD").len(), 1);
    assert_eq!(git_out(&r, &["status", "--porcelain"]), "");
    let e = err(&r, Action::Continue);
    assert!(e.message.contains("No cherry-pick"), "{e:?}");

    // A conflicting revert stops and can be aborted, leaving nothing behind.
    let r = linear(3);
    let head = r.rev("HEAD");
    let out = ok(
        &r,
        Action::Revert {
            commit: "HEAD~1".into(),
        },
    );
    assert_eq!(out.conflicts, vec!["file.txt".to_string()]);
    ok(&r, Action::AbortRevert);
    assert_eq!(r.rev("HEAD"), head);
    assert_eq!(git_out(&r, &["status", "--porcelain"]), "");

    // Reverting a merge undoes what it brought in.
    let r = diverged();
    r.checkout("main");
    r.git(&["merge", "--no-edit", "-q", "side"]);
    let p = preview(
        &repo(&r),
        &Action::Revert {
            commit: "HEAD".into(),
        },
        false,
    )
    .unwrap();
    assert!(
        p.impact.join(" ").contains("It is a merge"),
        "{:?}",
        p.impact
    );
    ok(
        &r,
        Action::Revert {
            commit: "HEAD".into(),
        },
    );
    assert!(!r.path().join("side.txt").exists());
    assert!(r.path().join("main.txt").exists());
    // And a merge commit cannot be cherry-picked.
    r.checkout_new("other");
    r.git(&["reset", "-q", "--hard", "main~2"]);
    let e = err(
        &r,
        Action::CherryPick {
            commit: "main~1".into(),
        },
    );
    assert!(e.message.contains("merge commit"), "{e:?}");
}

#[test]
fn rebase_replays_the_branch_and_handles_conflicts() {
    // Clean: side's two commits land on top of main.
    let r = diverged();
    let p = preview(
        &repo(&r),
        &Action::Rebase {
            onto: "main".into(),
        },
        false,
    )
    .unwrap();
    assert!(p.dangerous);
    let text = p.impact.join(" ");
    assert!(text.contains("Replays 2 commits of side"), "{text}");
    let out = ok(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    assert!(out.backup.is_some());
    assert_eq!(r.rev("side~2"), r.rev("main"));
    assert_eq!(r.git(&["symbolic-ref", "--short", "HEAD"]), "side");
    // Rebasing again is refused: side already contains main.
    let e = err(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    assert!(e.message.contains("already contains"), "{e:?}");

    // Tracked changes are refused before anything runs.
    let r = diverged();
    r.write("file.txt", "edited\n");
    let e = err(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    assert!(e.message.contains("commit or stash"), "{e:?}");
    // So is a detached HEAD.
    let r = diverged();
    r.git(&["checkout", "-q", "--detach"]);
    let e = err(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    assert!(e.message.contains("not on a branch"), "{e:?}");

    // A conflict stops it; settle and continue finishes it on the branch.
    let r = conflict();
    r.checkout("topic");
    let out = ok(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    assert_eq!(out.conflicts, vec!["conflict.txt".to_string()]);
    let s = crate::git::state::read_state(r.path(), false).unwrap();
    assert_eq!(s.in_progress, Some(crate::git::state::Operation::Rebase));
    let e = err(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    assert!(e.message.contains("A rebase is in progress"), "{e:?}");
    // During a rebase "mine" is topic's commit being replayed, though git calls it theirs.
    let info = crate::git::conflict::describe(r.path(), "conflict.txt").unwrap();
    assert!(info.swapped);
    assert_eq!(info.ours_name.as_deref(), Some("topic"));
    assert_eq!(info.theirs_name.as_deref(), Some("main"));
    assert_eq!(info.regions[0].ours, ["topic version"]);
    assert_eq!(info.regions[0].theirs, ["main version"]);
    ok(&r, resolve("conflict.txt", Keep::Ours));
    ok(&r, Action::Continue);
    assert_eq!(r.git(&["symbolic-ref", "--short", "HEAD"]), "topic");
    assert_eq!(r.rev("topic~1"), r.rev("main"));
    assert_eq!(r.read("conflict.txt"), "topic version\n");

    // Settled to main's version, the commit has nothing left and is dropped.
    let r = conflict();
    r.checkout("topic");
    ok(
        &r,
        Action::Rebase {
            onto: "main".into(),
        },
    );
    ok(&r, resolve("conflict.txt", Keep::Theirs));
    ok(&r, Action::Continue);
    assert_eq!(r.rev("topic"), r.rev("main"));

    // Abort puts the branch back, and its backup keeps the files as they were.
    let r = conflict();
    let tip = r.rev("topic");
    start_rebase_conflict(&r);
    r.write("conflict.txt", "settled by hand\n");
    let out = ok(&r, Action::AbortRebase);
    assert_eq!(r.rev("topic"), tip);
    assert_eq!(r.git(&["symbolic-ref", "--short", "HEAD"]), "topic");
    assert_eq!(git_out(&r, &["status", "--porcelain"]), "");
    let backup = out.backup.expect("abort-rebase takes a backup");
    let restored = Backups::open(r.path())
        .unwrap()
        .restore(&backup.id, &Default::default())
        .unwrap();
    assert!(restored.warnings.is_empty(), "{:?}", restored.warnings);
    assert_eq!(r.read("conflict.txt"), "settled by hand\n");
}

/// `main` with a first commit and four more, each adding its own file, so any order applies
/// cleanly.
fn stack() -> FixtureRepo {
    let r = FixtureRepo::new();
    r.commit_file("base.txt", "base\n", "base");
    for n in ["a", "b", "c", "d"] {
        r.commit_file(&format!("{n}.txt"), &format!("{n}\n"), &format!("add {n}"));
    }
    r
}

/// `main` where `set one` and `set two` change the same line one after the other, so swapping
/// them conflicts, with `add g` (another file) between them.
fn clashing() -> FixtureRepo {
    let r = FixtureRepo::new();
    r.commit_file("f.txt", "zero\n", "base");
    r.commit_file("f.txt", "one\n", "set one");
    r.commit_file("g.txt", "g\n", "add g");
    r.commit_file("f.txt", "two\n", "set two");
    r
}

fn todo_item(r: &FixtureRepo, rev: &str, op: TodoOp) -> TodoItem {
    TodoItem {
        commit: r.rev(rev),
        op,
        message: None,
    }
}

fn reworded(r: &FixtureRepo, rev: &str, message: &str) -> TodoItem {
    TodoItem {
        message: Some(message.into()),
        ..todo_item(r, rev, TodoOp::Pick)
    }
}

fn edit(r: &FixtureRepo, base: Option<&str>, todo: Vec<TodoItem>) -> Action {
    Action::RebaseInteractive {
        base: base.map(|b| r.rev(b)),
        todo,
    }
}

/// The subjects of the current branch, oldest first.
fn subjects(r: &FixtureRepo) -> Vec<String> {
    git_out(r, &["log", "--reverse", "--format=%s"])
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn rebase_range_lists_the_commits_to_edit() {
    let r = stack();
    let range = rebase_range(&repo(&r), "HEAD~3").unwrap();
    assert_eq!(range.branch, "main");
    assert_eq!(range.base, Some(r.rev("HEAD~4")));
    let listed: Vec<&str> = range.commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(listed, ["add a", "add b", "add c", "add d"]);
    let a = &range.commits[0];
    assert_eq!(a.id, r.rev("HEAD~3"));
    assert_eq!(a.short_id, git_out(&r, &["rev-parse", "--short", "HEAD~3"]));
    assert_eq!(a.message, "add a\n");
    assert!(!a.author.is_empty());
    assert!(range.commits.iter().all(|c| !c.pushed));

    // From the first commit: no base, every commit.
    let range = rebase_range(&repo(&r), "HEAD~4").unwrap();
    assert_eq!(range.base, None);
    assert_eq!(range.commits.len(), 5);

    // Commits a remote-tracking branch has are marked; the whole message is given.
    r.git(&["update-ref", "refs/remotes/origin/main", "HEAD~2"]);
    r.commit_file("e.txt", "e\n", "add e\n\nwith a body\n#123 and this");
    let range = rebase_range(&repo(&r), "HEAD~3").unwrap();
    let pushed: Vec<bool> = range.commits.iter().map(|c| c.pushed).collect();
    assert_eq!(pushed, [true, false, false, false]);
    assert_eq!(
        range.commits[3].message,
        "add e\n\nwith a body\n#123 and this\n"
    );

    // Refused: a commit that is not on the branch, a merge, a detached HEAD, uncommitted
    // changes, and an operation in progress.
    let r = diverged();
    let e = rebase_range(&repo(&r), "main").unwrap_err();
    assert!(
        e.message.contains("not part of the current branch"),
        "{e:?}"
    );
    let r = merges();
    r.checkout("main");
    let e = rebase_range(&repo(&r), "HEAD").unwrap_err();
    assert!(e.message.contains("That is a merge commit"), "{e:?}");
    let e = rebase_range(&repo(&r), "HEAD~3").unwrap_err();
    assert!(e.message.contains("is a merge commit"), "{e:?}");
    let r = stack();
    r.git(&["checkout", "-q", "--detach"]);
    let e = rebase_range(&repo(&r), "HEAD~1").unwrap_err();
    assert!(e.message.contains("not on a branch"), "{e:?}");
    let r = stack();
    r.write("a.txt", "edited\n");
    let e = rebase_range(&repo(&r), "HEAD~1").unwrap_err();
    assert!(e.message.contains("commit or stash"), "{e:?}");
    let r = conflict();
    start_rebase_conflict(&r);
    let e = rebase_range(&repo(&r), "HEAD").unwrap_err();
    assert!(e.message.contains("A rebase is in progress"), "{e:?}");
}

#[test]
fn interactive_rebase_reorders_squashes_drops_and_rewords() {
    use TodoOp::*;

    // Reorder: the base keeps its id, the files are all there.
    let r = stack();
    let base = r.rev("HEAD~4");
    let action = edit(
        &r,
        Some("HEAD~4"),
        vec![
            todo_item(&r, "HEAD~2", Pick),
            todo_item(&r, "HEAD~3", Pick),
            todo_item(&r, "HEAD~1", Pick),
            todo_item(&r, "HEAD", Pick),
        ],
    );
    let p = preview(&repo(&r), &action, false).unwrap();
    assert!(p.dangerous);
    assert_eq!(p.title, "Edit 4 commits of main");
    let text = p.impact.join("\n");
    assert!(text.contains("Puts the commits in the new order"), "{text}");
    assert!(text.contains("4 commits get new ids"), "{text}");
    let out = ok(&r, action);
    assert!(out.backup.is_some());
    assert!(out.conflicts.is_empty());
    assert_eq!(subjects(&r), ["base", "add b", "add a", "add c", "add d"]);
    assert_eq!(r.rev("HEAD~4"), base);
    assert_eq!(r.git(&["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(git_out(&r, &["status", "--porcelain"]), "");
    for f in ["a", "b", "c", "d"] {
        assert_eq!(r.read(&format!("{f}.txt")), format!("{f}\n"));
    }

    // Reword one in the middle: its author, date and changes stay; the commits before it keep
    // their ids. Lines starting with # are kept.
    let r = stack();
    let (a, b) = (r.rev("HEAD~3"), r.rev("HEAD~2"));
    let action = edit(
        &r,
        Some("HEAD~4"),
        vec![
            todo_item(&r, "HEAD~3", Pick),
            reworded(&r, "HEAD~2", "Add B  \n\n\nwith a body\n#123 kept\n\n"),
            todo_item(&r, "HEAD~1", Pick),
            todo_item(&r, "HEAD", Pick),
        ],
    );
    let text = preview(&repo(&r), &action, false)
        .unwrap()
        .impact
        .join("\n");
    assert!(text.contains("Gives 1 commit a new message"), "{text}");
    assert!(!text.contains("new order"), "{text}");
    assert!(text.contains("3 commits get new ids"), "{text}");
    ok(&r, action);
    assert_eq!(r.rev("HEAD~3"), a);
    assert_eq!(
        git_out(&r, &["log", "-1", "--format=%B", "HEAD~2"]),
        "Add B\n\nwith a body\n#123 kept"
    );
    let who = "--format=%an %ae %ad %cn %ce %cd %T";
    assert_eq!(
        git_out(&r, &["log", "-1", who, "HEAD~2"]),
        git_out(&r, &["log", "-1", who, &b])
    );
    assert_eq!(subjects(&r), ["base", "add a", "Add B", "add c", "add d"]);

    // Squash and fixup: one commit with both kept messages and all the changes.
    let r = stack();
    ok(
        &r,
        edit(
            &r,
            Some("HEAD~4"),
            vec![
                todo_item(&r, "HEAD~3", Pick),
                todo_item(&r, "HEAD~2", Squash),
                todo_item(&r, "HEAD~1", Fixup),
                todo_item(&r, "HEAD", Pick),
            ],
        ),
    );
    assert_eq!(subjects(&r), ["base", "add a", "add d"]);
    assert_eq!(
        git_out(&r, &["log", "-1", "--format=%B", "HEAD~1"]),
        "add a\n\nadd b"
    );
    assert_eq!(
        git_out(&r, &["show", "--format=", "--name-only", "HEAD~1"]),
        "a.txt\nb.txt\nc.txt"
    );

    // A message given on the first commit of a group is the group's whole message.
    let r = stack();
    ok(
        &r,
        edit(
            &r,
            Some("HEAD~2"),
            vec![
                reworded(&r, "HEAD~1", "c and d"),
                todo_item(&r, "HEAD", Squash),
            ],
        ),
    );
    assert_eq!(subjects(&r), ["base", "add a", "add b", "c and d"]);
    assert_eq!(git_out(&r, &["log", "-1", "--format=%B"]), "c and d");

    // Drop: the commit and its file are gone; the backup is named in the impact.
    let r = stack();
    let action = edit(
        &r,
        Some("HEAD~4"),
        vec![
            todo_item(&r, "HEAD~3", Pick),
            todo_item(&r, "HEAD~2", Drop),
            todo_item(&r, "HEAD~1", Pick),
            todo_item(&r, "HEAD", Pick),
        ],
    );
    let text = preview(&repo(&r), &action, false)
        .unwrap()
        .impact
        .join("\n");
    assert!(text.contains("Leaves out 1 commit"), "{text}");
    assert!(text.contains("the backup keeps it"), "{text}");
    ok(&r, action);
    assert_eq!(subjects(&r), ["base", "add a", "add c", "add d"]);
    assert!(!r.path().join("b.txt").exists());

    // Dropping only the last commit gives nothing a new id.
    let r = stack();
    let action = edit(
        &r,
        Some("HEAD~2"),
        vec![todo_item(&r, "HEAD~1", Pick), todo_item(&r, "HEAD", Drop)],
    );
    let text = preview(&repo(&r), &action, false)
        .unwrap()
        .impact
        .join("\n");
    assert!(!text.contains("new ids"), "{text}");
    let parent = r.rev("HEAD~1");
    ok(&r, action);
    assert_eq!(r.head(), parent);

    // From the first commit, rewording it: the new first commit has no parent.
    let r = stack();
    let mut todo = vec![reworded(&r, "HEAD~4", "the beginning")];
    for rev in ["HEAD~3", "HEAD~2", "HEAD~1", "HEAD"] {
        todo.push(todo_item(&r, rev, Pick));
    }
    ok(&r, edit(&r, None, todo));
    assert_eq!(
        subjects(&r),
        ["the beginning", "add a", "add b", "add c", "add d"]
    );
    assert!(r.parents("HEAD~4").is_empty());

    // Everything at once, with the impact listing each kind of change and other branches.
    let r = stack();
    r.branch("keep-old");
    r.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let action = edit(
        &r,
        Some("HEAD~4"),
        vec![
            todo_item(&r, "HEAD", Pick),
            reworded(&r, "HEAD~3", "A"),
            todo_item(&r, "HEAD~2", Squash),
            todo_item(&r, "HEAD~1", Drop),
        ],
    );
    let text = preview(&repo(&r), &action, false)
        .unwrap()
        .impact
        .join("\n");
    for want in [
        "new order",
        "new message",
        "Folds 1 commit",
        "Leaves out 1 commit",
        "2 commits get new ids",
        "already on origin/main",
        "Other branches with these commits keep the old ones: keep-old",
    ] {
        assert!(text.contains(want), "{want:?} missing from {text}");
    }
    ok(&r, action);
    assert_eq!(subjects(&r), ["base", "add d", "A"]);
    assert_eq!(git_out(&r, &["log", "-1", "--format=%B"]), "A");
    assert!(!r.path().join("c.txt").exists());
    assert_eq!(r.read("b.txt"), "b\n");
    assert_eq!(r.rev("keep-old~1"), r.rev("origin/main~1"));
}

#[test]
fn interactive_rebase_refuses_lists_that_do_not_fit() {
    use TodoOp::*;
    let r = stack();
    let head = r.head();
    let full = |r: &FixtureRepo| -> Vec<TodoItem> {
        ["HEAD~3", "HEAD~2", "HEAD~1", "HEAD"]
            .iter()
            .map(|rev| todo_item(r, rev, Pick))
            .collect()
    };
    let refused = |todo: Vec<TodoItem>, want: &str| {
        let e = err(&r, edit(&r, Some("HEAD~4"), todo));
        assert!(e.message.contains(want), "{want:?}: {e:?}");
        assert_eq!(r.head(), head);
    };
    refused(full(&r), "Nothing would change");
    let mut same_message = full(&r);
    same_message[1].message = Some("add b\n".into());
    refused(same_message, "Nothing would change");
    refused(full(&r)[1..].to_vec(), "is missing");
    let mut twice = full(&r);
    twice[0] = twice[1].clone();
    refused(twice, "listed twice");
    let mut other = full(&r);
    other[0].commit = r.rev("HEAD~4");
    refused(other, "not one of the commits being edited");
    let mut fold_first = full(&r);
    fold_first[0].op = Fixup;
    refused(fold_first, "no kept commit before it");
    let mut after_drop = full(&r);
    after_drop[0].op = Drop;
    after_drop[1].op = Squash;
    refused(after_drop, "no kept commit before it");
    let mut all_dropped = full(&r);
    for i in &mut all_dropped {
        i.op = Drop;
    }
    refused(all_dropped, "reset main");
    let mut dropped_message = full(&r);
    dropped_message[2].op = Drop;
    dropped_message[2].message = Some("x".into());
    refused(dropped_message, "cannot get a new message");
    let mut empty = full(&r);
    empty[2].message = Some(" \n ".into());
    refused(empty, "is empty");
    let mut nul = full(&r);
    nul[2].message = Some("a\0b".into());
    refused(nul, "NUL");

    // The base must be on the branch and before HEAD, and the range free of merges.
    let e = err(&r, edit(&r, Some("HEAD"), vec![]));
    assert!(e.message.contains("no commits after"), "{e:?}");
    let r = diverged();
    let e = err(
        &r,
        edit(&r, Some("main"), vec![todo_item(&r, "HEAD", Drop)]),
    );
    assert!(e.message.contains("is not part of side"), "{e:?}");
    let r = merges();
    r.checkout("main");
    let e = err(&r, edit(&r, Some("HEAD~3"), vec![]));
    assert!(e.message.contains("is a merge commit"), "{e:?}");

    // Every commit dropped from the first one would leave the branch empty.
    let r = linear(2);
    let e = err(
        &r,
        edit(
            &r,
            None,
            vec![todo_item(&r, "HEAD~1", Drop), todo_item(&r, "HEAD", Drop)],
        ),
    );
    assert!(e.message.contains("cannot be left empty"), "{e:?}");

    // Tracked changes are refused; untracked files are fine.
    let r = stack();
    r.write("untracked.txt", "x\n");
    r.write("a.txt", "edited\n");
    let drop_last = |r: &FixtureRepo| {
        edit(
            r,
            Some("HEAD~2"),
            vec![todo_item(r, "HEAD~1", Pick), todo_item(r, "HEAD", Drop)],
        )
    };
    let e = err(&r, drop_last(&r));
    assert!(e.message.contains("commit or stash"), "{e:?}");
    r.git(&["checkout", "--", "a.txt"]);
    ok(&r, drop_last(&r));
    assert_eq!(r.read("untracked.txt"), "x\n");
}

#[test]
fn interactive_rebase_stops_on_conflicts_and_keeps_new_messages() {
    use TodoOp::*;

    // Swap the clashing commits, rewording the one that moves first. Both stop on a conflict;
    // settling each and continuing finishes with the new message in place.
    let r = clashing();
    let out = ok(
        &r,
        edit(
            &r,
            Some("HEAD~3"),
            vec![
                reworded(&r, "HEAD", "two, first now\n\n#7 stays"),
                todo_item(&r, "HEAD~2", Pick),
                todo_item(&r, "HEAD~1", Pick),
            ],
        ),
    );
    assert_eq!(out.conflicts, vec!["f.txt".to_string()]);
    let s = crate::git::state::read_state(r.path(), false).unwrap();
    assert_eq!(s.in_progress, Some(crate::git::state::Operation::Rebase));
    // "Mine" is the commit being replayed; the other side is the rewritten commits so far.
    let info = crate::git::conflict::describe(r.path(), "f.txt").unwrap();
    assert!(info.swapped);
    assert_eq!(info.ours_name.as_deref(), Some("main"));
    assert_eq!(info.theirs_name.as_deref(), Some("the commits before it"));
    ok(&r, resolve("f.txt", Keep::Ours));
    let out = ok(&r, Action::Continue);
    assert_eq!(out.conflicts, vec!["f.txt".to_string()]);
    ok(&r, resolve("f.txt", Keep::Ours));
    ok(&r, Action::Continue);
    let s = crate::git::state::read_state(r.path(), false).unwrap();
    assert_eq!(s.in_progress, None);
    assert_eq!(subjects(&r), ["base", "two, first now", "set one", "add g"]);
    // git tidies a conflicted commit's message as it continues, dropping # lines.
    assert_eq!(
        git_out(&r, &["log", "-1", "--format=%B", "HEAD~2"]),
        "two, first now"
    );
    assert_eq!(r.read("f.txt"), "one\n");

    // A squashed commit that conflicts: the group still gets both messages, with none of
    // git's own notes.
    let r = clashing();
    ok(
        &r,
        edit(
            &r,
            Some("HEAD~3"),
            vec![
                todo_item(&r, "HEAD~2", Drop),
                todo_item(&r, "HEAD~1", Pick),
                todo_item(&r, "HEAD", Squash),
            ],
        ),
    );
    ok(&r, resolve("f.txt", Keep::Ours));
    ok(&r, Action::Continue);
    assert_eq!(subjects(&r), ["base", "add g"]);
    assert_eq!(
        git_out(&r, &["log", "-1", "--format=%B"]),
        "add g\n\nset two"
    );
    assert_eq!(r.read("f.txt"), "two\n");

    // Abort puts the branch back as it was.
    let r = clashing();
    let tip = r.head();
    ok(
        &r,
        edit(
            &r,
            Some("HEAD~3"),
            vec![
                todo_item(&r, "HEAD", Pick),
                todo_item(&r, "HEAD~2", Pick),
                todo_item(&r, "HEAD~1", Pick),
            ],
        ),
    );
    ok(&r, Action::AbortRebase);
    assert_eq!(r.head(), tip);
    assert_eq!(r.git(&["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(git_out(&r, &["status", "--porcelain"]), "");
}

#[test]
fn a_failed_backup_stops_every_dangerous_action() {
    let cases: Vec<(FixtureRepo, Action)> = vec![
        (
            linear(2),
            Action::Discard {
                paths: vec!["file.txt".into()],
            },
        ),
        (
            linear(2),
            Action::Commit {
                message: "amended".into(),
                amend: true,
            },
        ),
        (
            {
                let r = linear(2);
                r.branch("side");
                r
            },
            Action::DeleteBranch {
                name: "side".into(),
            },
        ),
        (
            linear(2),
            Action::Reset {
                target: "HEAD~1".into(),
                mode: ResetMode::Hard,
            },
        ),
        (
            {
                let r = conflict();
                start_conflict(&r);
                r
            },
            Action::AbortMerge,
        ),
        (
            diverged(),
            Action::Rebase {
                onto: "main".into(),
            },
        ),
        (
            {
                let r = conflict();
                let _ = r.command(r.path(), &["cherry-pick", "topic"]).output();
                r
            },
            Action::AbortCherryPick,
        ),
        (
            {
                let r = linear(3);
                start_revert_conflict(&r);
                r
            },
            Action::AbortRevert,
        ),
        (
            {
                let r = conflict();
                start_rebase_conflict(&r);
                r
            },
            Action::AbortRebase,
        ),
        (
            {
                let r = linear(2);
                r.write("stashed.txt", "x\n");
                r.git(&["add", "stashed.txt"]);
                r.git(&["stash", "push", "-q"]);
                r
            },
            Action::StashDrop {
                stash: String::new(),
            },
        ),
        (
            {
                let r = linear(2);
                r.write("stashed.txt", "x\n");
                r.git(&["add", "stashed.txt"]);
                r.git(&["stash", "push", "-q"]);
                r
            },
            Action::StashClear,
        ),
    ];
    let (pulled, _pull_remote) = ahead_behind();
    let (forced, force_remote) = ahead_behind();
    let remote_main = git_out(&force_remote, &["rev-parse", "main"]);
    let edited = stack();
    let edit_action = edit(
        &edited,
        Some("HEAD~2"),
        vec![
            todo_item(&edited, "HEAD", TodoOp::Pick),
            todo_item(&edited, "HEAD~1", TodoOp::Pick),
        ],
    );
    let cases = cases.into_iter().chain([
        (edited, edit_action),
        (pulled, Action::Pull { rebase: true }),
        (
            forced,
            Action::Push {
                remote: None,
                force: true,
            },
        ),
    ]);
    for (r, mut action) in cases {
        if let Action::StashDrop { stash } = &mut action {
            *stash = git_out(&r, &["rev-parse", "stash@{0}"]);
        }
        if !matches!(
            action,
            Action::AbortMerge
                | Action::AbortCherryPick
                | Action::AbortRevert
                | Action::AbortRebase
                | Action::Rebase { .. }
                | Action::RebaseInteractive { .. }
                | Action::StashDrop { .. }
                | Action::StashClear
                | Action::Pull { .. }
        ) {
            r.write("file.txt", "unstaged edit\n");
        }
        let p = preview(&repo(&r), &action, false).unwrap();
        // Break the backup service: a folder where its empty no-hooks file must be.
        std::fs::create_dir_all(r.path().join(".git/pm/no-hooks")).unwrap();
        let before = snapshot(&r);
        let log = OpLog::new();
        let e = run_logged(&r, &action, Some(&p.token), &log).unwrap_err();
        assert!(e.message.contains("No backup could be taken"), "{e:?}");
        assert_eq!(snapshot(&r), before, "{action:?} ran without a backup");
        let entries = log.entries();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].kind, OpKind::Backup);
        assert!(!entries[0].ok);
    }
    // A push changes the remote, which the local snapshot does not show.
    assert_eq!(git_out(&force_remote, &["rev-parse", "main"]), remote_main);
}

#[test]
fn dangerous_actions_need_a_current_confirmation() {
    let r = linear(3);
    let reset = Action::Reset {
        target: "HEAD~1".into(),
        mode: ResetMode::Hard,
    };
    let log = OpLog::new();
    let e = run_logged(&r, &reset, None, &log).unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidInput);
    assert!(log.entries().is_empty());

    // A token for another action does not count.
    let other = preview(
        &repo(&r),
        &Action::Reset {
            target: "HEAD~2".into(),
            mode: ResetMode::Hard,
        },
        false,
    )
    .unwrap();
    let e = run_logged(&r, &reset, Some(&other.token), &log).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Stale);

    // Work done after the preview makes its confirmation stale.
    let p = preview(&repo(&r), &reset, false).unwrap();
    r.write("file.txt", "written after the preview\n");
    let e = run_logged(&r, &reset, Some(&p.token), &log).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Stale);
    assert_eq!(r.read("file.txt"), "written after the preview\n");
    assert!(log.entries().is_empty());

    // Previewing again shows the new work and lets it run.
    let p = preview(&repo(&r), &reset, false).unwrap();
    assert!(
        p.impact
            .iter()
            .any(|l| l.contains("uncommitted changes to 1 file")),
        "{:?}",
        p.impact
    );
    run_logged(&r, &reset, Some(&p.token), &log).unwrap();

    // Safe actions need no token.
    r.write("n.txt", "n\n");
    run_logged(
        &r,
        &Action::Stage {
            paths: vec!["n.txt".into()],
        },
        None,
        &log,
    )
    .unwrap();
}

#[test]
fn previews_describe_the_impact() {
    let r = linear(4);
    // `side` forks before commits 3 and 4, so resetting main back two leaves them on no branch.
    r.git(&["branch", "side", "HEAD~2"]);
    r.checkout("side");
    r.commit_file("side.txt", "side\n", "side work");
    r.checkout("main");
    let p = preview(
        &repo(&r),
        &Action::Reset {
            target: "HEAD~2".into(),
            mode: ResetMode::Mixed,
        },
        false,
    )
    .unwrap();
    assert!(p.dangerous);
    assert!(p.title.starts_with("Reset main to "), "{}", p.title);
    let text = p.impact.join("\n");
    assert!(
        text.contains("2 commits will no longer be on main"),
        "{text}"
    );
    assert!(
        text.contains("2 commits of them will be on no branch"),
        "{text}"
    );
    assert_eq!(p.commands.len(), 1);
    assert!(
        p.commands[0].starts_with("git reset -q --mixed "),
        "{:?}",
        p.commands
    );

    let p = preview(
        &repo(&r),
        &Action::DeleteBranch {
            name: "side".into(),
        },
        false,
    )
    .unwrap();
    assert!(
        p.impact.join("\n").contains("1 commit only on this branch"),
        "{:?}",
        p.impact
    );

    // Amending a commit that is on a remote-tracking branch warns about the force push.
    let (local, _remote) = ahead_behind();
    let p = preview(
        &repo(&local),
        &Action::Commit {
            message: "x".into(),
            amend: true,
        },
        false,
    );
    let p = p.unwrap();
    assert!(
        !p.impact.join("\n").contains("force push"),
        "{:?}",
        p.impact
    );
    local.git(&["reset", "-q", "--hard", "origin/main"]);
    let p = preview(
        &repo(&local),
        &Action::Commit {
            message: "x".into(),
            amend: true,
        },
        false,
    )
    .unwrap();
    assert!(p.impact.join("\n").contains("force push"), "{:?}", p.impact);
}

#[test]
fn stage_unstage_and_commit() {
    let r = linear(2);
    r.commit_file("gone.txt", "gone\n", "add gone.txt");
    r.write("file.txt", "changed\n");
    r.write("dir/new.txt", "new\n");
    r.remove("gone.txt");
    ok(
        &r,
        Action::Stage {
            paths: vec!["file.txt".into(), "dir/new.txt".into(), "gone.txt".into()],
        },
    );
    assert_eq!(
        r.git(&["diff", "--cached", "--name-status"]),
        "A\tdir/new.txt\nM\tfile.txt\nD\tgone.txt"
    );
    ok(
        &r,
        Action::Unstage {
            paths: vec!["dir/new.txt".into()],
        },
    );
    assert_eq!(
        git_out(&r, &["status", "--porcelain", "--", "dir"]),
        "?? dir/"
    );

    let message = "Subject line\n\n#42 stays in the body\n";
    let out = ok(
        &r,
        Action::Commit {
            message: message.into(),
            amend: false,
        },
    );
    assert!(out.backup.is_none());
    assert_eq!(r.git(&["log", "-1", "--format=%B"]), message.trim_end());
    assert_eq!(
        r.git(&["show", "--name-only", "--format=", "HEAD"]),
        "file.txt\ngone.txt"
    );

    let e = err(
        &r,
        Action::Commit {
            message: "nothing staged".into(),
            amend: false,
        },
    );
    assert_eq!(e.kind, ErrorKind::InvalidInput);
    let e = err(
        &r,
        Action::Commit {
            message: "  \n".into(),
            amend: false,
        },
    );
    assert!(e.message.contains("empty"), "{e:?}");
}

#[test]
fn paths_are_literal() {
    let r = linear(1);
    // On Windows `*` cannot be in a file name; a `[ab]` pathspec glob still can.
    let star = if cfg!(windows) { "[ab].txt" } else { "*.txt" };
    for f in ["a.txt", "b.txt", star] {
        r.write(f, "x\n");
    }
    ok(
        &r,
        Action::Stage {
            paths: vec![star.into()],
        },
    );
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), star);
    // Pathspec magic is not magic either.
    let e = err(
        &r,
        Action::Stage {
            paths: vec![":(glob)*.txt".into()],
        },
    );
    assert_eq!(e.kind, ErrorKind::Git, "{e:?}");
    assert_eq!(r.git(&["diff", "--cached", "--name-only"]), star);
}

#[test]
fn bad_arguments_are_refused_before_git_runs() {
    let r = linear(2);
    r.branch("side");
    let paths = [
        "",
        "../outside",
        "/etc/passwd",
        "C:/Windows",
        "a/../../b",
        "a//b",
        "./a",
        "a\\b",
        ".git/config",
        ".GIT/hooks/pre-commit",
        "sub/.git/config",
        ".git./config",
    ];
    let mut actions: Vec<Action> = Vec::new();
    for p in paths {
        actions.push(Action::Stage {
            paths: vec![p.into()],
        });
        actions.push(Action::Discard {
            paths: vec![p.into()],
        });
    }
    actions.push(Action::Stage { paths: vec![] });
    for name in [
        "-D", "--force", "a..b", "HEAD", "a b", "x.lock", "@{-1}", "",
    ] {
        actions.push(Action::CreateBranch {
            name: name.into(),
            start: None,
            checkout: false,
        });
        actions.push(Action::DeleteBranch { name: name.into() });
        actions.push(Action::Checkout {
            branch: name.into(),
        });
        actions.push(Action::Merge {
            branch: name.into(),
        });
        actions.push(Action::RenameBranch {
            from: "side".into(),
            to: name.into(),
        });
    }
    for target in [
        "--upload-pack=touch x",
        "-q",
        "",
        "no-such-rev",
        "HEAD:file.txt",
    ] {
        actions.push(Action::Reset {
            target: target.into(),
            mode: ResetMode::Hard,
        });
        actions.push(Action::CreateBranch {
            name: "new".into(),
            start: Some(target.into()),
            checkout: false,
        });
    }
    let before = snapshot(&r);
    for action in actions {
        let log = OpLog::new();
        let e = preview(&repo(&r), &action, false).expect_err(&format!("{action:?}"));
        assert!(
            matches!(e.kind, ErrorKind::InvalidInput | ErrorKind::NotFound),
            "{action:?}: {e:?}"
        );
        // Run checks the same way, even with a made-up token.
        assert!(run_logged(&r, &action, Some("0"), &log).is_err());
        assert!(
            log.entries().is_empty(),
            "{action:?} ran {:?}",
            log.entries()
        );
    }
    assert_eq!(snapshot(&r), before);
}

#[test]
fn branch_operations() {
    let r = linear(3);
    ok(
        &r,
        Action::CreateBranch {
            name: "feature/x".into(),
            start: Some("HEAD~1".into()),
            checkout: false,
        },
    );
    assert_eq!(r.rev("feature/x"), r.rev("HEAD~1"));
    assert_eq!(r.git(&["branch", "--show-current"]), "main");
    let e = err(
        &r,
        Action::CreateBranch {
            name: "feature/x".into(),
            start: None,
            checkout: false,
        },
    );
    assert_eq!(e.kind, ErrorKind::AlreadyExists);

    ok(
        &r,
        Action::RenameBranch {
            from: "feature/x".into(),
            to: "feature/y".into(),
        },
    );
    assert!(r.git(&["branch", "--list", "feature/x"]).is_empty());
    let e = err(
        &r,
        Action::RenameBranch {
            from: "feature/y".into(),
            to: "main".into(),
        },
    );
    assert_eq!(e.kind, ErrorKind::AlreadyExists);

    ok(
        &r,
        Action::Checkout {
            branch: "feature/y".into(),
        },
    );
    assert_eq!(r.git(&["branch", "--show-current"]), "feature/y");
    let e = err(
        &r,
        Action::DeleteBranch {
            name: "feature/y".into(),
        },
    );
    assert!(e.message.contains("current branch"), "{e:?}");

    ok(
        &r,
        Action::CreateBranch {
            name: "topic".into(),
            start: None,
            checkout: true,
        },
    );
    assert_eq!(r.git(&["branch", "--show-current"]), "topic");
    assert_eq!(r.rev("topic"), r.rev("feature/y"));

    // A change the checkout would overwrite: git refuses, nothing changes, the log shows why.
    r.commit_file("file.txt", "topic\n", "topic edit");
    r.write("file.txt", "local edit\n");
    let p = preview(
        &repo(&r),
        &Action::Checkout {
            branch: "main".into(),
        },
        false,
    )
    .unwrap();
    assert!(
        p.impact.join(" ").contains("carried over"),
        "{:?}",
        p.impact
    );
    let log = OpLog::new();
    let e = run_logged(
        &r,
        &Action::Checkout {
            branch: "main".into(),
        },
        None,
        &log,
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Git);
    assert_eq!(r.read("file.txt"), "local edit\n");
    let entries = log.entries();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].ok);
    assert!(entries[0].stderr.contains("overwritten"), "{entries:?}");
}

#[test]
fn merges_fast_forward_clean_and_conflicted() {
    let r = linear(2);
    r.checkout_new("ff");
    r.commit_file("ff.txt", "ff\n", "ff work");
    r.checkout("main");
    let p = preview(
        &repo(&r),
        &Action::Merge {
            branch: "ff".into(),
        },
        false,
    )
    .unwrap();
    assert!(!p.dangerous);
    assert!(
        p.impact
            .join(" ")
            .contains("Fast-forwards main by 1 commit"),
        "{:?}",
        p.impact
    );
    ok(
        &r,
        Action::Merge {
            branch: "ff".into(),
        },
    );
    assert_eq!(r.rev("main"), r.rev("ff"));

    r.checkout_new("side");
    r.commit_file("side.txt", "side\n", "side work");
    r.checkout("main");
    r.commit_file("main.txt", "main\n", "main work");
    let out = ok(
        &r,
        Action::Merge {
            branch: "side".into(),
        },
    );
    assert!(out.conflicts.is_empty());
    assert_eq!(r.parents("HEAD").len(), 2);
    assert_eq!(r.git(&["log", "-1", "--format=%s"]), "Merge branch 'side'");

    // A tag with the same name as the branch: the branch is merged, not the tag.
    let r = conflict();
    r.git(&["tag", "topic", "main~1"]);
    let out = ok(
        &r,
        Action::Merge {
            branch: "topic".into(),
        },
    );
    assert_eq!(out.conflicts, vec!["conflict.txt".to_string()]);
    assert_eq!(r.rev("MERGE_HEAD"), r.rev("refs/heads/topic"));
    let e = err(
        &r,
        Action::Commit {
            message: "too early".into(),
            amend: false,
        },
    );
    assert!(e.message.contains("conflicts"), "{e:?}");
    ok(&r, Action::AbortMerge);
    assert_eq!(git_out(&r, &["status", "--porcelain"]), "");
    let e = err(&r, Action::AbortMerge);
    assert_eq!(e.kind, ErrorKind::InvalidInput);
}

#[test]
fn merge_of_a_remote_tracking_branch() {
    let (local, _remote) = ahead_behind();
    let behind = local.commit_count("main..origin/main");
    assert!(behind > 0);
    let out = ok(
        &local,
        Action::Merge {
            branch: "origin/main".into(),
        },
    );
    assert!(out.conflicts.is_empty());
    assert_eq!(local.commit_count("main..origin/main"), 0);
}

#[test]
fn the_operation_log_records_commands_and_failures() {
    let entries = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = entries.clone();
    let log = OpLog::with_listener(move |e| seen.lock().unwrap().push(e.clone()));
    let r = linear(2);
    r.write("file.txt", "edit\n");
    let stage = Action::Stage {
        paths: vec!["file.txt".into()],
    };
    run_logged(&r, &stage, None, &log).unwrap();
    let commit = Action::Commit {
        message: "secret-looking message".into(),
        amend: false,
    };
    run_logged(&r, &commit, None, &log).unwrap();
    let reset = Action::Reset {
        target: "HEAD~1".into(),
        mode: ResetMode::Soft,
    };
    let p = preview(&repo(&r), &reset, false).unwrap();
    run_logged(&r, &reset, Some(&p.token), &log).unwrap();

    let all = log.entries();
    assert_eq!(*entries.lock().unwrap(), all);
    let summary: Vec<(OpKind, &str, &str, bool)> = all
        .iter()
        .map(|e| (e.kind, e.action.as_str(), e.command.as_str(), e.ok))
        .collect();
    assert_eq!(summary.len(), 4, "{summary:?}");
    assert_eq!(
        summary[0],
        (OpKind::Git, "stage", "git add -A -- file.txt", true)
    );
    assert_eq!(
        summary[1],
        (
            OpKind::Git,
            "commit",
            "git commit -q --cleanup=whitespace -F -",
            true
        )
    );
    assert_eq!(summary[2].0, OpKind::Backup);
    assert!(summary[2].2.starts_with("backup ") && summary[2].2.ends_with("-reset-soft"));
    assert!(summary[3].2.starts_with("git reset -q --soft "));
    assert!(all.iter().all(|e| e.code.is_none() || e.code == Some(0)));
    assert!(all.windows(2).all(|w| w[1].seq == w[0].seq + 1));
    // The commit message went to git on stdin, so it is nowhere in the log.
    assert!(!format!("{all:?}").contains("secret-looking"));
}

#[test]
fn the_operation_log_keeps_the_latest_entries() {
    let log = OpLog::new();
    let out = GitOutput {
        command: "git status".into(),
        code: Some(1),
        stdout: Vec::new(),
        stderr: "x".repeat(10_000),
        duration: std::time::Duration::from_millis(3),
    };
    for _ in 0..OP_LOG_CAPACITY + 5 {
        log.git("/repo", "test", &out);
    }
    let all = log.entries();
    assert_eq!(all.len(), OP_LOG_CAPACITY);
    assert_eq!(all[0].seq, 6);
    assert!(all[0].stderr.len() < 4100);
    assert!(!all[0].ok);
    assert_eq!(all[0].duration_ms, 3);
}

#[test]
fn every_action_has_a_valid_backup_name() {
    let all = [
        Action::Stage { paths: vec![] },
        Action::Unstage { paths: vec![] },
        Action::StageHunks {
            path: String::new(),
            hunks: vec![],
        },
        Action::UnstageHunks {
            path: String::new(),
            hunks: vec![],
        },
        Action::Discard { paths: vec![] },
        Action::Commit {
            message: String::new(),
            amend: false,
        },
        Action::Commit {
            message: String::new(),
            amend: true,
        },
        Action::CreateBranch {
            name: String::new(),
            start: None,
            checkout: false,
        },
        Action::RenameBranch {
            from: String::new(),
            to: String::new(),
        },
        Action::DeleteBranch {
            name: String::new(),
        },
        Action::Checkout {
            branch: String::new(),
        },
        Action::Merge {
            branch: String::new(),
        },
        Action::AbortMerge,
        Action::Reset {
            target: String::new(),
            mode: ResetMode::Soft,
        },
        Action::Reset {
            target: String::new(),
            mode: ResetMode::Mixed,
        },
        Action::Reset {
            target: String::new(),
            mode: ResetMode::Hard,
        },
        Action::StashPush {
            message: None,
            include_untracked: false,
            staged_only: false,
        },
        Action::StashApply {
            stash: String::new(),
            index: false,
        },
        Action::StashPop {
            stash: String::new(),
            index: false,
        },
        Action::StashDrop {
            stash: String::new(),
        },
        Action::StashClear,
        Action::CherryPick {
            commit: String::new(),
        },
        Action::Revert {
            commit: String::new(),
        },
        Action::Rebase {
            onto: String::new(),
        },
        Action::RebaseInteractive {
            base: None,
            todo: vec![],
        },
        Action::AbortCherryPick,
        Action::AbortRevert,
        Action::AbortRebase,
        Action::Continue,
        Action::Fetch { remote: None },
        Action::Pull { rebase: false },
        Action::Pull { rebase: true },
        Action::Push {
            remote: None,
            force: false,
        },
        Action::Push {
            remote: None,
            force: true,
        },
        Action::PushTag {
            tag: String::new(),
            remote: String::new(),
        },
    ];
    let dangerous: Vec<&str> = all.iter().filter_map(Action::danger).collect();
    assert_eq!(
        dangerous,
        [
            "discard",
            "amend",
            "delete-branch",
            "abort-merge",
            "reset-soft",
            "reset-mixed",
            "reset-hard",
            "stash-drop",
            "stash-clear",
            "rebase",
            "rebase-interactive",
            "abort-cherry-pick",
            "abort-revert",
            "abort-rebase",
            "pull-rebase",
            "force-push"
        ]
    );
    let r = linear(1);
    let b = Backups::open(r.path()).unwrap();
    for name in dangerous {
        b.create(name, &Default::default()).unwrap();
    }
    // The wire format the UI sends.
    let json = serde_json::to_string(&Action::Reset {
        target: "HEAD~1".into(),
        mode: ResetMode::Hard,
    })
    .unwrap();
    assert_eq!(json, r#"{"type":"reset","target":"HEAD~1","mode":"hard"}"#);
    let back: Action = serde_json::from_str(r#"{"type":"abortMerge"}"#).unwrap();
    assert_eq!(back, Action::AbortMerge);
}

/// The filesystem root is owned by root: actions there are refused unless confirmed.
#[cfg(unix)]
#[test]
fn untrusted_repositories_are_refused() {
    if crate::git::is_trusted(Path::new("/")) {
        eprintln!("skipped: running as root or safe.directory allows /");
        return;
    }
    let action = Action::Stage {
        paths: vec!["x".into()],
    };
    let e = preview(&Repo::new("/"), &action, false).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Untrusted);
    let e = run(
        &Repo::new("/"),
        &action,
        None,
        &RunOptions::default(),
        &OpLog::new(),
        &CancelToken::new(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Untrusted);
}

fn work_hunks(r: &FixtureRepo, path: &str, side: WorkSide) -> Vec<Hunk> {
    let g = crate::git::state::open(r.path()).unwrap();
    work_diff(&g, path, side, hunks::WORK_CONTEXT, false)
        .unwrap()
        .hunks
}

fn ids(hs: &[&Hunk]) -> Vec<String> {
    hs.iter().map(|h| h.id.clone()).collect()
}

/// The staged bytes of a file.
fn staged(r: &FixtureRepo, path: &str) -> Vec<u8> {
    r.command(r.path(), &["show", &format!(":{path}")])
        .output()
        .unwrap()
        .stdout
}

fn numbered(lines: impl IntoIterator<Item = String>) -> String {
    lines.into_iter().map(|l| l + "\n").collect()
}

#[test]
fn hunks_are_staged_and_unstaged_in_any_combination() {
    let r = linear(1);
    let base: Vec<String> = (1..=20).map(|i| format!("l{i}")).collect();
    r.commit_file("f.txt", &numbered(base.clone()), "twenty lines");
    // Three hunks: a changed line, two lines inserted after l10, and l18-l19 deleted.
    let edit = |change: bool, insert: bool, delete: bool| {
        let mut out = Vec::new();
        for (i, l) in base.iter().enumerate() {
            let n = i + 1;
            if delete && (n == 18 || n == 19) {
                continue;
            }
            out.push(if change && n == 2 {
                "two".into()
            } else {
                l.clone()
            });
            if insert && n == 10 {
                out.extend(["new a".to_string(), "new b".to_string()]);
            }
        }
        numbered(out).into_bytes()
    };
    let disk = edit(true, true, true);
    r.write("f.txt", &disk);
    let hs = work_hunks(&r, "f.txt", WorkSide::Unstaged);
    assert_eq!(hs.len(), 3, "{hs:#?}");

    // The later two together: the deletion's position shifts by the insertion.
    let log = OpLog::new();
    let stage = Action::StageHunks {
        path: "f.txt".into(),
        hunks: ids(&[&hs[1], &hs[2]]),
    };
    let p = preview(&repo(&r), &stage, false).unwrap();
    assert!(!p.dangerous);
    assert_eq!(p.title, "Stage 2 hunks of f.txt");
    run_logged(&r, &stage, None, &log).unwrap();
    assert_eq!(staged(&r, "f.txt"), edit(false, true, true));
    assert_eq!(std::fs::read(r.path().join("f.txt")).unwrap(), disk);
    // git apply finds hunks by their old lines, so check the patch's headers (the later hunk's
    // new start moved by the insertion) against git's own diff of the result.
    let patch = hunks::build_patch(
        "f.txt",
        numbered(base.clone()).as_bytes(),
        &disk,
        &[&hs[1], &hs[2]],
        false,
    )
    .unwrap();
    let headers = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|l| l.starts_with("@@"))
            .map(|l| l.split(" @@").next().unwrap().to_string())
            .collect()
    };
    let git_diff = git_out(&r, &["diff", "--cached", "-U3", "--", "f.txt"]);
    assert_eq!(
        headers(&String::from_utf8_lossy(&patch)),
        headers(&git_diff)
    );
    assert_eq!(headers(&git_diff), ["@@ -8,6 +8,8", "@@ -15,6 +17,4"]);
    // The patch goes to git on stdin; no line of the file reaches the log.
    let entries = log.entries();
    assert_eq!(
        entries.last().unwrap().command,
        "git apply --cached --whitespace=nowarn -"
    );
    assert!(!format!("{entries:?}").contains("new a"));

    // Unstage the deletion only, then the insertion.
    let st = work_hunks(&r, "f.txt", WorkSide::Staged);
    assert_eq!(st.len(), 2);
    ok(
        &r,
        Action::UnstageHunks {
            path: "f.txt".into(),
            hunks: ids(&[&st[1]]),
        },
    );
    assert_eq!(staged(&r, "f.txt"), edit(false, true, false));
    let st = work_hunks(&r, "f.txt", WorkSide::Staged);
    ok(
        &r,
        Action::UnstageHunks {
            path: "f.txt".into(),
            hunks: ids(&[&st[0]]),
        },
    );
    assert_eq!(staged(&r, "f.txt"), edit(false, false, false));

    // All at once, both ways.
    let hs = work_hunks(&r, "f.txt", WorkSide::Unstaged);
    ok(
        &r,
        Action::StageHunks {
            path: "f.txt".into(),
            hunks: ids(&hs.iter().collect::<Vec<_>>()),
        },
    );
    assert_eq!(staged(&r, "f.txt"), disk);
    assert_eq!(git_out(&r, &["status", "--porcelain"]), "M  f.txt");
    let st = work_hunks(&r, "f.txt", WorkSide::Staged);
    assert_eq!(st.len(), 3);
    ok(
        &r,
        Action::UnstageHunks {
            path: "f.txt".into(),
            hunks: ids(&st.iter().collect::<Vec<_>>()),
        },
    );
    assert_eq!(staged(&r, "f.txt"), edit(false, false, false));

    // A hunk that is no longer there (the file changed after the diff was shown).
    let old = work_hunks(&r, "f.txt", WorkSide::Unstaged);
    r.write("f.txt", edit(false, true, true));
    let e = err(
        &r,
        Action::StageHunks {
            path: "f.txt".into(),
            hunks: ids(&[&old[0]]),
        },
    );
    assert_eq!(e.kind, ErrorKind::Stale, "{e:?}");
    assert_eq!(staged(&r, "f.txt"), edit(false, false, false));
    let e = err(
        &r,
        Action::StageHunks {
            path: "f.txt".into(),
            hunks: vec![],
        },
    );
    assert_eq!(e.kind, ErrorKind::InvalidInput);
}

#[test]
fn hunk_staging_keeps_bytes_and_refuses_what_git_would_change() {
    let r = linear(1);
    let stage_all = |r: &FixtureRepo, path: &str| {
        let hs = work_hunks(r, path, WorkSide::Unstaged);
        confirm_and_run(
            r,
            &Action::StageHunks {
                path: path.into(),
                hunks: ids(&hs.iter().collect::<Vec<_>>()),
            },
        )
    };

    // CRLF kept in a file committed with CRLF, a byte that is not UTF-8, no final newline.
    let lines: Vec<&[u8]> = vec![b"a", b"b", b"c", b"d", b"e", b"f", b"g", b"h", b"i", b"j"];
    r.write("crlf.txt", lines.join(&b"\r\n"[..]));
    r.commit_all("crlf");
    let mut edited = lines.clone();
    edited[1] = b"B\xe9";
    edited[9] = b"J";
    let disk = edited.join(&b"\r\n"[..]);
    r.write("crlf.txt", &disk);
    let hs = work_hunks(&r, "crlf.txt", WorkSide::Unstaged);
    assert_eq!(hs.len(), 2);
    ok(
        &r,
        Action::StageHunks {
            path: "crlf.txt".into(),
            hunks: ids(&[&hs[1]]),
        },
    );
    let mut last_only = lines.clone();
    last_only[9] = b"J";
    assert_eq!(staged(&r, "crlf.txt"), last_only.join(&b"\r\n"[..]));
    stage_all(&r, "crlf.txt").unwrap();
    assert_eq!(staged(&r, "crlf.txt"), disk);

    // CRLF on disk over LF in the index: git keeps the CRLF unless it is told to convert.
    r.commit_file("lf.txt", "one\ntwo\nthree\n", "lf");
    r.write("lf.txt", "one\r\nTWO\r\nthree\r\n");
    let e = stage_all(&r, "lf.txt").unwrap_err();
    assert!(e.message.contains("CRLF"), "{e:?}");
    assert_eq!(staged(&r, "lf.txt"), b"one\ntwo\nthree\n");
    r.git(&["config", "core.autocrlf", "true"]);
    stage_all(&r, "lf.txt").unwrap();
    assert_eq!(staged(&r, "lf.txt"), b"one\nTWO\nthree\n");
    r.git(&["config", "--unset", "core.autocrlf"]);

    // Names that need quoting in a patch.
    let mut names = vec!["sp ace #1.txt".to_string()];
    if cfg!(target_os = "linux") {
        names.push("q\"uote é.txt".into());
    }
    for name in &names {
        r.commit_file(name, "x\ny\n", "awkward name");
        r.write(name, "x\nY\n");
        stage_all(&r, name).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(staged(&r, name), b"x\nY\n", "{name}");
    }

    // A filter would store something else than the bytes on disk.
    r.commit_file("big.dat", "1\n2\n", "data");
    r.write(".gitattributes", "*.dat filter=lfs diff=lfs merge=lfs\n");
    r.write("big.dat", "1\n3\n");
    let e = stage_all(&r, "big.dat").unwrap_err();
    assert!(e.message.contains("filter"), "{e:?}");

    // New files, and the old-to-new id check, take the whole file.
    r.write("brand-new.txt", "hello\n");
    let e = confirm_and_run(
        &r,
        &Action::StageHunks {
            path: "brand-new.txt".into(),
            hunks: vec!["x".into()],
        },
    )
    .unwrap_err();
    assert!(e.message.contains("is new"), "{e:?}");

    let c = conflict();
    start_conflict(&c);
    let path = git_out(&c, &["diff", "--name-only", "--diff-filter=U"]);
    let e = confirm_and_run(
        &c,
        &Action::StageHunks {
            path: path.trim().into(),
            hunks: vec!["x".into()],
        },
    )
    .unwrap_err();
    assert!(e.message.contains("conflict"), "{e:?}");

    // Paths are checked like every other action's.
    let e = confirm_and_run(
        &r,
        &Action::StageHunks {
            path: "../outside.txt".into(),
            hunks: vec!["x".into()],
        },
    )
    .unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidInput);
}

/// The full id of the stash at this position.
fn stash_id(r: &FixtureRepo, n: usize) -> String {
    git_out(r, &["rev-parse", &format!("stash@{{{n}}}")])
}

/// Two stashes, each holding one tracked edit. Returns (older, newer) ids.
fn two_stashes(r: &FixtureRepo) -> (String, String) {
    r.write("file.txt", "first stash\n");
    ok(
        r,
        Action::StashPush {
            message: Some("first".into()),
            include_untracked: false,
            staged_only: false,
        },
    );
    r.write("file.txt", "second stash\n");
    ok(
        r,
        Action::StashPush {
            message: None,
            include_untracked: false,
            staged_only: false,
        },
    );
    (stash_id(r, 1), stash_id(r, 0))
}

fn push(message: Option<&str>, include_untracked: bool, staged_only: bool) -> Action {
    Action::StashPush {
        message: message.map(str::to_string),
        include_untracked,
        staged_only,
    }
}

#[test]
fn stash_sets_work_aside_and_apply_brings_it_back() {
    let r = linear(2);
    make_dirty(&r);
    let before = git_out(&r, &["status", "--porcelain=v1", "--untracked-files=all"]);
    let out = ok(&r, push(Some("before the merge"), false, false));
    assert_eq!(out.title, "Stash \"before the merge\"");
    // Stashing needs no confirmation but leaves a restore point, so it can be undone.
    let backup = out.backup.clone().expect("a stash leaves a restore point");
    assert_eq!(backup.action, "stash-push");
    // Tracked changes are set aside, the untracked file stays.
    assert_eq!(
        git_out(&r, &["status", "--porcelain=v1", "--untracked-files=all"]).trim(),
        "?? untracked/new.txt"
    );
    let list = list_stashes(&repo(&r)).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].message, "before the merge");
    assert_eq!(list[0].branch.as_deref(), Some("main"));
    assert_eq!((list[0].files, list[0].untracked), (2, 0));
    assert_eq!(list[0].untracked_commit, None);
    assert_eq!(list[0].commit, stash_id(&r, 0));

    // Restoring the point puts the stashed changes back in the working tree.
    crate::backup::Backups::open(r.path())
        .unwrap()
        .restore(&backup.id, &Default::default())
        .unwrap();
    assert_eq!(r.read("file.txt"), "unstaged edit\n");
    assert_eq!(
        git_out(&r, &["status", "--porcelain=v1", "--untracked-files=all"]),
        before
    );
    // Back to just the stash for the rest of the test. A restore keeps stashes made since the
    // point, so the first one is still listed (when the second is made in the same second it is
    // the very same commit and the list collapses, which made this test depend on the clock).
    r.git(&["stash", "clear"]);
    r.git(&["reset", "-q", "--hard"]);
    r.git(&["clean", "-q", "-f", "-d"]);
    make_dirty(&r);
    let out = ok(&r, push(Some("before the merge"), false, false));
    assert!(out.backup.is_some());
    let list = list_stashes(&repo(&r)).unwrap();
    // Apply keeps the stash; the edit comes back unstaged, the new file stays staged.
    let out = ok(
        &r,
        Action::StashApply {
            stash: list[0].commit.clone(),
            index: false,
        },
    );
    assert!(out.conflicts.is_empty());
    assert_eq!(r.read("file.txt"), "unstaged edit\n");
    assert_eq!(
        git_out(&r, &["status", "--porcelain=v1", "--untracked-files=all"]),
        " M file.txt\nA  staged.txt\n?? untracked/new.txt"
    );
    assert_eq!(list_stashes(&repo(&r)).unwrap().len(), 1);

    // Pop removes it once applied.
    r.git(&["reset", "-q", "--hard"]);
    ok(
        &r,
        Action::StashPop {
            stash: list[0].commit.clone(),
            index: false,
        },
    );
    assert_eq!(r.read("file.txt"), "unstaged edit\n");
    assert!(list_stashes(&repo(&r)).unwrap().is_empty());
}

#[test]
fn stash_can_include_untracked_files_or_only_the_staged_ones() {
    let r = linear(2);
    make_dirty(&r);
    ok(&r, push(None, true, false));
    assert_eq!(git_out(&r, &["status", "--porcelain=v1"]), "");
    assert!(!r.path().join("untracked/new.txt").exists());
    let entry = &list_stashes(&repo(&r)).unwrap()[0];
    assert_eq!((entry.files, entry.untracked), (2, 1));
    // The untracked files can be shown as a commit of their own.
    let third = entry.untracked_commit.clone().expect("untracked commit");
    assert_eq!(
        third,
        git_out(&r, &["rev-parse", &format!("{}^3", entry.commit)])
    );
    // The default message is git's own.
    assert!(entry.message.contains(&r.rev("HEAD")[..7]), "{entry:?}");
    ok(
        &r,
        Action::StashPop {
            stash: entry.commit.clone(),
            index: false,
        },
    );
    assert_eq!(r.read("untracked/new.txt"), "untracked\n");

    // Only what is staged; the rest of the working tree is untouched.
    let r = linear(2);
    make_dirty(&r);
    ok(&r, push(Some("staged only"), false, true));
    assert_eq!(
        git_out(&r, &["status", "--porcelain=v1", "--untracked-files=all"]),
        " M file.txt\n?? untracked/new.txt"
    );
    assert_eq!(list_stashes(&repo(&r)).unwrap()[0].files, 1);
}

#[test]
fn applying_with_index_stages_again_what_was_staged() {
    let r = linear(2);
    r.write("file.txt", "staged edit\n");
    r.git(&["add", "file.txt"]);
    ok(&r, push(None, false, false));
    let id = stash_id(&r, 0);
    ok(
        &r,
        Action::StashApply {
            stash: id.clone(),
            index: true,
        },
    );
    assert_eq!(git_out(&r, &["status", "--porcelain=v1"]), "M  file.txt");
    r.git(&["reset", "-q", "--hard"]);
    ok(
        &r,
        Action::StashApply {
            stash: id,
            index: false,
        },
    );
    assert_eq!(git_out(&r, &["status", "--porcelain=v1"]), " M file.txt");
}

#[test]
fn a_stash_is_named_by_its_commit_not_its_position() {
    let r = linear(2);
    let (first, second) = two_stashes(&r);
    // `first` is now at position 1; another stash on top moves it to 2.
    r.write("file.txt", "third\n");
    ok(&r, push(None, false, false));
    ok(
        &r,
        Action::StashApply {
            stash: first.clone(),
            index: false,
        },
    );
    assert_eq!(r.read("file.txt"), "first stash\n");
    r.git(&["reset", "-q", "--hard"]);
    ok(&r, Action::StashDrop { stash: second });
    let ids: Vec<String> = list_stashes(&repo(&r))
        .unwrap()
        .into_iter()
        .map(|s| s.commit)
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&first));

    // One that is gone is not found, and a position or a name is not an id.
    let gone = ids[0].clone();
    ok(
        &r,
        Action::StashDrop {
            stash: gone.clone(),
        },
    );
    for stash in [
        gone,
        "stash@{0}".to_string(),
        "main".to_string(),
        "--all".into(),
    ] {
        let e = err(
            &r,
            Action::StashApply {
                stash,
                index: false,
            },
        );
        assert!(
            matches!(e.kind, ErrorKind::NotFound | ErrorKind::InvalidInput),
            "{e:?}"
        );
    }
}

#[test]
fn a_conflicting_pop_keeps_the_stash_and_lists_the_conflicts() {
    let r = linear(2);
    r.write("file.txt", "from the stash\n");
    ok(&r, push(None, false, false));
    r.commit_file("file.txt", "from a commit\n", "another change");
    let id = stash_id(&r, 0);
    let out = ok(
        &r,
        Action::StashPop {
            stash: id.clone(),
            index: false,
        },
    );
    assert_eq!(out.conflicts, ["file.txt"]);
    assert_eq!(
        list_stashes(&repo(&r)).unwrap()[0].commit,
        id,
        "pop keeps the stash after a conflict"
    );
    // The conflict is resolved like any other, and nothing else can start meanwhile.
    let e = err(&r, push(None, false, false));
    assert!(e.message.contains("conflicts"), "{e:?}");
    let e = err(
        &r,
        Action::StashApply {
            stash: id,
            index: false,
        },
    );
    assert!(e.message.contains("conflicts"), "{e:?}");
}

#[test]
fn a_stash_that_would_overwrite_changes_is_refused_and_changes_nothing() {
    let r = linear(2);
    r.write("file.txt", "stashed\n");
    ok(&r, push(None, false, false));
    let id = stash_id(&r, 0);
    r.write("file.txt", "local edit\n");
    let before = snapshot(&r);
    let e = err(
        &r,
        Action::StashPop {
            stash: id,
            index: false,
        },
    );
    assert_eq!(e.kind, ErrorKind::Git, "{e:?}");
    assert_eq!(snapshot(&r), before);
}

#[test]
fn bad_stash_requests_are_refused_before_git_runs() {
    let r = linear(2);
    let e = err(&r, push(None, false, false));
    assert!(e.message.contains("nothing to stash"), "{e:?}");
    r.write("only-new.txt", "x\n");
    let e = err(&r, push(None, false, false));
    assert!(e.message.contains("untracked"), "{e:?}");
    let e = err(&r, push(None, false, true));
    assert!(e.message.contains("staged"), "{e:?}");
    r.write("file.txt", "edit\n");
    let e = err(&r, push(Some("two\nlines"), false, false));
    assert!(e.message.contains("one line"), "{e:?}");
    let e = err(&r, push(None, true, true));
    assert!(e.message.contains("untracked"), "{e:?}");
    // Nothing was stashed by any of them.
    assert!(list_stashes(&repo(&r)).unwrap().is_empty());
    let e = err(&r, Action::StashClear);
    assert!(e.message.contains("no stashes"), "{e:?}");

    // No commits yet, and a merge in progress.
    let empty = FixtureRepo::new();
    empty.write("a.txt", "a\n");
    let e = err(&empty, push(None, true, false));
    assert!(e.message.contains("no commits"), "{e:?}");
    let r = conflict();
    start_conflict(&r);
    let e = err(&r, push(None, false, false));
    assert!(e.message.contains("merge"), "{e:?}");
}

#[test]
fn stash_drop_and_clear_preview_what_they_delete() {
    let r = linear(2);
    let (first, _) = two_stashes(&r);
    let p = preview(&repo(&r), &Action::StashDrop { stash: first }, false).unwrap();
    assert!(p.dangerous);
    assert_eq!(p.title, "Drop stash \"first\"");
    assert!(p.impact[0].contains("1 file"), "{p:?}");
    assert_eq!(p.commands, ["git stash drop -q stash@{1}"]);
    let p = preview(&repo(&r), &Action::StashClear, false).unwrap();
    assert!(p.dangerous);
    assert!(p.impact[0].contains("all 2 stashes"), "{p:?}");

    // The confirmation goes stale when the list changes.
    let token = p.token;
    r.write("file.txt", "third\n");
    ok(&r, push(None, false, false));
    let e = run_logged(&r, &Action::StashClear, Some(&token), &OpLog::new()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Stale, "{e:?}");
    assert_eq!(list_stashes(&repo(&r)).unwrap().len(), 3);
}

#[test]
fn restoring_a_backup_puts_dropped_stashes_back_and_keeps_newer_ones() {
    let r = linear(2);
    let (first, second) = two_stashes(&r);
    let out = ok(&r, Action::StashClear);
    let backup = out.backup.unwrap();
    assert_eq!(backup.stashes.len(), 2);
    assert_eq!(backup.stashes[0].commit, second);
    r.write("file.txt", "made after the clear\n");
    ok(&r, push(Some("newer"), false, false));
    Backups::open(r.path())
        .unwrap()
        .restore(&backup.id, &Default::default())
        .unwrap();
    let list = list_stashes(&repo(&r)).unwrap();
    let ids: Vec<&str> = list.iter().map(|s| s.commit.as_str()).collect();
    assert_eq!(ids.len(), 3, "{list:?}");
    assert!(ids.contains(&first.as_str()) && ids.contains(&second.as_str()));
    assert!(list.iter().any(|s| s.message == "newer"));
    // The messages came back too.
    assert!(list.iter().any(|s| s.message == "first"));
}

#[test]
fn the_operation_log_survives_a_restart_and_clears() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("oplog.jsonl");
    let r = linear(2);
    r.write("file.txt", "edit\n");
    let stage = Action::Stage {
        paths: vec!["file.txt".into()],
    };

    let log = OpLog::persistent(file.clone(), |_| {});
    run_logged(&r, &stage, None, &log).unwrap();
    let before = log.entries();
    assert!(!before.is_empty());

    // A new log on the same file has the same entries and carries on numbering after them.
    let again = OpLog::persistent(file.clone(), |_| {});
    assert_eq!(again.entries(), before);
    run_logged(
        &r,
        &Action::Unstage {
            paths: vec!["file.txt".into()],
        },
        None,
        &again,
    )
    .unwrap();
    assert!(again.entries().len() > before.len());
    let seqs: Vec<u64> = again.entries().iter().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));

    // A damaged line is skipped, and clearing empties the file too.
    std::fs::write(&file, "not json\n").unwrap();
    assert!(OpLog::persistent(file.clone(), |_| {}).entries().is_empty());
    again.clear();
    assert!(OpLog::persistent(file, |_| {}).entries().is_empty());
}

fn resolve(path: &str, keep: Keep) -> Action {
    Action::Resolve {
        path: path.into(),
        keep,
    }
}

fn unmerged(r: &FixtureRepo) -> String {
    git_out(r, &["ls-files", "-u"])
}

#[test]
fn resolving_a_conflict_keeps_a_side_and_takes_a_restore_point() {
    for (keep, want) in [
        (Keep::Ours, "main version\n"),
        (Keep::Theirs, "topic version\n"),
    ] {
        let r = conflict();
        start_conflict(&r);
        let out = ok(&r, resolve("conflict.txt", keep));
        assert_eq!(
            std::fs::read_to_string(r.path().join("conflict.txt")).unwrap(),
            want
        );
        assert!(unmerged(&r).is_empty());
        assert!(out.backup.is_some(), "a restore point is taken first");
    }
}

#[test]
fn resolving_with_edited_text_writes_it_and_stages_it() {
    let r = conflict();
    start_conflict(&r);
    ok(
        &r,
        resolve(
            "conflict.txt",
            Keep::Edited {
                content: "both\n".into(),
            },
        ),
    );
    assert_eq!(
        std::fs::read_to_string(r.path().join("conflict.txt")).unwrap(),
        "both\n"
    );
    assert!(unmerged(&r).is_empty());
    assert_eq!(
        git_out(&r, &["diff", "--cached", "--name-only"]).trim(),
        "conflict.txt"
    );
}

#[test]
fn resolving_refuses_what_is_not_in_conflict() {
    let r = conflict();
    start_conflict(&r);
    r.write("other.txt", "x\n");
    assert!(
        err(&r, resolve("other.txt", Keep::Ours))
            .message
            .contains("not in conflict")
    );
    assert!(err(&r, resolve("../x", Keep::Ours)).message.contains(".."));
}

#[test]
fn a_file_deleted_on_one_side_can_be_deleted_or_kept() {
    let make = || {
        let r = FixtureRepo::new();
        r.commit_file("a.txt", "x\n", "base");
        r.checkout_new("topic");
        r.git(&["rm", "-q", "a.txt"]);
        r.git(&["commit", "-q", "-m", "delete"]);
        r.checkout("main");
        r.commit_file("a.txt", "x\nchanged\n", "change");
        let _ = r.command(r.path(), &["merge", "topic"]).output();
        r
    };
    let r = make();
    assert!(
        err(&r, resolve("a.txt", Keep::Theirs))
            .message
            .contains("delete it instead")
    );
    ok(&r, resolve("a.txt", Keep::Delete));
    assert!(!r.path().join("a.txt").exists());
    assert!(unmerged(&r).is_empty());
    let r = make();
    ok(&r, resolve("a.txt", Keep::Ours));
    assert!(r.path().join("a.txt").exists());
    assert!(unmerged(&r).is_empty());
}

#[test]
fn ignoring_adds_a_line_to_gitignore_once() {
    let r = linear(1);
    r.write("junk.log", "x\n");
    assert!(git_out(&r, &["status", "--porcelain"]).contains("junk.log"));
    let out = ok(
        &r,
        Action::Ignore {
            pattern: "*.log".into(),
        },
    );
    assert_eq!(out.title, "Ignore *.log");
    assert_eq!(r.read(".gitignore"), "*.log\n");
    assert!(!git_out(&r, &["status", "--porcelain"]).contains("junk.log"));
    // A second line is added after the first, whatever the file ended with.
    std::fs::write(r.path().join(".gitignore"), "*.log").unwrap();
    ok(
        &r,
        Action::Ignore {
            pattern: "/build/".into(),
        },
    );
    assert_eq!(r.read(".gitignore"), "*.log\n/build/\n");
    assert!(
        err(
            &r,
            Action::Ignore {
                pattern: "*.log".into()
            }
        )
        .message
        .contains("already")
    );
    assert!(
        err(
            &r,
            Action::Ignore {
                pattern: "a\nb".into()
            }
        )
        .message
        .contains("one line")
    );
    assert!(
        err(
            &r,
            Action::Ignore {
                pattern: "!keep".into()
            }
        )
        .message
        .contains("start with")
    );
}

#[test]
fn fetch_pull_and_push_with_a_remote() {
    let e = err(&linear(1), Action::Fetch { remote: None });
    assert!(e.message.contains("no remote"), "{e:?}");

    let (r, remote) = ahead_behind();
    let e = err(
        &r,
        Action::Fetch {
            remote: Some("nope".into()),
        },
    );
    assert!(e.message.contains("no remote \"nope\""), "{e:?}");
    // Behind the remote: a plain push is refused before git runs.
    let push = Action::Push {
        remote: None,
        force: false,
    };
    let e = err(&r, push.clone());
    assert!(e.message.contains("pull first"), "{e:?}");
    // The force push dialog says what is replaced, and that this is the main branch.
    let p = preview(
        &repo(&r),
        &Action::Push {
            remote: None,
            force: true,
        },
        false,
    )
    .unwrap();
    assert!(p.dangerous);
    assert!(
        p.impact[0].starts_with("Replaces main on origin: 1 commit"),
        "{:?}",
        p.impact
    );
    assert!(
        p.impact.iter().any(|l| l.contains("main branch")),
        "{:?}",
        p.impact
    );
    assert!(
        p.commands[0].contains("--force-with-lease=refs/heads/main:"),
        "{:?}",
        p.commands
    );

    ok(&r, Action::Fetch { remote: None });
    ok(
        &r,
        Action::Fetch {
            remote: Some("origin".into()),
        },
    );
    let p = preview(&repo(&r), &Action::Pull { rebase: false }, false).unwrap();
    assert!(!p.dangerous);
    assert!(
        p.impact
            .iter()
            .any(|l| l.contains("1 commit is to come in and main has 2 commits")),
        "{:?}",
        p.impact
    );
    let out = ok(&r, Action::Pull { rebase: false });
    assert!(out.conflicts.is_empty());
    assert_eq!(
        r.commit_count("HEAD"),
        6,
        "two of each side, the base and a merge"
    );
    ok(&r, push.clone());
    assert_eq!(git_out(&remote, &["rev-parse", "main"]), r.head());
    let e = err(&r, push.clone());
    assert!(e.message.starts_with("Nothing to push"), "{e:?}");
    let e = err(
        &r,
        Action::Push {
            remote: None,
            force: true,
        },
    );
    assert!(e.message.contains("normal Push is enough"), "{e:?}");

    // A new branch goes to the only remote and tracks it from then on.
    r.checkout_new("feat");
    let e = err(&r, Action::Pull { rebase: false });
    assert!(e.message.contains("no upstream"), "{e:?}");
    r.commit_file("feat.txt", "f\n", "feature");
    let p = preview(&repo(&r), &push, false).unwrap();
    assert!(
        p.impact[0].starts_with("Creates feat on origin"),
        "{:?}",
        p.impact
    );
    assert!(p.impact.iter().any(|l| l == "feat then tracks origin/feat"));
    ok(&r, push.clone());
    assert_eq!(git_out(&remote, &["rev-parse", "feat"]), r.head());
    assert_eq!(
        git_out(&r, &["rev-parse", "--abbrev-ref", "feat@{upstream}"]),
        "origin/feat"
    );
    let e = err(
        &r,
        Action::Push {
            remote: Some("elsewhere".into()),
            force: false,
        },
    );
    assert!(e.message.contains("no remote"), "{e:?}");

    // Tags.
    r.tag("v1", Some("first"));
    ok(
        &r,
        Action::PushTag {
            tag: "v1".into(),
            remote: "origin".into(),
        },
    );
    assert_eq!(
        git_out(&remote, &["rev-parse", "v1"]),
        git_out(&r, &["rev-parse", "v1"])
    );
    let e = err(
        &r,
        Action::PushTag {
            tag: "v2".into(),
            remote: "origin".into(),
        },
    );
    assert!(e.message.contains("no tag"), "{e:?}");

    // Detached HEAD: nothing to push or pull.
    r.git(&["checkout", "-q", "--detach"]);
    let e = err(&r, push);
    assert!(e.message.contains("not on a branch"), "{e:?}");
}

#[test]
fn a_force_push_is_leased_on_what_the_preview_saw() {
    let (r, remote) = ahead_behind();
    let url = remote.path().to_str().unwrap().to_string();
    let force = Action::Push {
        remote: None,
        force: true,
    };
    let p = preview(&repo(&r), &force, false).unwrap();

    // Someone else pushes after the preview; this repository has not fetched it.
    let other = FixtureRepo::new();
    other.git(&["fetch", "-q", &url, "main"]);
    other.git(&["reset", "-q", "--hard", "FETCH_HEAD"]);
    other.commit_file("late.txt", "late\n", "late work");
    other.git(&["push", "-q", &url, "HEAD:main"]);
    let late = other.head();

    let e = run_logged(&r, &force, Some(&p.token), &OpLog::new()).unwrap_err();
    assert!(e.message.contains("changed since your last fetch"), "{e:?}");
    assert!(e.detail.unwrap().contains("(stale info)"));
    assert_eq!(
        git_out(&remote, &["rev-parse", "main"]),
        late,
        "the late work is kept"
    );

    // After a fetch the new preview names it, and the push goes through.
    ok(&r, Action::Fetch { remote: None });
    let p = preview(&repo(&r), &force, false).unwrap();
    assert!(p.impact[0].contains("2 commits"), "{:?}", p.impact);
    ok(&r, force);
    assert_eq!(git_out(&remote, &["rev-parse", "main"]), r.head());
}

#[test]
fn a_conflicting_pull_stops_mid_merge() {
    let (r, _remote) = ahead_behind();
    // The remote added other.txt; add a different one here.
    r.commit_file("other.txt", "mine\n", "my other");
    let out = ok(&r, Action::Pull { rebase: false });
    assert_eq!(out.conflicts, vec!["other.txt".to_string()]);
    let e = err(&r, Action::Pull { rebase: false });
    assert!(e.message.contains("merge is in progress"), "{e:?}");
    ok(&r, Action::AbortMerge);

    // With rebase, it stops on the commit that conflicts, and Abort puts main back.
    let head = r.head();
    let out = ok(&r, Action::Pull { rebase: true });
    assert_eq!(out.conflicts, vec!["other.txt".to_string()]);
    ok(&r, Action::AbortRebase);
    assert_eq!(r.head(), head);
}
