//! Backup and restore against fixture repos. Each test captures the full repository state,
//! does damage, restores, and checks the state is identical.

use std::collections::BTreeMap;
use std::path::Path;

use super::*;
use crate::testing::FixtureRepo;
use crate::testing::shapes::*;

/// Everything a restore must bring back: HEAD, branch and tag tips, `git status`, and the bytes
/// of every file outside `.git`.
#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    head_ref: String,
    head: String,
    refs: String,
    status: String,
    files: BTreeMap<String, Vec<u8>>,
}

fn fingerprint(r: &FixtureRepo) -> Fingerprint {
    let head_ref = r
        .command(r.path(), &["symbolic-ref", "-q", "HEAD"])
        .output()
        .unwrap()
        .stdout_str();
    let head = r
        .command(r.path(), &["rev-parse", "-q", "--verify", "HEAD"])
        .output()
        .unwrap()
        .stdout_str();
    let refs = r.git(&[
        "for-each-ref",
        "--format=%(objectname) %(refname)",
        "refs/heads",
        "refs/tags",
    ]);
    let status = r.git(&["status", "--porcelain=v1", "-uall"]);
    let mut files = BTreeMap::new();
    collect(r.path(), r.path(), &mut files);
    Fingerprint {
        head_ref,
        head,
        refs,
        status,
        files,
    }
}

fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.file_name().unwrap() == ".git" {
            continue;
        }
        if e.file_type().unwrap().is_dir() {
            collect(root, &p, out);
        } else {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel, std::fs::read(&p).unwrap());
        }
    }
}

/// A repo with every kind of local change: staged, unstaged, deleted, untracked (including in
/// a new folder), and an ignored file.
fn messy() -> FixtureRepo {
    let r = linear(3);
    r.write(".gitignore", "*.log\n");
    r.write("keep.txt", "keep\n");
    r.write("delete-me.txt", "bye\n");
    r.commit_all("more files");
    r.write("file.txt", "staged edit\n");
    r.git(&["add", "file.txt"]);
    r.write("file.txt", "staged edit\nplus unstaged\n");
    r.write("keep.txt", "unstaged only\n");
    r.remove("delete-me.txt");
    r.write("new/untracked.txt", "u\n");
    r.write("build.log", "ignored\n");
    r
}

fn backups(r: &FixtureRepo) -> Backups {
    Backups::open(r.path()).unwrap()
}

#[test]
fn restore_after_hard_reset_and_clean() {
    let r = messy();
    let before = fingerprint(&r);
    let b = backups(&r);
    let rec = b.create("reset-hard", &BackupOptions::default()).unwrap();
    assert_eq!(rec.untracked, ["new/untracked.txt"]);
    assert!(!rec.conflicted);
    assert_eq!(
        fingerprint(&r),
        before,
        "taking a backup must not change anything"
    );

    // Destroy local work and move the branch back.
    r.git(&["reset", "-q", "--hard", "HEAD~2"]);
    r.git(&["clean", "-q", "-fd", "-e", "build.log"]);
    assert_ne!(fingerprint(&r), before);

    let out = b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(fingerprint(&r), before);
    // The ignored file was never touched.
    assert_eq!(r.read("build.log"), "ignored\n");
}

#[test]
fn preview_says_what_a_restore_would_change_and_changes_nothing() {
    let r = messy();
    let b = backups(&r);
    let rec = b.create("reset-hard", &BackupOptions::default()).unwrap();
    let head = r.git(&["rev-parse", "HEAD"]).trim().to_string();
    // Nothing moved yet: only the dirty files are reported.
    let p = b.preview(&rec.id).unwrap();
    assert!(p.moves.is_empty(), "{:?}", p.moves);
    assert_eq!(p.head_now.as_deref(), Some("main"));
    assert_eq!(p.untracked_then, 1);
    assert!(p.changed_now >= 3, "{}", p.changed_now);

    r.git(&["reset", "-q", "--hard", "HEAD~1"]);
    let before = fingerprint(&r);
    let p = b.preview(&rec.id).unwrap();
    assert_eq!(p.moves.len(), 1);
    assert_eq!(
        (p.moves[0].name.as_str(), p.moves[0].kind.as_str()),
        ("main", "branch")
    );
    assert_eq!(p.moves[0].to, head);
    assert!(p.moves[0].from.as_deref().is_some_and(|f| f != head));
    assert_eq!(fingerprint(&r), before, "a preview changes nothing");
}

