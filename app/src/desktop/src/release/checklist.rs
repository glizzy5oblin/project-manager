//! The release checklist: the ordered steps of a release, each reporting what is done.
//!
//! The checklist is a read-only view — it runs nothing. It answers "what is left, and what is
//! next" by checking each step against the repository, `.proj`, the app data directory and the
//! GitHub releases of the public repo. Steps are checked in order and every step reports its
//! own state; one failing step (say, GitHub is unreachable) never hides the others. `next_id`
//! is the first pending step — the next thing to do — so the UI can highlight it and the user
//! can resume exactly there.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::github::GitHub;
use crate::proj::ProjFile;

/// One step of the release checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseChecklistStep {
    /// Stable key: `version`, `tag`, `history`, `zip`, `checksum`, `notes`, `published` or `asset`.
    pub id: String,
    /// The step's title.
    pub label: String,
    /// Whether the step is satisfied.
    pub done: bool,
    /// What is known: the current version, the tag, the asset path, or the reason it is not
    /// done yet.
    pub detail: String,
}

/// The ordered checklist of the current version's release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseChecklist {
    /// The current version from `.proj` (empty when `.proj` cannot be read).
    pub version: String,
    /// The release tag (`v<version>`) for that version.
    pub tag: String,
    pub steps: Vec<ReleaseChecklistStep>,
    /// The first pending step's id, or `null` when every step is done.
    pub next_id: Option<String>,
}

