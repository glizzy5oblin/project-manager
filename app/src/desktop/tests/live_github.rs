//! Live GitHub check: the whole publish flow against a real repository.
//!
//! Skipped by default. Set PM_LIVE_GITHUB=1 and PM_LIVE_GITHUB_REPO="owner/repo" to run it
//! (the repo must exist, be public, and be new):
//!
//! ```sh
//! gh repo create <name> --public --description "..."
//! PM_LIVE_GITHUB=1 PM_LIVE_GITHUB_REPO="owner/<name>" cargo test --test live_github -- --nocapture
//! ```
//!
//! The test creates the scratch project locally, finishes its release, pushes the local public
//! side to the test repo, and calls the app's GitHub release API for real: create the release,
//! upload the zip, list the releases, and delete the release it created. The test repo is not
//! deleted here (the caller owns it).

use std::path::Path;
use std::process::Command;

use project_manager_lib::github::{Client, Endpoints, GhCli, GitHub, Token, TokenStore};
use project_manager_lib::release;

fn live() -> Option<String> {
    match std::env::var("PM_LIVE_GITHUB") {
        Ok(v) if v == "1" => {}
        _ => return None,
    }
    let repo = std::env::var("PM_LIVE_GITHUB_REPO")
        .unwrap_or_else(|_| panic!("PM_LIVE_GITHUB=1 needs PM_LIVE_GITHUB_REPO=\"owner/repo\""));
    Some(repo)
}

struct RefTokenStore(std::sync::Mutex<Option<Token>>);
impl TokenStore for RefTokenStore {
    fn load(&self) -> Result<Option<Token>, project_manager_lib::github::GitHubError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, _token: &Token) -> Result<(), project_manager_lib::github::GitHubError> {
        Ok(())
    }
    fn clear(&self) -> Result<(), project_manager_lib::github::GitHubError> {
        Ok(())
    }
}

fn git_output(repo: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "live-test")
        .env("GIT_AUTHOR_EMAIL", "live@test")
        .env("GIT_COMMITTER_NAME", "live-test")
        .env("GIT_COMMITTER_EMAIL", "live@test")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn git(repo: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git_output(repo, args))
        .trim()
        .to_string()
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

const PROJ: &str = r#"schema_version = 1

[project]
name = "LiveRelease"
version = "1.0.0"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"
remote = "origin"
"#;

#[test]
fn live_release_upload_succeeds() {
    let Some(repo_slug) = live() else {
        eprintln!("skipping live GitHub test (set PM_LIVE_GITHUB=1)");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let out = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(&repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert!(out.status.success());

    write(&repo.join("example.proj"), PROJ);
    write(
        &repo.join("README.md"),
        "# LiveRelease\n\nA live GitHub release test.\n",
    );
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "feat: live test fixture"]);

    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            &format!("https://github.com/{repo_slug}.git"),
        ],
    );

    let token = GhCli::from_env().token().expect("gh auth token");
    let store = RefTokenStore(std::sync::Mutex::new(Some(token)));
    let github = GitHub::new(
        Client::new(Endpoints::github()),
        Box::new(store),
        GhCli::from_env(),
        None,
        dir.path().join("prefs.json"),
    );

    // The release must name this slug, whatever the local remote says.
    assert_eq!(
        release::github::public_repo_slug(&repo).unwrap(),
        repo_slug,
        "scratch repo remote must be the live test repo"
    );

    // The public branch, and the default branch on the remote, both exist: the release's
    // tag is created on the default branch there.
    git(&repo, &["checkout", "-q", "-b", "public"]);
    git(&repo, &["checkout", "-q", "main"]);
    // The test repo starts with a README commit; integrate it before pushing.
    git(&repo, &["fetch", "origin"]);
    git(&repo, &["rebase", "origin/main"]);
    // The ephemeral env drops the user's credential helper, so give it explicitly.
    git(
        &repo,
        &[
            "-c",
            "credential.helper=!gh auth git-credential",
            "push",
            "-q",
            "origin",
            "main",
        ],
    );

    let release = release::github::create_release(
        &github,
        &repo,
        None, // tag: v1.0.0 from the .proj version
        None,
        "Live release notes",
        false,
        false,
    )
    .expect("live release creation");
    assert_eq!(release.tag_name, "v1.0.0");
    assert!(!release.draft);

    // Build + upload the zip, the way the UI does it.
    let outcome = release::build_release_zip(&repo, &dir.path().join("data")).unwrap();
    let asset = release::github::upload_asset(&github, &repo, &dir.path().join("data"), release.id)
        .expect("live asset upload");
    assert_eq!(asset.name, outcome.file_name);
    assert!(asset.size > 0);
    assert!(
        asset
            .browser_download_url
            .starts_with("https://github.com/")
    );

    // The release now carries the zip.
    let releases = release::github::list_releases(&github, &repo).expect("list releases");
    let found = releases
        .iter()
        .find(|r| r.id == release.id)
        .expect("release listed");
    let zip = release::github::zip_asset_of(found, &outcome.file_name).expect("zip asset listed");
    assert_eq!(zip.id, asset.id);

    // Clean up the release the test created (leaving the test repo for the caller).
    release::github::delete_release(&github, &repo, release.id).unwrap();
}