#[test]
fn restore_is_itself_undoable() {
    let r = messy();
    let b = backups(&r);
    let rec = b.create("reset-hard", &BackupOptions::default()).unwrap();
    r.git(&["reset", "-q", "--hard", "HEAD~1"]);
    r.write("after.txt", "made after the reset\n");
    let damaged = fingerprint(&r);

    let out = b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert_eq!(out.pre_restore.action, "restore");
    b.restore(&out.pre_restore.id, &BackupOptions::default())
        .unwrap();
    assert_eq!(fingerprint(&r), damaged);
}

#[test]
fn deleted_branch_and_tag_come_back() {
    let r = many_branches(3);
    r.tag("annotated", Some("note"));
    let before = fingerprint(&r);
    let b = backups(&r);
    let rec = b
        .create("branch-delete", &BackupOptions::default())
        .unwrap();
    r.git(&["branch", "-D", "feature/1"]);
    r.git(&["tag", "-d", "annotated"]);
    r.git(&["tag", "-d", "v0.1.0"]);
    b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert_eq!(fingerprint(&r), before);
    // Annotated tags come back as tag objects, not lightweight tags.
    assert_eq!(r.git(&["cat-file", "-t", "annotated"]), "tag");
}

#[test]
fn detached_head_is_restored() {
    let r = linear(4);
    r.git(&["checkout", "-q", "--detach", "HEAD~2"]);
    r.write("file.txt", "detached edit\n");
    let before = fingerprint(&r);
    let b = backups(&r);
    let rec = b.create("checkout", &BackupOptions::default()).unwrap();
    assert_eq!(rec.head_branch, None);
    r.git(&["checkout", "-q", "-f", "main"]);
    b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert_eq!(fingerprint(&r), before);
}

#[test]
fn unborn_repo_with_only_untracked_files() {
    let r = FixtureRepo::new();
    r.write("a.txt", "a\n");
    r.write("dir/b.txt", "b\n");
    let before = fingerprint(&r);
    let b = backups(&r);
    let rec = b.create("clean", &BackupOptions::default()).unwrap();
    assert_eq!(rec.head_commit, None);
    r.git(&["clean", "-q", "-fd"]);
    assert!(fingerprint(&r).files.is_empty());
    b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert_eq!(fingerprint(&r), before);
}

#[test]
fn conflicted_merge_is_restored_mid_merge() {
    let r = conflict();
    // A file merged cleanly (staged by the merge) next to the conflicted one.
    r.checkout("topic");
    r.commit_file("clean.txt", "from topic\n", "topic adds a file");
    r.checkout("main");
    start_conflict(&r);
    let git_dir = r.path().join(".git");
    let state = |r: &FixtureRepo| {
        (
            r.git(&["ls-files", "-s"]),
            r.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
            r.read("conflict.txt"),
            std::fs::read_to_string(git_dir.join("MERGE_HEAD")).ok(),
            std::fs::read_to_string(git_dir.join("MERGE_MSG")).ok(),
        )
    };
    let before = state(&r);
    assert!(before.0.contains(" 3\tconflict.txt"), "{}", before.0);
    assert!(before.1.contains("A  clean.txt"), "{}", before.1);

    let b = backups(&r);
    // With a bundle: it must also carry the refs that keep the conflicted blobs.
    let opts = BackupOptions {
        bundle_dir: Some(r.sibling("bundles")),
    };
    let rec = b.create("abort-merge", &opts).unwrap();
    assert!(rec.bundle.is_some());
    assert!(rec.conflicted);
    assert_eq!(rec.conflicts.len(), 3, "{:?}", rec.conflicts);
    assert!(rec.index_commit.is_some());
    assert_eq!(rec.pending.len(), 1);
    assert_eq!(rec.pending[0].name, "MERGE_HEAD");
    assert_eq!(rec.pending[0].commit, r.rev("topic"));

    r.git(&["merge", "--abort"]);
    assert!(!git_dir.join("MERGE_HEAD").exists());
    r.git(&["gc", "-q", "--prune=now"]);

    let out = b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(state(&r), before);
    let s = crate::git::state::read_state(r.path(), true).unwrap();
    assert_eq!(s.in_progress, Some(crate::git::state::Operation::Merge));
    // The restored merge can be finished like the original.
    r.write("conflict.txt", "resolved\n");
    r.git(&["add", "conflict.txt"]);
    r.git(&["commit", "-q", "--no-edit"]);
    assert_eq!(r.parents("HEAD").len(), 2);
}