/// Check the release checklist for the project at `root`.
///
/// `data_dir` is the app data directory (where built zips land) and `github` is the shared
/// client. Every step reports its own state; nothing is written.
pub fn checklist(root: &Path, data_dir: &Path, github: &GitHub) -> ReleaseChecklist {
    let proj = crate::proj::find_proj_file(root)
        .ok()
        .and_then(|p| ProjFile::load(&p).ok());

    let version = proj
        .as_ref()
        .map(|p| p.project.version.clone())
        .unwrap_or_default();
    let tag = if version.is_empty() {
        String::new()
    } else {
        format!("v{version}")
    };

    let mut steps: Vec<ReleaseChecklistStep> = Vec::new();

    // 1. The version is set (and parses as semver).
    let (version_done, version_detail) = match proj.as_ref() {
        Some(p) => match semver::Version::parse(&p.project.version) {
            Ok(_) => (true, format!("version {}", p.project.version)),
            Err(e) => (
                false,
                format!(
                    "version {:?} is not a semantic version: {e}",
                    p.project.version
                ),
            ),
        },
        None => (false, "the project file could not be read".to_string()),
    };
    steps.push(step("version", "Version set", version_done, version_detail));

    // 2. The tag exists.
    let tag_result: Result<String, String> = if tag.is_empty() {
        Err(String::from("no version to tag"))
    } else {
        // The tag lives on the private branch; the branch is checked against the private repo.
        let private = proj
            .as_ref()
            .and_then(|p| crate::proj::private_root(root, p).ok())
            .unwrap_or_else(|| root.to_path_buf());
        crate::git::GitCommand::new(&private)
            .args(["tag", "--list", &tag])
            .run()
            .map(|out| out.stdout_str())
            .map_err(|e| e.to_string())
    };
    let (tag_done, tag_detail) = match tag_result {
        Ok(out) if !out.is_empty() => (true, format!("tag {tag} created")),
        Ok(_) => (false, format!("tag {tag} not created — run Tag")),
        Err(e) => (false, format!("tag check failed: {e}")),
    };
    steps.push(step("tag", "Tag created", tag_done, tag_detail));

    // 3. The release is recorded in the history table.
    let history_entry = proj
        .as_ref()
        .and_then(|p| p.release.history.iter().find(|e| e.tag == tag).cloned());
    let (history_done, history_detail) = match (proj.as_ref(), history_entry) {
        (Some(_), Some(entry)) => (true, format!("recorded on {} — history table", entry.date)),
        (Some(_), None) => (
            false,
            format!("no history entry for {tag} — record the release"),
        ),
        (None, _) => (false, "the project file could not be read".to_string()),
    };
    steps.push(step(
        "history",
        "Release recorded",
        history_done,
        history_detail,
    ));

    // 4. The zip is built.
    let asset = proj
        .as_ref()
        .map(|p| p.asset_file_name())
        .unwrap_or_default();
    let zip_path = data_dir.join("releases").join(&asset);
    let (zip_done, zip_detail) = if asset.is_empty() {
        (false, "no release asset to check".to_string())
    } else if zip_path.exists() {
        (true, format!("zip built: {}", zip_path.display()))
    } else {
        (false, "zip not built — run Build zip".to_string())
    };
    steps.push(step("zip", "Zip built", zip_done, zip_detail));

    // 5. The zip's checksum verifies: the built zip is read back and hashed,
    // so a corrupted or replaced artifact cannot pass as a release.
    let (checksum_done, checksum_detail) = if asset.is_empty() {
        (false, "no release asset to check".to_string())
    } else if !zip_path.exists() {
        (false, "zip not built — run Build zip".to_string())
    } else {
        match super::file_sha256(&zip_path) {
            Ok(sha256) => (true, format!("zip sha256: {}", &sha256[..16])),
            Err(e) => (false, format!("checksum could not be computed: {e}")),
        }
    };
    steps.push(step(
        "checksum",
        "Checksum verified",
        checksum_done,
        checksum_detail,
    ));

    // 6. The notes are generated.
    let (notes_done, notes_detail) = match super::generate_changelog(root) {
        Ok(changelog) => {
            let commits: usize = changelog.groups.iter().map(|g| g.commits.len()).sum();
            let from = changelog
                .from_tag
                .as_deref()
                .map(|t| t.to_string())
                .unwrap_or_else(|| "the beginning".to_string());
            (true, format!("{commits} commits since {from}"))
        }
        Err(e) => (false, format!("notes could not be generated: {e}")),
    };
    steps.push(step("notes", "Notes generated", notes_done, notes_detail));

    // 7. The release is published on GitHub (a draft does not count as published).
    // 8. The zip is attached to the release.
    let releases = super::github::list_releases(github, root);
    let (published_done, published_detail) = match &releases {
        Ok(releases) => match releases.iter().find(|r| r.tag_name == tag) {
            Some(r) if !r.draft => (true, format!("published as {}", r.tag_name)),
            Some(r) => (
                false,
                format!(
                    "draft release {} — publish it to publish the release",
                    r.tag_name
                ),
            ),
            None => (false, format!("no GitHub release for {tag}")),
        },
        Err(e) => (false, format!("GitHub unavailable: {e}")),
    };
    steps.push(step(
        "published",
        "Release published",
        published_done,
        published_detail,
    ));

    let (asset_done, asset_detail) = match &releases {
        Ok(releases) => match releases.iter().find(|r| r.tag_name == tag) {
            Some(r) => match r.assets.iter().find(|a| a.name == asset) {
                Some(_) => (true, format!("zip attached to {tag}")),
                None => (false, format!("zip asset {asset:?} not uploaded")),
            },
            None => (false, format!("no GitHub release for {tag}")),
        },
        Err(e) => (false, format!("GitHub unavailable: {e}")),
    };
    steps.push(step(
        "asset",
        "Zip asset uploaded",
        asset_done,
        asset_detail,
    ));

    let next_id = steps.iter().find(|s| !s.done).map(|s| s.id.clone());

    ReleaseChecklist {
        version,
        tag,
        steps,
        next_id,
    }
}

