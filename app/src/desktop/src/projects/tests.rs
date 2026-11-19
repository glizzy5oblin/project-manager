use std::sync::{Mutex, mpsc};
use std::time::Duration;

use super::*;
use crate::ipc::{EventSink, TaskEvent, TaskManager, TaskOutcome};
use crate::testing::FixtureRepo;
use crate::testing::shapes::*;

const PROJ: &str = r#"schema_version = 1

[project]
name = "Example Mod"
version = "1.2.0"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"
"#;

fn with_proj(r: &FixtureRepo) {
    r.write("example.proj", PROJ);
    r.commit_all("add proj");
}

fn registry() -> (tempfile::TempDir, Registry) {
    let dir = tempfile::tempdir().unwrap();
    let reg = Registry::load(dir.path().join("projects.json"));
    (dir, reg)
}

#[test]
fn add_list_remove_and_persist() {
    let r = linear(1);
    with_proj(&r);
    let (dir, mut reg) = registry();
    let e = reg.add(r.path(), None, 100).unwrap();
    assert_eq!(e.name, "Example Mod");
    assert_eq!(e.path, canonical(r.path()));
    // Adding again (through a different spelling of the path) returns the same entry.
    let again = reg.add(&r.path().join("."), None, 200).unwrap();
    assert_eq!(again, e);
    assert_eq!(reg.list().len(), 1);

    let reloaded = Registry::load(dir.path().join("projects.json"));
    assert_eq!(reloaded.list(), std::slice::from_ref(&e));

    reg.remove(&e.id).unwrap();
    assert!(reg.list().is_empty());
    assert!(
        r.path().join("example.proj").exists(),
        "files are never deleted"
    );
    assert_eq!(reg.remove(&e.id).unwrap_err().kind, ErrorKind::NotFound);
    // Ids are never reused.
    let e2 = reg.add(r.path(), None, 300).unwrap();
    assert_ne!(e2.id, e.id);
}

#[test]
fn add_rejects_non_projects() {
    let (_d, mut reg) = registry();
    let plain = tempfile::tempdir().unwrap();
    assert_eq!(
        reg.add(plain.path(), None, 0).unwrap_err().kind,
        ErrorKind::NotARepo
    );
    let r = linear(1);
    std::fs::create_dir(r.path().join("sub")).unwrap();
    assert_eq!(
        reg.add(&r.path().join("sub"), None, 0).unwrap_err().kind,
        ErrorKind::NotARepo,
        "a subfolder is not the repository root"
    );
    assert_eq!(
        reg.add(r.path(), None, 0).unwrap_err().kind,
        ErrorKind::NoProjFile
    );
    r.write("bad.proj", "schema_version = 1\n[project]\n");
    assert_eq!(
        reg.add(r.path(), None, 0).unwrap_err().kind,
        ErrorKind::InvalidProject
    );
    let bare = FixtureRepo::new_with(&["--bare"]);
    assert_eq!(
        reg.add(bare.path(), None, 0).unwrap_err().kind,
        ErrorKind::InvalidInput
    );
    assert_eq!(
        reg.add(&plain.path().join("missing"), None, 0)
            .unwrap_err()
            .kind,
        ErrorKind::NotFound
    );
    assert!(reg.list().is_empty());
}

#[test]
fn add_can_write_a_starter_proj() {
    let r = linear(1);
    let (_d, mut reg) = registry();
    let e = reg.add(r.path(), Some(WorkflowMode::TwoRepo), 0).unwrap();
    let folder = r.path().file_name().unwrap().to_string_lossy().into_owned();
    let proj = ProjFile::load(&r.path().join(format!("{folder}.proj"))).unwrap();
    assert_eq!(e.name, folder);
    assert_eq!(
        proj.workflow.public.path.as_deref(),
        Some(format!("../{folder}-public").as_str())
    );
    // An existing .proj is never replaced.
    let r2 = linear(1);
    with_proj(&r2);
    reg.add(r2.path(), Some(WorkflowMode::TwoRepo), 0).unwrap();
    assert_eq!(
        std::fs::read_to_string(r2.path().join("example.proj")).unwrap(),
        PROJ
    );
}

#[test]
fn reorder_projects() {
    let (_d, mut reg) = registry();
    let repos: Vec<FixtureRepo> = (0..3).map(|_| linear(1)).collect();
    let ids: Vec<String> = repos
        .iter()
        .map(|r| {
            with_proj(r);
            reg.add(r.path(), None, 0).unwrap().id
        })
        .collect();
    reg.reorder(&ids[2], 0).unwrap();
    let order: Vec<_> = reg.list().iter().map(|p| p.id.clone()).collect();
    assert_eq!(order, [ids[2].clone(), ids[0].clone(), ids[1].clone()]);
    reg.reorder(&ids[2], 99).unwrap();
    assert_eq!(reg.list().last().unwrap().id, ids[2]);
}