#[test]
fn cherry_pick_in_progress_is_restored() {
    let r = conflict();
    let out = r
        .command(r.path(), &["cherry-pick", "topic"])
        .output()
        .unwrap();
    assert!(!out.success(), "fixture cherry-pick should conflict");
    let pick = r.rev("CHERRY_PICK_HEAD");
    let b = backups(&r);
    let rec = b.create("abort", &BackupOptions::default()).unwrap();
    assert_eq!(rec.pending.len(), 1);
    assert_eq!(rec.pending[0].name, "CHERRY_PICK_HEAD");
    let index = r.git(&["ls-files", "-s"]);
    r.git(&["cherry-pick", "--abort"]);
    b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert_eq!(r.rev("CHERRY_PICK_HEAD"), pick);
    assert_eq!(r.git(&["ls-files", "-s"]), index);
}

#[test]
fn rebase_in_progress_is_stopped_before_restore() {
    let r = conflict();
    let b = backups(&r);
    let rec = b.create("rebase", &BackupOptions::default()).unwrap();
    let before = fingerprint(&r);
    let out = r.command(r.path(), &["rebase", "topic"]).output().unwrap();
    assert!(!out.success(), "fixture rebase should conflict");
    let state = crate::git::state::read_state(r.path(), false).unwrap();
    assert_eq!(
        state.in_progress,
        Some(crate::git::state::Operation::Rebase)
    );

    let out = b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert!(
        out.warnings.iter().any(|w| w.contains("rebase")),
        "{:?}",
        out.warnings
    );
    let state = crate::git::state::read_state(r.path(), false).unwrap();
    assert_eq!(state.in_progress, None);
    assert_eq!(fingerprint(&r), before);
}

#[test]
fn bundle_restores_after_refs_are_gone() {
    let r = messy();
    let before = fingerprint(&r);
    let bundle_dir = r.sibling("bundles");
    let opts = BackupOptions {
        bundle_dir: Some(bundle_dir.clone()),
    };
    let b = backups(&r);
    let rec = b.create("history-rewrite", &opts).unwrap();
    let bundle = rec.bundle.clone().unwrap();
    assert!(bundle.starts_with(&bundle_dir) && bundle.is_file());

    // Simulate losing the refs (and the objects only they kept alive).
    r.git(&["reset", "-q", "--hard", "HEAD~3"]);
    r.git(&["clean", "-q", "-fd", "-e", "build.log"]);
    b.delete_refs(&rec.id).unwrap();
    r.git(&["reflog", "expire", "--expire=now", "--all"]);
    r.git(&["gc", "-q", "--prune=now"]);
    assert!(b.restore(&rec.id, &BackupOptions::default()).is_err());

    b.import_bundle(&rec.id).unwrap();
    b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert_eq!(fingerprint(&r), before);
}

#[test]
fn backup_refs_survive_gc() {
    let r = messy();
    let b = backups(&r);
    let rec = b.create("reset-hard", &BackupOptions::default()).unwrap();
    r.git(&["reset", "-q", "--hard", "HEAD~2"]);
    r.git(&["reflog", "expire", "--expire=now", "--all"]);
    r.git(&["gc", "-q", "--prune=now"]);
    b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert_eq!(r.read("new/untracked.txt"), "u\n");
}

