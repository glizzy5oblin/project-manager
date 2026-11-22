use super::*;
use crate::testing::FixtureRepo;

fn repo() -> FixtureRepo {
    let r = FixtureRepo::new();
    r.write("b.txt", "b\n");
    r.write("A.txt", "a\n");
    r.write("src/main.rs", "fn main() {}\n");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "c"]);
    r
}

#[test]
fn lists_folders_first_then_files_and_hides_git() {
    let r = repo();
    let list = list_dir(r.path(), "").unwrap();
    let names: Vec<_> = list.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["src", "A.txt", "b.txt"]);
    let inner = list_dir(r.path(), "src").unwrap();
    assert_eq!(inner.entries[0].path, "src/main.rs");
    assert!(!inner.truncated);
}

#[test]
fn reads_text_and_says_why_it_cannot() {
    let r = repo();
    assert_eq!(
        read_text(r.path(), "b.txt").unwrap().text.as_deref(),
        Some("b\n")
    );
    std::fs::write(r.path().join("bin"), [0u8, 1, 2]).unwrap();
    assert_eq!(
        read_text(r.path(), "bin").unwrap().reason.as_deref(),
        Some("binary")
    );
    std::fs::write(r.path().join("latin"), [0xffu8, 0xfe, b'a']).unwrap();
    assert_eq!(
        read_text(r.path(), "latin").unwrap().reason.as_deref(),
        Some("encoding")
    );
    std::fs::write(r.path().join("big"), vec![b'a'; MAX_BYTES as usize + 1]).unwrap();
    assert_eq!(
        read_text(r.path(), "big").unwrap().reason.as_deref(),
        Some("large")
    );
    assert!(read_text(r.path(), "src").is_err());
}

#[test]
fn writes_a_file_and_creates_a_new_one() {
    let r = repo();
    write_text(r.path(), "b.txt", "changed\n").unwrap();
    assert_eq!(r.read("b.txt"), "changed\n");
    write_text(r.path(), "src/new.rs", "x").unwrap();
    assert_eq!(r.read("src/new.rs"), "x");
    assert!(write_text(r.path(), "src", "x").is_err());
}

#[test]
fn refuses_paths_that_leave_the_project() {
    let r = repo();
    assert!(read_text(r.path(), "../x").is_err());
    assert!(write_text(r.path(), ".git/config", "x").is_err());
    assert!(list_dir(r.path(), "/etc").is_err());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path(), r.path().join("link")).unwrap();
        assert!(read_text(r.path(), "link/secret").is_err());
        assert!(write_text(r.path(), "link/new", "x").is_err());
        assert!(list_dir(r.path(), "link").is_err());
    }
}

#[test]
fn creates_files_and_folders_but_never_replaces() {
    let r = repo();
    create(r.path(), "docs/notes/todo.md", false).unwrap();
    assert_eq!(r.read("docs/notes/todo.md"), "");
    create(r.path(), "empty/dir", true).unwrap();
    assert!(r.path().join("empty/dir").is_dir());
    assert!(create(r.path(), "b.txt", false).is_err());
    assert!(create(r.path(), "src", true).is_err());
    assert!(create(r.path(), "b.txt/inner", false).is_err());
    assert!(create(r.path(), "", false).is_err());
    assert!(create(r.path(), "../out", false).is_err());
    assert!(create(r.path(), ".git/hook", false).is_err());
    assert_eq!(r.read("b.txt"), "b\n");
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), r.path().join("link")).unwrap();
        assert!(create(r.path(), "link/new/x", false).is_err());
        assert!(!outside.path().join("new").exists());
    }
}