#[test]
fn damaged_registry_is_kept_aside() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("projects.json");
    std::fs::write(&file, "{ not json").unwrap();
    let reg = Registry::load(file.clone());
    assert!(reg.list().is_empty());
    assert!(!file.exists());
    let aside: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(aside.len(), 1);
    assert!(aside[0].starts_with("projects.damaged-"), "{aside:?}");
}

#[test]
fn summary_matches_the_repository() {
    let (clone, _remote) = ahead_behind();
    with_proj(&clone); // a third local commit
    clone.write("dirty.txt", "x\n");
    clone.write("README.md", "changed\n");
    clone.git(&["add", "README.md"]);
    clone.tag("v1.1.0", None);
    clone.tag("v1.10.0", Some("newest"));
    clone.tag("v1.9.0", None);
    clone.tag("nightly", None);
    let (_d, mut reg) = registry();
    let e = reg.add(clone.path(), None, 0).unwrap();
    let s = summarize(&e);
    assert_eq!(s.name, "Example Mod");
    assert_eq!(s.version.as_deref(), Some("1.2.0"));
    assert_eq!(s.mode, Some(WorkflowMode::SingleRepo));
    assert_eq!(s.branch.as_deref(), Some("main"));
    assert_eq!(s.upstream.as_deref(), Some("origin/main"));
    assert_eq!((s.ahead, s.behind), (Some(3), Some(1)));
    assert_eq!((s.staged, s.unstaged, s.untracked), (1, 0, 1));
    assert_eq!(s.last_release.as_deref(), Some("v1.10.0"));
    assert!(s.trusted);
    assert_eq!((s.proj_error.clone(), s.repo_error.clone()), (None, None));
}

#[test]
fn summary_reports_problems_per_card() {
    let r = linear(1);
    with_proj(&r);
    let (_d, mut reg) = registry();
    let e = reg.add(r.path(), None, 0).unwrap();
    r.write("example.proj", "schema_version = 99\n");
    let s = summarize(&e);
    assert_eq!(s.proj_error.unwrap().kind, ErrorKind::InvalidProject);
    assert!(s.repo_error.is_none());
    assert_eq!(s.branch.as_deref(), Some("main"));

    let moved = ProjectEntry {
        path: r.sibling("gone"),
        ..e
    };
    let s = summarize(&moved);
    assert_eq!(s.repo_error.unwrap().kind, ErrorKind::NotFound);
}

#[test]
fn release_tag_ordering() {
    let tags = [
        "v0.9.0",
        "v0.10.0",
        "1.0.0-beta.1",
        "release",
        "v0.10.0-rc1",
    ];
    assert_eq!(
        last_release(tags.iter().copied()).as_deref(),
        Some("1.0.0-beta.1")
    );
    assert_eq!(last_release(["x", "y"].iter().copied()), None);
}

