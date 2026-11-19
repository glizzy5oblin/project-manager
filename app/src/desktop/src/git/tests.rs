//! Repository-state reads (gix) checked against fixture repos and the git CLI.

use super::state::{ChangeKind, Operation, read_state};
use crate::testing::FixtureRepo;
use crate::testing::shapes::*;

#[test]
fn ahead_behind_matches_git() {
    let (clone, _remote) = ahead_behind();
    let s = read_state(clone.path(), true).unwrap();
    let main = s.branches.iter().find(|b| b.name == "main").unwrap();
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (Some(2), Some(1)));
    let cli = clone.git(&["rev-list", "--left-right", "--count", "main...origin/main"]);
    assert_eq!(cli.replace('\t', " "), "2 1");
    assert!(main.is_head);
    assert_eq!(s.head.branch.as_deref(), Some("main"));
    assert_eq!(s.head.commit.as_deref(), Some(clone.head().as_str()));
    assert!(!s.head.detached);
    assert_eq!(s.remote_branches.len(), 1);
    assert_eq!(s.remote_branches[0].name, "origin/main");
    assert_eq!(s.remotes.len(), 1);
    assert_eq!(s.remotes[0].name, "origin");
    assert!(!s.status.unwrap().is_dirty());
    assert!(s.trusted);
}

#[test]
fn branch_without_upstream_has_no_counts() {
    let r = linear(2);
    let s = read_state(r.path(), false).unwrap();
    assert_eq!(s.branches.len(), 1);
    assert_eq!(s.branches[0].upstream, None);
    assert_eq!(s.branches[0].ahead, None);
    assert!(s.status.is_none());
}

#[test]
fn branches_and_tags_match_git() {
    let r = many_branches(4);
    r.tag("light", None);
    let s = read_state(r.path(), false).unwrap();
    let mut names: Vec<_> = s.branches.iter().map(|b| b.name.clone()).collect();
    names.sort();
    let mut cli: Vec<String> = r
        .git(&["branch", "--format=%(refname:short)"])
        .lines()
        .map(String::from)
        .collect();
    cli.sort();
    assert_eq!(names, cli);
    for b in &s.branches {
        assert_eq!(b.commit, r.rev(&b.name), "{}", b.name);
    }
    let lightweight = s.tags.iter().find(|t| t.name == "light").unwrap();
    let annotated = s.tags.iter().find(|t| t.name == "v0.1.0").unwrap();
    assert!(!lightweight.annotated);
    assert!(annotated.annotated);
    assert_eq!(annotated.commit, r.rev("v0.1.0^{commit}"));
}

#[test]
fn status_lists_each_kind() {
    let r = FixtureRepo::new();
    r.write("keep.txt", "1\n");
    r.write("change.txt", "1\n");
    r.write("gone.txt", "1\n");
    r.write("staged-change.txt", "1\n");
    r.commit_all("base");
    r.write("change.txt", "2\n");
    r.remove("gone.txt");
    r.write("staged-change.txt", "2\n");
    r.write("new-staged.txt", "x\n");
    r.git(&["add", "staged-change.txt", "new-staged.txt"]);
    r.write("dir/untracked.txt", "u\n");

    let s = read_state(r.path(), true).unwrap().status.unwrap();
    let staged: Vec<_> = s.staged.iter().map(|c| (c.path.as_str(), c.kind)).collect();
    assert_eq!(
        staged,
        [
            ("new-staged.txt", ChangeKind::Added),
            ("staged-change.txt", ChangeKind::Modified)
        ]
    );
    let unstaged: Vec<_> = s
        .unstaged
        .iter()
        .map(|c| (c.path.as_str(), c.kind))
        .collect();
    assert_eq!(
        unstaged,
        [
            ("change.txt", ChangeKind::Modified),
            ("gone.txt", ChangeKind::Deleted)
        ]
    );
    assert_eq!(s.untracked, ["dir/untracked.txt"]);
    assert!(s.conflicted.is_empty());
    assert!(s.is_dirty());
}

#[test]
fn conflict_is_reported_once_with_operation() {
    let r = conflict();
    start_conflict(&r);
    let s = read_state(r.path(), true).unwrap();
    assert_eq!(s.in_progress, Some(Operation::Merge));
    let st = s.status.unwrap();
    assert_eq!(st.conflicted, ["conflict.txt"]);
    assert!(st.staged.iter().all(|c| c.path != "conflict.txt"));
    assert!(st.unstaged.iter().all(|c| c.path != "conflict.txt"));
}

#[test]
fn unborn_and_detached_heads() {
    let r = FixtureRepo::new();
    let s = read_state(r.path(), true).unwrap();
    assert_eq!(s.head.branch.as_deref(), Some("main"));
    assert_eq!(s.head.commit, None);
    assert!(s.branches.is_empty());

    let r = linear(3);
    r.git(&["checkout", "-q", "--detach", "HEAD~1"]);
    let s = read_state(r.path(), false).unwrap();
    assert!(s.head.detached);
    assert_eq!(s.head.branch, None);
    assert_eq!(s.head.commit.as_deref(), Some(r.rev("HEAD").as_str()));
    assert!(s.branches.iter().all(|b| !b.is_head));
}

#[test]
fn bare_repo_has_no_status() {
    let r = FixtureRepo::new_with(&["--bare"]);
    let s = read_state(r.path(), true).unwrap();
    assert!(s.bare);
    assert!(s.status.is_none());
}

#[test]
fn does_not_discover_parent_repo() {
    let r = linear(1);
    std::fs::create_dir_all(r.path().join("sub")).unwrap();
    assert!(read_state(&r.path().join("sub"), false).is_err());
}

#[test]
fn submodule_file_is_detected() {
    let r = linear(1);
    r.write(
        ".gitmodules",
        "[submodule \"x\"]\n\tpath = x\n\turl = ../x\n",
    );
    assert!(read_state(r.path(), false).unwrap().has_submodules);
}

/// Security: credentials in a remote URL never leave the backend.
#[test]
fn remote_url_credentials_are_redacted() {
    let r = linear(1);
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://user:ghp_secret123@github.com/a/b.git",
    ]);
    let s = read_state(r.path(), false).unwrap();
    let url = s.remotes[0].fetch_url.as_deref().unwrap();
    assert_eq!(url, "https://***@github.com/a/b.git");
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains("ghp_secret123"));
}

#[test]
fn large_history_ahead_behind_is_quick() {
    let r = large_history(5000);
    r.git(&["branch", "old", "main~4000"]);
    r.git(&["branch", "-q", "--set-upstream-to=old", "main"]);
    let started = std::time::Instant::now();
    let s = read_state(r.path(), false).unwrap();
    let main = s.branches.iter().find(|b| b.name == "main").unwrap();
    assert_eq!((main.ahead, main.behind), (Some(4000), Some(0)));
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
}
