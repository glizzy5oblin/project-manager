//! Gate 2/3/4: a full release of a scratch repository, end to end.
//!
//! A real local repository: bump, commit, tag (annotated, history recorded), build the release
//! zip and generate the changelog. Everything is checked against the repository itself rather
//! than against the returned values.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::Digest;

use project_manager_lib::proj::ProjFile;
use project_manager_lib::release;

fn git_output(repo: &Path, args: &[&str]) -> std::process::Output {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "tester")
        .env("GIT_AUTHOR_EMAIL", "tester@test")
        .env("GIT_COMMITTER_NAME", "tester")
        .env("GIT_COMMITTER_EMAIL", "tester@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git(repo: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git_output(repo, args).stdout)
        .trim()
        .to_string()
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn init_repo(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    let out = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(&repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success());
    repo
}

const PROJ: &str = r#"schema_version = 1

[project]
name = "FullRelease"
version = "1.0.0"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"

[staging]
include = ["src/**", "README.md"]

[release]
version_files = ["manifest.json"]
"#;

#[test]
fn full_release_completes_against_a_real_repository() {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo(dir.path());

    write(&repo.join("example.proj"), PROJ);
    write(&repo.join("manifest.json"), "{\"version\": \"1.0.0\"}\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "feat: initial project"]);

    // The public branch carries the public files only.
    git(&repo, &["checkout", "-q", "-b", "public"]);
    write(&repo.join("src/index.js"), "// public\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "feat: public index"]);
    git(&repo, &["checkout", "-q", "main"]);

    let data_dir = dir.path().join("data");

    // --- bump --------------------------------------------------------------
    let preview = release::preview_bump(&repo, release::Bump::Minor, None).unwrap();
    assert_eq!(preview.current, "1.0.0");
    assert_eq!(preview.target, "1.1.0");

    let applied = release::apply_bump(&repo, release::Bump::Minor, None).unwrap();
    assert_eq!(applied.target, "1.1.0");

    let proj_after = ProjFile::load(&repo.join("example.proj")).unwrap();
    assert_eq!(proj_after.project.version, "1.1.0");
    assert!(
        std::fs::read_to_string(repo.join("manifest.json"))
            .unwrap()
            .contains("1.1.0")
    );

    // The bump is committed, as a user would before tagging.
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "feat: release 1.1.0"]);

    // --- tag (annotated) ----------------------------------------------------
    let tag = release::create_tag(&repo, false).unwrap();
    assert_eq!(tag.tag, "v1.1.0");
    assert!(!tag.pushed);
    assert_eq!(git(&repo, &["tag", "--list", "v*"]), "v1.1.0");

    // The history was recorded in the project file.
    let proj_after_tag = ProjFile::load(&repo.join("example.proj")).unwrap();
    assert_eq!(proj_after_tag.release.history.len(), 1);
    assert_eq!(proj_after_tag.release.history[0].tag, "v1.1.0");
    assert_eq!(proj_after_tag.release.history[0].version, "1.1.0");
    let file_text = std::fs::read_to_string(repo.join("example.proj")).unwrap();
    assert!(file_text.contains("[[release.history]]"), "{file_text}");

    // --- zip ----------------------------------------------------------------
    let outcome = release::build_release_zip(&repo, &data_dir).unwrap();
    assert_eq!(outcome.file_name, "FullRelease-1.1.0.zip");
    assert!(outcome.path.exists());

    // The zip contains the public branch's tracked files plus the manifest and the
    // project file. The public branch carries only the staged public files.
    let paths: Vec<&str> = outcome.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"src/index.js"), "{paths:?}");
    assert!(paths.contains(&"manifest.json"), "{paths:?}");

    // No allowlist violations: everything the zip carries is staged.
    assert!(outcome.violations.is_empty(), "{:?}", outcome.violations);

    // The checksum is over the zip as written.
    let zip_bytes = std::fs::read(&outcome.path).unwrap();
    assert_eq!(outcome.bytes, zip_bytes.len() as u64);
    assert_eq!(
        outcome.sha256,
        format!("{:x}", sha2::Sha256::digest(&zip_bytes))
    );

    // Every entry's checksum matches the public branch's blob.
    for e in &outcome.entries {
        let bytes = git_output(&repo, &["cat-file", "blob", &format!("public:{}", e.path)]).stdout;
        assert_eq!(
            e.sha256,
            format!("{:x}", sha2::Sha256::digest(bytes)),
            "{e:?}"
        );
    }

    // --- notes --------------------------------------------------------------
    let changelog = release::generate_changelog(&repo).unwrap();
    assert_eq!(changelog.to_version, "1.1.0");
    assert!(!changelog.markdown.is_empty());

    // --- checklist ----------------------------------------------------------
    let checklist = {
        use project_manager_lib::github::{Client, Endpoints, GhCli, GitHub, TokenStore};
        struct RefTokenStore;
        impl TokenStore for RefTokenStore {
            fn load(
                &self,
            ) -> Result<
                Option<project_manager_lib::github::Token>,
                project_manager_lib::github::GitHubError,
            > {
                Ok(Some(
                    project_manager_lib::github::Token::new("test-token").unwrap(),
                ))
            }
            fn save(
                &self,
                _token: &project_manager_lib::github::Token,
            ) -> Result<(), project_manager_lib::github::GitHubError> {
                Ok(())
            }
            fn clear(&self) -> Result<(), project_manager_lib::github::GitHubError> {
                Ok(())
            }
        }

        let github = GitHub::new(
            Client::new(Endpoints::github()),
            Box::new(RefTokenStore),
            GhCli::from_env(),
            None,
            data_dir.join("github-prefs.json"),
        );
        release::checklist::checklist(&repo, &data_dir, &github)
    };
    assert_eq!(checklist.version, "1.1.0");
    assert_eq!(checklist.tag, "v1.1.0");

    // The local steps are done; only the GitHub steps are pending (no remote).
    let by_id = |id: &str| checklist.steps.iter().find(|s| s.id == id).unwrap();
    assert!(by_id("version").done);
    assert!(by_id("tag").done);
    assert!(by_id("history").done);
    assert!(by_id("zip").done);
    assert!(by_id("notes").done);
    assert!(!by_id("published").done, "{}", by_id("published").detail);
}