#[test]
fn session_is_sanitized_against_the_registry() {
    let r = linear(1);
    with_proj(&r);
    let (_d, mut reg) = registry();
    let e = reg.add(r.path(), None, 0).unwrap();
    let tab = |id: &str| TabState {
        project_id: id.into(),
        view: ProjectView::Graph,
    };
    let mut s = Session {
        tabs: vec![tab(&e.id), tab("p999"), tab(&e.id)],
        active: Some("p999".into()),
    };
    s.sanitize(&reg);
    assert_eq!(s.tabs, [tab(&e.id)]);
    assert_eq!(s.active, None);

    // Round trip, and old or partial files still load.
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("sub/session.json");
    write_json(&f, &s).unwrap();
    assert_eq!(read_json::<Session>(&f), s);
    std::fs::write(&f, r#"{"tabs":[{"projectId":"p1"}]}"#).unwrap();
    let s: Session = read_json(&f);
    assert_eq!(s.tabs[0].view, ProjectView::Overview);
    let settings: Settings = read_json(&dir.path().join("missing.json"));
    assert_eq!(settings.theme, "dark");
}

#[test]
fn project_names() {
    for ok in ["ExampleMod", "My Mod 2", "mod_v1.2", "Ünïcödé"] {
        validate_project_name(ok).unwrap();
    }
    for bad in [
        "",
        " lead",
        "trail ",
        ".hidden",
        "-x",
        "dot.",
        "a/b",
        "a\\b",
        "a:b",
        "a*b",
        "CON",
        "com1",
        "lpt9.txt",
        "nul.proj",
        "tab\there",
    ] {
        assert!(validate_project_name(bad).is_err(), "{bad:?}");
    }
    validate_project_name("CONSOLE").unwrap();
    assert!(validate_project_name(&"a".repeat(101)).is_err());

    assert_eq!(
        name_from_url("https://github.com/a/My-Mod.git").as_deref(),
        Some("My-Mod")
    );
    assert_eq!(
        name_from_url("git@github.com:a/b.git/").as_deref(),
        Some("b")
    );
    assert_eq!(name_from_url("/srv/repos/thing").as_deref(), Some("thing"));
    assert_eq!(name_from_url("https://host/.git"), None);
}

#[test]
fn create_both_modes() {
    let parent = tempfile::tempdir().unwrap();
    let root = create_project(parent.path(), "Two", WorkflowMode::TwoRepo).unwrap();
    assert_eq!(root, canonical(parent.path()).join("Two/private"));
    assert!(root.join("Two.proj").exists());
    let proj = inspect(&root).unwrap();
    assert_eq!(proj.project.name, "Two");
    assert_eq!(proj.workflow.public.path.as_deref(), Some("../public"));
    assert!(crate::git::state::open(&parent.path().join("Two/public")).is_ok());
    let s = crate::git::state::read_state(&root, false).unwrap();
    assert_eq!(s.head.branch.as_deref(), Some("main"));

    let root = create_project(parent.path(), "One", WorkflowMode::SingleRepo).unwrap();
    assert_eq!(
        inspect(&root).unwrap().workflow.mode,
        WorkflowMode::SingleRepo
    );
    assert_eq!(root, canonical(parent.path()).join("One/private"));
    assert!(!parent.path().join("One/public").exists());

    let (_d, mut reg) = registry();
    reg.add(&root, None, 0).unwrap();
}

#[test]
fn create_refuses_existing_and_leaves_nothing_on_failure() {
    let parent = tempfile::tempdir().unwrap();
    std::fs::create_dir(parent.path().join("Taken")).unwrap();
    assert_eq!(
        create_project(parent.path(), "Taken", WorkflowMode::SingleRepo)
            .unwrap_err()
            .kind,
        ErrorKind::AlreadyExists
    );
    // Only the project folder is checked: a leftover `public` elsewhere does not matter.
    std::fs::create_dir(parent.path().join("public")).unwrap();
    create_project(parent.path(), "New", WorkflowMode::TwoRepo).unwrap();
    assert!(create_project(parent.path(), "../escape", WorkflowMode::SingleRepo).is_err());
    assert!(!parent.path().parent().unwrap().join("escape").exists());
}

struct ChannelSink(Mutex<mpsc::Sender<TaskEvent>>);
impl EventSink for ChannelSink {
    fn emit(&self, event: &TaskEvent) {
        let _ = self.0.lock().unwrap().send(event.clone());
    }
}

fn run_clone(url: String, parent: PathBuf, name: Option<String>) -> TaskOutcome {
    let (tx, rx) = mpsc::channel();
    let tasks = TaskManager::new(ChannelSink(Mutex::new(tx)));
    let id = tasks
        .spawn("clone", "Cloning", None, move |ctx| {
            clone_project(&url, &parent, name.as_deref(), ctx)
        })
        .unwrap();
    loop {
        if let TaskEvent::Finished { id: done, outcome } =
            rx.recv_timeout(Duration::from_secs(30)).unwrap()
            && done == id
        {
            return outcome;
        }
    }
}

#[test]
fn clone_then_add() {
    let source = linear(2);
    with_proj(&source);
    let parent = tempfile::tempdir().unwrap();
    let url = format!("file://{}", source.path().display());
    let outcome = run_clone(
        url.clone(),
        parent.path().to_path_buf(),
        Some("Copy".into()),
    );
    let TaskOutcome::Ok { value } = outcome else {
        panic!("{outcome:?}")
    };
    let dest: PathBuf = serde_json::from_value(value).unwrap();
    assert_eq!(dest, canonical(parent.path()).join("Copy"));
    let (_d, mut reg) = registry();
    assert_eq!(reg.add(&dest, None, 0).unwrap().name, "Example Mod");

    // The destination exists now: refused, and the existing folder is untouched.
    let outcome = run_clone(url, parent.path().to_path_buf(), Some("Copy".into()));
    assert!(
        matches!(&outcome, TaskOutcome::Failed { error } if error.kind == ErrorKind::AlreadyExists),
        "{outcome:?}"
    );
    assert!(dest.join("example.proj").exists());
}

/// Security: dangerous URLs are refused before git runs, and a failed clone leaves no folder.
#[test]
fn clone_refuses_bad_urls_and_cleans_up() {
    let parent = tempfile::tempdir().unwrap();
    for bad in [
        "ext::sh -c touch% /tmp/pwned",
        "-uhttps://x/y",
        "https://user:token@github.com/a/b.git",
        "fd::17",
    ] {
        let outcome = run_clone(bad.into(), parent.path().to_path_buf(), Some("x".into()));
        assert!(
            matches!(&outcome, TaskOutcome::Failed { error } if error.kind == ErrorKind::InvalidInput),
            "{bad}: {outcome:?}"
        );
    }
    let missing = format!("file://{}", parent.path().join("nope").display());
    let outcome = run_clone(missing, parent.path().to_path_buf(), Some("x".into()));
    assert!(
        matches!(&outcome, TaskOutcome::Failed { error } if error.kind == ErrorKind::Git),
        "{outcome:?}"
    );
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
}

/// The form the registry stores: canonical, without Windows' `\\?\` prefix when not needed.
fn canonical(p: &std::path::Path) -> PathBuf {
    super::dunce_simplified(std::fs::canonicalize(p).unwrap())
}