#[test]
fn failed_backup_records_nothing() {
    let r = messy();
    let b = backups(&r);
    // A bundle "folder" that is actually a file makes the backup fail.
    let not_a_dir = r.sibling("file-not-dir");
    std::fs::write(&not_a_dir, "x").unwrap();
    let err = b
        .create(
            "reset-hard",
            &BackupOptions {
                bundle_dir: Some(not_a_dir),
            },
        )
        .unwrap_err();
    assert!(matches!(err, BackupError::Io { .. }), "{err:?}");
    assert!(b.list().unwrap().is_empty());
    assert_eq!(r.git(&["for-each-ref", REF_PREFIX]), "");
}

#[test]
fn action_names_are_validated() {
    let r = linear(1);
    let b = backups(&r);
    for bad in ["", "Reset", "a b", "../x", "-x", "x/y", &"a".repeat(41)] {
        assert!(
            matches!(
                b.create(bad, &BackupOptions::default()),
                Err(BackupError::BadAction(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn list_ids_and_delete() {
    let r = linear(2);
    let b = backups(&r);
    let a = b.create("reset-hard", &BackupOptions::default()).unwrap();
    let c = b.create("reset-hard", &BackupOptions::default()).unwrap();
    assert_ne!(a.id, c.id, "ids are unique even within one second");
    assert_eq!(b.list().unwrap().len(), 2);
    b.delete(&a.id).unwrap();
    let left = b.list().unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, c.id);
    assert_eq!(
        r.git(&[
            "for-each-ref",
            "--format=%(refname)",
            &format!("{REF_PREFIX}{}/", a.id)
        ]),
        ""
    );
    assert!(matches!(b.delete(&a.id), Err(BackupError::NotFound(_))));
}

#[test]
fn retention_by_count_and_age() {
    let r = linear(1);
    let b = backups(&r);
    for _ in 0..5 {
        b.create("reset-hard", &BackupOptions::default()).unwrap();
    }
    // Age the three oldest by rewriting the index.
    let mut index = b.read_index().unwrap();
    index.backups.sort_by_key(|x| x.id.clone());
    let now = unix_now();
    for (i, rec) in index.backups.iter_mut().enumerate() {
        rec.created = now - (5 - i as u64) * 10 * 86_400; // 50, 40, 30, 20, 10 days old
    }
    b.write_index(&index).unwrap();

    let deleted = b
        .prune(
            Retention {
                max_count: None,
                max_age_days: Some(35),
            },
            now,
        )
        .unwrap();
    assert_eq!(deleted.len(), 2);
    let deleted = b
        .prune(
            Retention {
                max_count: Some(2),
                max_age_days: None,
            },
            now,
        )
        .unwrap();
    assert_eq!(deleted.len(), 1);
    let left = b.list().unwrap();
    assert_eq!(left.len(), 2);
    assert!(left[0].created > left[1].created);
}

#[test]
fn cleanup_preview_lists_what_cleanup_deletes() {
    let r = linear(1);
    let b = backups(&r);
    let mut ids = Vec::new();
    for i in 0..4 {
        ids.push(
            b.create("reset-hard", &BackupOptions::default())
                .unwrap()
                .id,
        );
        // Age the newest so the list is not all "now".
        let mut index = b.read_index().unwrap();
        if let Some(rec) = index
            .backups
            .iter_mut()
            .find(|x| x.id == *ids.last().unwrap())
        {
            rec.created = unix_now() - (4 - i as u64) * 86_400;
        }
        b.write_index(&index).unwrap();
    }
    let now = unix_now();
    let policy = Retention {
        max_count: Some(2),
        max_age_days: None,
    };
    let preview = b.cleanup_preview(policy, now).unwrap();
    assert_eq!(
        preview.len(),
        2,
        "the two oldest sit beyond the count limit"
    );
    assert!(preview.iter().all(|b| ids.contains(&b.id)));
    // Nothing deleted by the preview.
    assert_eq!(b.list().unwrap().len(), 4);
    let deleted = b.cleanup(policy, now).unwrap();
    assert_eq!(deleted.len(), 2);
    assert_eq!(b.list().unwrap().len(), 2);
    // Cleanup on an empty or fully-inside-limits list deletes nothing.
    let again = b.cleanup(policy, now).unwrap();
    assert!(again.is_empty());
}

#[test]
fn lfs_files_round_trip() {
    let Some(r) = lfs() else {
        eprintln!("skipped: git-lfs not installed");
        return;
    };
    let lfs_files: Vec<String> = r
        .git(&["lfs", "ls-files", "-n"])
        .lines()
        .map(String::from)
        .collect();
    let file = lfs_files.first().expect("fixture has an LFS file").clone();
    r.write(&file, b"changed binary \x00\x01");
    r.write("assets/new.bin", b"untracked lfs \x02");
    let before = fingerprint(&r);
    let b = backups(&r);
    let rec = b.create("reset-hard", &BackupOptions::default()).unwrap();
    assert!(rec.lfs);
    r.git(&["reset", "-q", "--hard"]);
    r.git(&["clean", "-q", "-fd"]);
    b.restore(&rec.id, &BackupOptions::default()).unwrap();
    assert_eq!(fingerprint(&r), before);
    // git-lfs installs hooks from its filters; none may land in the hooks path.
    let no_hooks = r.path().join(".git/pm/no-hooks");
    assert!(std::fs::symlink_metadata(&no_hooks).unwrap().is_file());
    assert_eq!(std::fs::read(&no_hooks).unwrap(), b"");
}

/// Security: taking and restoring a backup must not run any repository hook, whether from
/// `.git/hooks` or from a `core.hooksPath` the repository sets.
#[cfg(unix)]
#[test]
fn backups_never_run_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let r = messy();
    let marker = r.sibling("hooks-ran");
    let custom = r.sibling("custom-hooks");
    let names = [
        "reference-transaction",
        "post-checkout",
        "post-index-change",
        "pre-commit",
        "post-commit",
        "post-rewrite",
        "pre-auto-gc",
    ];
    for dir in [r.path().join(".git/hooks"), custom.clone()] {
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            let hook = dir.join(name);
            let script = format!("#!/bin/sh\necho {name} >> '{}'\n", marker.display());
            std::fs::write(&hook, script).unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let round_trip = || {
        let b = backups(&r);
        let rec = b.create("reset-hard", &BackupOptions::default()).unwrap();
        r.git(&["reset", "-q", "--hard", "HEAD~1"]);
        b.restore(&rec.id, &BackupOptions::default()).unwrap();
        assert!(
            !marker.exists(),
            "hooks ran: {:?}",
            std::fs::read_to_string(&marker)
        );
    };
    // Control: a plain git command runs the hook, so the hooks are live.
    let control = |what: &str| {
        GitCommand::new(r.path())
            .args(["update-ref", "refs/heads/hook-check", "HEAD"])
            .run()
            .unwrap();
        assert!(marker.exists(), "control: {what} hooks did not run");
        std::fs::remove_file(&marker).unwrap();
    };

    round_trip();
    control(".git/hooks");
    r.git(&["config", "core.hooksPath", custom.to_str().unwrap()]);
    round_trip();
    control("core.hooksPath");
}

#[test]
fn a_directory_at_the_no_hooks_path_is_refused() {
    let r = linear(2);
    std::fs::create_dir_all(r.path().join(".git/pm/no-hooks")).unwrap();
    assert!(Backups::open(r.path()).is_err());
}

/// Security: a restore must not write or delete through a symlink that points outside the repo.
#[cfg(unix)]
#[test]
fn safe_join_refuses_escapes() {
    let r = linear(1);
    let outside = r.sibling("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, r.path().join("link")).unwrap();
    assert!(matches!(
        safe_join(r.path(), "link/victim.txt"),
        Err(BackupError::UnsafePath { .. })
    ));
    for bad in ["../x", "/etc/passwd", "a/../../x", ""] {
        assert!(safe_join(r.path(), bad).is_err(), "{bad}");
    }
    assert_eq!(
        safe_join(r.path(), "a/b.txt").unwrap(),
        r.path().join("a/b.txt")
    );
    // The link itself (last component) may be removed; it is not followed.
    assert!(safe_join(r.path(), "link").is_ok());
}

#[test]
fn utc_formatting() {
    assert_eq!(format_utc(0), "19700101-000000");
    assert_eq!(format_utc(1_577_836_800), "20200101-000000");
    assert_eq!(format_utc(1_709_210_096), "20240229-123456");
}