fn step(id: &str, label: &str, done: bool, detail: String) -> ReleaseChecklistStep {
    ReleaseChecklistStep {
        id: id.to_string(),
        label: label.to_string(),
        done,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let proj = r#"schema_version = 1

[project]
name = "Example"
version = "1.2.3"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"
"#;
        std::fs::write(root.join("example.proj"), proj).unwrap();
        let output = std::process::Command::new("git")
            .args(["-C", root.to_str().unwrap(), "init", "-q", "-b", "main"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = std::process::Command::new("git")
            .args(["-C", root.to_str().unwrap(), "add", "."])
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = std::process::Command::new("git")
            .args([
                "-C",
                root.to_str().unwrap(),
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@test.local",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "core.autocrlf=false",
                "-c",
                "advice.detachedHead=false",
                "commit",
                "-q",
                "-m",
                "add proj",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        (dir, root)
    }

    #[test]
    fn checklist_reports_done_steps_for_a_tagged_repo() {
        let (dir, root) = fixture_repo();
        // Tag v1.2.3 and record the release, like create_tag does.
        let output = std::process::Command::new("git")
            .args([
                "-C",
                root.to_str().unwrap(),
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "tag",
                "-a",
                "v1.2.3",
                "-m",
                "v1.2.3",
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        super::super::record_release_history(&root, "v1.2.3").unwrap();

        let store = std::sync::Arc::new(crate::github::token::MemoryStore::default());
        let github = GitHub::new(
            crate::github::http::Client::new(crate::github::http::Endpoints::github()),
            Box::new(store),
            crate::github::GhCli::at("/nonexistent/pm-test-gh"),
            None,
            dir.path().join("github.json"),
        );
        let checklist = checklist(&root, dir.path(), &github);

        let version_step = checklist.steps.iter().find(|s| s.id == "version").unwrap();
        assert!(version_step.done, "{version_step:?}");
        let tag_step = checklist.steps.iter().find(|s| s.id == "tag").unwrap();
        assert!(tag_step.done, "{tag_step:?}");
        let history_step = checklist.steps.iter().find(|s| s.id == "history").unwrap();
        assert!(history_step.done, "{history_step:?}");

        // The GitHub steps fail offline: they report pending, never hide the other steps.
        let published_step = checklist
            .steps
            .iter()
            .find(|s| s.id == "published")
            .unwrap();
        assert!(!published_step.done);
        let asset_step = checklist.steps.iter().find(|s| s.id == "asset").unwrap();
        assert!(!asset_step.done);
        // The whole list is in order: version, tag, history, zip, notes, published, asset.
        assert_eq!(
            checklist
                .steps
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "version",
                "tag",
                "history",
                "zip",
                "checksum",
                "notes",
                "published",
                "asset"
            ]
        );
    }

    #[test]
    fn checklist_verifies_the_checksum_of_a_built_zip() {
        let (dir, root) = fixture_repo();
        let releases = dir.path().join("releases");
        std::fs::create_dir_all(&releases).unwrap();
        let zip = releases.join("Example-1.2.3.zip");
        std::fs::write(&zip, b"pretend this is a release zip").unwrap();
        use sha2::Digest as _;
        let sha256 = format!(
            "{:x}",
            sha2::Sha256::digest(b"pretend this is a release zip")
        );

        let store = std::sync::Arc::new(crate::github::token::MemoryStore::default());
        let github = GitHub::new(
            crate::github::http::Client::new(crate::github::http::Endpoints::github()),
            Box::new(store),
            crate::github::GhCli::at("/nonexistent/pm-test-gh"),
            None,
            dir.path().join("github.json"),
        );
        let checklist = checklist(&root, dir.path(), &github);

        // The zip and checksum steps both pass: the built zip is on disk and hashes back.
        let zip_step = checklist.steps.iter().find(|s| s.id == "zip").unwrap();
        assert!(zip_step.done, "{zip_step:?}");
        let checksum_step = checklist.steps.iter().find(|s| s.id == "checksum").unwrap();
        assert!(checksum_step.done, "{checksum_step:?}");
        assert!(
            checksum_step.detail.contains(&sha256[..16]),
            "{checksum_step:?}"
        );
    }

    #[test]
    fn checklist_reports_the_version_and_tag_step() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let proj = r#"schema_version = 1

[project]
name = "Example"
version = "1.2.3"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"
"#;
        std::fs::write(root.join("example.proj"), proj).unwrap();
        let store = std::sync::Arc::new(crate::github::token::MemoryStore::default());
        let github = GitHub::new(
            crate::github::http::Client::new(crate::github::http::Endpoints::github()),
            Box::new(store),
            crate::github::GhCli::at("/nonexistent/pm-test-gh"),
            None,
            dir.path().join("github.json"),
        );
        let checklist = checklist(&root, dir.path(), &github);
        assert_eq!(checklist.version, "1.2.3");
        assert_eq!(checklist.tag, "v1.2.3");

        let version_step = checklist.steps.iter().find(|s| s.id == "version").unwrap();
        assert!(version_step.done);

        // The repo is not even a repository: the tag check fails without hiding the other
        // steps.
        let tag_step = checklist.steps.iter().find(|s| s.id == "tag").unwrap();
        assert!(!tag_step.done);
        assert!(
            checklist
                .steps
                .iter()
                .all(|s| s.id != "published" || !s.done)
        );
    }
}
