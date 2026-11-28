//! Releases: a version bump written to `.proj` and the configured version files, an annotated
//! tag on the private branch, and a zip of the staged public files.
//!
//! The plan (tasks 5.1, 5.2, 5.3): a semantic version bump (preview reads only, then writes);
//! an annotated `v<version>` tag with checks (wrong branch, dirty tree, tag exists) refused
//! before anything is created, optionally pushed to the private remote; and a release zip built
//! from the staged public files (the public branch's HEAD), with a manifest (inside the zip as
//! `release-manifest.json`) and a checksum, and
//! a verification step against the staging allowlist. Everything the release needs is read
//! from `.proj` (project.version is the single source of truth) or the public branch; nothing
//! is written to the repository beyond the tag.

pub mod checklist;
pub mod github;

use std::io::Write as _;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use ts_rs::TS;
use zip::write::SimpleFileOptions;

/// Which part of the version to move. The exact target is passed separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum Bump {
    /// 1.2.3 -> 2.0.0
    Major,
    /// 1.2.3 -> 1.3.0
    Minor,
    /// 1.2.3 -> 1.2.4
    Patch,
    /// The exact version is given in `exact`.
    Exact,
}

/// One file a bump would touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseFile {
    /// Repository-relative path.
    pub path: String,
    /// The version that was in the file.
    pub before: String,
    /// The version the file would get.
    pub after: String,
}

/// The computed change of a bump: what moves, and to what. Nothing is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ReleasePreview {
    /// The version before the bump.
    pub current: String,
    /// The version after the bump.
    pub target: String,
    /// `.proj` first, then the configured version files. Empty when `version_files` is empty.
    pub files: Vec<ReleaseFile>,
}

#[derive(Debug, Error)]
pub enum ReleaseError {
    /// `.proj` or a version file could not be read.
    #[error("{path}: {source}")]
    Io {
        #[source]
        source: std::io::Error,
        path: PathBuf,
    },
    #[error(transparent)]
    Proj(#[from] crate::proj::ProjError),
    /// `.proj`'s `project.version` is not a semantic version. (The file is normally rejected by
    /// validation first; this keeps the error readable rather than failing a parse.)
    #[error("the current version {0:?} in .proj is not a semantic version")]
    CurrentNotSemver(String),
    /// The exact target is missing or is not a semantic version.
    #[error("the exact target version {0:?} is not a semantic version (x.y.z)")]
    ExactNotSemver(String),
    /// The exact target is not newer than the current version.
    #[error("the target version {target:?} is not newer than the current version {current:?}")]
    NotNewer { current: String, target: String },
    /// The exact target version is missing for `Bump::Exact`.
    #[error("an exact target version is required")]
    MissingExact,
    /// A configured version file could not be found (or is not a file).
    #[error("version file {path} does not exist")]
    VersionFileMissing { path: PathBuf },
    /// A configured version file does not contain the current version, so the bump could not
    /// be rewritten safely. The whole bump is refused rather than half-written.
    #[error("version file {path} does not contain the current version {current:?}")]
    VersionFileWithoutCurrent { path: PathBuf, current: String },
    /// A two-repo project is missing its `workflow.public.path`.
    #[error("the two-repo workflow is missing a public path")]
    MissingPublicPath,
    /// A git command failed while reading the public side.
    #[error(transparent)]
    Git(#[from] crate::git::GitError),
    /// The release zip could not be written (compression or the file itself).
    #[error("the release zip could not be written: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// A GitHub release operation failed.
    #[error(transparent)]
    GitHub(#[from] crate::github::GitHubError),

    /// The release manifest could not be serialized.
    #[error("the release manifest could not be serialized: {0}")]
    Manifest(#[from] serde_json::Error),
}

/// The target version for a bump, or an error.
fn target_version(
    current: &Version,
    bump: Bump,
    exact: Option<&str>,
) -> Result<Version, ReleaseError> {
    let target = match bump {
        Bump::Major => Version::new(current.major + 1, 0, 0),
        Bump::Minor => Version::new(current.major, current.minor + 1, 0),
        Bump::Patch => Version::new(current.major, current.minor, current.patch + 1),
        Bump::Exact => {
            let raw = exact.ok_or(ReleaseError::MissingExact)?;
            let parsed =
                Version::parse(raw).map_err(|_| ReleaseError::ExactNotSemver(raw.to_string()))?;
            if parsed <= *current {
                return Err(ReleaseError::NotNewer {
                    current: current.to_string(),
                    target: raw.to_string(),
                });
            }
            parsed
        }
    };
    debug_assert!(target > *current);
    Ok(target)
}

/// Read the `.proj` file and compute what a bump would do, without writing anything.
pub fn preview_bump(
    root: &Path,
    bump: Bump,
    exact: Option<&str>,
) -> Result<ReleasePreview, ReleaseError> {
    let proj_path = crate::proj::find_proj_file(root)?;
    let doc = crate::proj::ProjDocument::load(&proj_path)?;
    let proj = doc.to_proj()?;

    let current = Version::parse(&proj.project.version)
        .map_err(|_| ReleaseError::CurrentNotSemver(proj.project.version.clone()))?;
    let target = target_version(&current, bump, exact)?;

    let proj_name = proj_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!(".{}", crate::proj::PROJ_EXTENSION));

    let private = crate::proj::private_root(root, &proj)?;

    let mut files = Vec::new();
    files.push(ReleaseFile {
        path: proj_name,
        before: current.to_string(),
        after: target.to_string(),
    });
    for f in &proj.release.version_files {
        let path = private.join(f);
        let text = read_version_file(&path)?;
        if !text.contains(&current.to_string()) {
            return Err(ReleaseError::VersionFileWithoutCurrent {
                path,
                current: current.to_string(),
            });
        }
        files.push(ReleaseFile {
            path: f.clone(),
            before: current.to_string(),
            after: target.to_string(),
        });
    }

    Ok(ReleasePreview {
        current: current.to_string(),
        target: target.to_string(),
        files,
    })
}

/// Apply a bump: rewrite the `.proj` version, then the configured version files. The `.proj` is
/// written through [`crate::proj::ProjDocument`] (validated, comment-preserving) and each
/// version file by replacing every occurrence of the old version string. Nothing is written if
/// anything fails along the way — `preview_bump` validates first — and if a write or a read
/// fails mid-way, the `.proj` and every version file written so far are restored to their
/// original contents, so a failed bump leaves the repository exactly as it was.
pub fn apply_bump(
    root: &Path,
    bump: Bump,
    exact: Option<&str>,
) -> Result<ReleasePreview, ReleaseError> {
    let preview = preview_bump(root, bump, exact)?;

    let proj_path = crate::proj::find_proj_file(root)?;
    // Capture every file's original contents before writing anything: the restore step needs
    // them, and it must not depend on the files being readable afterwards.
    let original_proj = std::fs::read_to_string(&proj_path).map_err(|e| ReleaseError::Io {
        path: proj_path.clone(),
        source: e,
    })?;
    let private = crate::proj::private_root(root, &crate::proj::ProjFile::load(&proj_path)?)?;
    let mut originals = Vec::new();
    for f in &preview.files[1..] {
        let path = private.join(&f.path);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ReleaseError::VersionFileMissing { path: path.clone() }
            } else {
                ReleaseError::Io {
                    path: path.clone(),
                    source: e,
                }
            }
        })?;
        originals.push((path, text));
    }

    let result = (|| -> Result<(), ReleaseError> {
        // Write `.proj` first; everything else follows it.
        let mut doc = crate::proj::ProjDocument::load(&proj_path)?;
        doc.set_string(&["project", "version"], &preview.target);
        doc.save(&proj_path)?;

        for (path, text) in &originals {
            std::fs::write(path, text.replace(&preview.current, &preview.target)).map_err(|e| {
                ReleaseError::Io {
                    path: path.clone(),
                    source: e,
                }
            })?;
        }
        Ok(())
    })();

    if result.is_err() {
        // Best-effort restore: an error here must not mask the original failure.
        let _ = std::fs::write(&proj_path, &original_proj);
        for (path, text) in &originals {
            let _ = std::fs::write(path, text);
        }
    }

    result?;
    Ok(preview)
}

fn read_version_file(path: &Path) -> Result<String, ReleaseError> {
    std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ReleaseError::VersionFileMissing {
                path: path.to_path_buf(),
            }
        } else {
            ReleaseError::Io {
                path: path.to_path_buf(),
                source: e,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureRepo;

    const SINGLE_REPO_PROJ: &str = r#"schema_version = 1

[project]
name = "Example"
version = "1.2.3"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"

[release]
version_files = ["manifest.json"]
"#;

    fn fixture() -> FixtureRepo {
        let r = FixtureRepo::new();
        r.write("example.proj", SINGLE_REPO_PROJ);
        r.commit_all("add proj");
        r.write("manifest.json", "{\"version\": \"1.2.3\"}\n");
        r.commit_all("add manifest");
        r
    }

    #[test]
    fn preview_computes_the_target_and_lists_files() {
        let r = fixture();
        let preview = preview_bump(r.path(), Bump::Patch, None).unwrap();
        assert_eq!(preview.current, "1.2.3");
        assert_eq!(preview.target, "1.2.4");
        let paths: Vec<&str> = preview.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["example.proj", "manifest.json"]);

        // Nothing is written by the preview.
        let doc = crate::proj::ProjDocument::load(&r.path().join("example.proj")).unwrap();
        assert_eq!(doc.to_proj().unwrap().project.version, "1.2.3");
        let manifest = std::fs::read_to_string(r.path().join("manifest.json")).unwrap();
        assert!(manifest.contains("1.2.3"));
    }

    #[test]
    fn bump_applies_the_versions() {
        let r = fixture();
        let preview = apply_bump(r.path(), Bump::Minor, None).unwrap();
        assert_eq!(preview.target, "1.3.0");

        let proj = crate::proj::ProjDocument::load(&r.path().join("example.proj"))
            .unwrap()
            .to_proj()
            .unwrap();
        assert_eq!(proj.project.version, "1.3.0");
        let manifest = std::fs::read_to_string(r.path().join("manifest.json")).unwrap();
        assert!(manifest.contains("1.3.0"), "{manifest}");
        assert!(!manifest.contains("1.2.3"), "{manifest}");
    }

    #[test]
    fn exact_bump_requires_a_newer_version() {
        let r = fixture();
        let err = preview_bump(r.path(), Bump::Exact, Some("1.2.3")).unwrap_err();
        assert!(matches!(err, ReleaseError::NotNewer { .. }), "{err:?}");
        let err = preview_bump(r.path(), Bump::Exact, Some("1.2.4")).unwrap();
        assert_eq!(err.target, "1.2.4");
        let err = preview_bump(r.path(), Bump::Exact, Some("banana")).unwrap_err();
        assert!(matches!(err, ReleaseError::ExactNotSemver(_)), "{err:?}");
        let err = preview_bump(r.path(), Bump::Exact, None).unwrap_err();
        assert!(matches!(err, ReleaseError::MissingExact), "{err:?}");
    }

    #[test]
    fn the_bump_is_refused_when_a_version_file_does_not_contain_the_current_version() {
        let r = fixture();
        r.write("manifest.json", "{\"version\": \"9.9.9\"}\n");
        let err = apply_bump(r.path(), Bump::Patch, None).unwrap_err();
        assert!(
            matches!(err, ReleaseError::VersionFileWithoutCurrent { .. }),
            "{err:?}"
        );
        // The old version is still there: the bump refused before writing.
        let proj = crate::proj::ProjDocument::load(&r.path().join("example.proj"))
            .unwrap()
            .to_proj()
            .unwrap();
        assert_eq!(proj.project.version, "1.2.3");
    }

    #[test]
    fn a_failed_write_restores_the_files() {
        let r = fixture();
        let manifest = r.path().join("manifest.json");
        let original = std::fs::read_to_string(&manifest).unwrap();
        // Read-only: the .proj is written first, then the manifest write must fail.
        let mut perms = std::fs::metadata(&manifest).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&manifest, perms).unwrap();

        let err = apply_bump(r.path(), Bump::Minor, None).unwrap_err();
        assert!(matches!(err, ReleaseError::Io { .. }), "{err:?}");

        // Everything is back to its original state.
        let proj = crate::proj::ProjDocument::load(&r.path().join("example.proj"))
            .unwrap()
            .to_proj()
            .unwrap();
        assert_eq!(proj.project.version, "1.2.3");
        let manifest_text = std::fs::read_to_string(&manifest).unwrap();
        assert_eq!(manifest_text, original);
    }

    #[test]
    fn a_missing_version_file_is_an_error() {
        let r = fixture();
        r.git(&["rm", "-q", "manifest.json"]);
        let err = preview_bump(r.path(), Bump::Patch, None).unwrap_err();
        assert!(
            matches!(err, ReleaseError::VersionFileMissing { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn tag_preview_reports_the_checks() {
        let r = fixture();
        let preview = tag_preview(r.path()).unwrap();
        assert_eq!(preview.tag, "v1.2.3");
        assert_eq!(preview.branch, "main");
        assert_eq!(preview.expected_branch, "main");
        assert!(preview.clean);
        assert!(!preview.tag_exists);

        // An untracked file makes the tree dirty.
        r.write("scratch.txt", "x");
        let preview = tag_preview(r.path()).unwrap();
        assert!(!preview.clean, "{preview:?}");

        // A tag that exists makes the check fail.
        r.git(&["rm", "-q", "manifest.json"]);
        r.git(&["add", "-A"]);
        r.git(&["commit", "-q", "-m", "remove manifest"]);
        let preview = tag_preview(r.path()).unwrap();
        assert!(preview.clean, "{preview:?}");
        r.git(&["tag", "-a", "v1.2.3", "-m", "v1.2.3"]);
        let preview = tag_preview(r.path()).unwrap();
        assert!(preview.tag_exists, "{preview:?}");
    }

    #[test]
    fn create_tag_makes_the_tag_on_the_private_branch() {
        let r = fixture();
        // Clean tree, right branch, tag free: the tag is created and reported.
        let result = create_tag(r.path(), false).unwrap();
        assert_eq!(result.tag, "v1.2.3");
        assert_eq!(result.branch, "main");
        assert!(!result.pushed);
        let out = r.git(&["tag", "--list", "v1.2.3"]);
        assert_eq!(out, "v1.2.3");
        let out = r.git(&["tag", "-l", "v1.2.3"]);
        assert!(out.contains("v1.2.3"), "{out}");

        // The release is recorded in the history table, with the tag's author date.
        let proj = crate::proj::ProjDocument::load(&r.path().join("example.proj"))
            .unwrap()
            .to_proj()
            .unwrap();
        assert_eq!(proj.release.history.len(), 1);
        assert_eq!(proj.release.history[0].tag, "v1.2.3");
        assert_eq!(proj.release.history[0].version, "1.2.3");
        assert_eq!(proj.release.history[0].date, "2020-01-01");

        // The history write dirties the tree: a second creation refuses as DirtyTree.
        let err = create_tag(r.path(), false).unwrap_err();
        assert!(matches!(err, TagError::DirtyTree), "{err:?}");

        // Committing the history entry, the tag-exists check fires as before.
        r.commit_all("record the release");
        let err = create_tag(r.path(), false).unwrap_err();
        assert!(matches!(err, TagError::TagExists(_)), "{err:?}");

        // A dirty tree refuses the tag.
        let r2 = fixture();
        r2.write("scratch.txt", "x");
        let err = create_tag(r2.path(), false).unwrap_err();
        assert!(matches!(err, TagError::DirtyTree), "{err:?}");

        // A branch that is not the private branch refuses the tag.
        let r3 = fixture();
        r3.git(&["checkout", "-q", "-b", "feature"]);
        let err = create_tag(r3.path(), false).unwrap_err();
        assert!(matches!(err, TagError::WrongBranch { .. }), "{err:?}");
    }

    #[test]
    fn a_push_failure_keeps_the_tag_and_the_history_entry() {
        let r = fixture();
        r.git(&["remote", "add", "origin", "/nonexistent/path/remote.git"]);

        let err = create_tag(r.path(), true).unwrap_err();
        assert!(matches!(err, TagError::Git(_)), "{err:?}");

        // The tag and the history record survive the failed push: a retry pushes the tag.
        let out = r.git(&["tag", "--list", "v1.2.3"]);
        assert_eq!(out, "v1.2.3");
        let doc = crate::proj::ProjDocument::load(&r.path().join("example.proj")).unwrap();
        let proj = doc.to_proj().unwrap();
        assert_eq!(proj.release.history.len(), 1);
        assert_eq!(proj.release.history[0].tag, "v1.2.3");
    }

    #[test]
    fn major_minor_patch_targets_are_computed() {
        let r = fixture();
        assert_eq!(
            preview_bump(r.path(), Bump::Major, None).unwrap().target,
            "2.0.0"
        );
        assert_eq!(
            preview_bump(r.path(), Bump::Minor, None).unwrap().target,
            "1.3.0"
        );
        assert_eq!(
            preview_bump(r.path(), Bump::Patch, None).unwrap().target,
            "1.2.4"
        );
        // A patch bump ignores `exact`.
        let err = preview_bump(r.path(), Bump::Patch, Some("9.9.9")).unwrap();
        assert_eq!(err.target, "1.2.4");
    }
}

/// The preview of a tag creation: what would be created, and whether the checks pass. All reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TagPreview {
    /// The tag that would be created (`v` + the current version, e.g. `v1.2.4`).
    pub tag: String,
    /// The current branch of the private repository.
    pub branch: String,
    /// The branch a release tag goes on (`workflow.private.branch`).
    pub expected_branch: String,
    /// True when the working tree is clean (no staged or unstaged changes, untracked files and
    /// conflicts aside from none).
    pub clean: bool,
    /// True when the tag already exists (a tag creation would refuse).
    pub tag_exists: bool,
}

/// What a tag creation actually did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TagResult {
    pub tag: String,
    pub branch: String,
    /// True when the tag was pushed to the private remote.
    pub pushed: bool,
}

/// Why a tag creation was refused.
#[derive(Debug, Error)]
pub enum TagError {
    #[error(transparent)]
    Proj(#[from] crate::proj::ProjError),
    #[error(transparent)]
    Git(#[from] crate::git::GitError),
    /// The repository is on the wrong branch to release.
    #[error("on branch {branch:?}, not {expected:?} — a release tag goes on the private branch")]
    WrongBranch { branch: String, expected: String },
    /// The working tree is not clean.
    #[error("the working tree is not clean — commit, or stash, the changes before tagging")]
    DirtyTree,
    /// The tag already exists.
    #[error("tag {0:?} already exists")]
    TagExists(String),
    /// `.proj`'s version is not a semantic version, so a tag name cannot be derived.
    #[error("the current version {0:?} in .proj is not a semantic version")]
    CurrentNotSemver(String),
}

/// The annotated tag to create for the current version: `v{x.y.z}`.
fn tag_name(version: &str) -> String {
    format!("v{version}")
}

/// Validate the tag checks for the project at `root` and compute the preview.
pub fn tag_preview(root: &Path) -> Result<TagPreview, TagError> {
    let proj_path = crate::proj::find_proj_file(root)?;
    let doc = crate::proj::ProjDocument::load(&proj_path)?;
    let proj = doc.to_proj()?;

    let version = proj.project.version.clone();
    Version::parse(&version).map_err(|_| TagError::CurrentNotSemver(version.clone()))?;
    let tag = tag_name(&version);

    let private = crate::proj::private_root(root, &proj)?;

    let out = crate::git::GitCommand::new(&private)
        .args(["branch", "--show-current"])
        .run()?;
    let branch = out.stdout_str();

    let out = crate::git::GitCommand::new(&private)
        .args(["status", "--porcelain"])
        .run()?;
    let clean = out.stdout_str().is_empty();

    let out = crate::git::GitCommand::new(&private)
        .args(["tag", "--list", &tag])
        .run()?;
    let tag_exists = !out.stdout_str().is_empty();

    Ok(TagPreview {
        tag,
        branch,
        expected_branch: proj.workflow.private.branch.clone(),
        clean,
        tag_exists,
    })
}

/// Record one released tag in the project file's `[[release.history]]` table. Returns whether a
/// new entry was added (a tag recorded by an earlier run is a no-op). The version is derived
/// from the tag (`v` + the semver), not from `.proj`: the entry records the release, which
/// may have been tagged by hand.
pub fn record_release_history(root: &Path, tag: &str) -> Result<bool, TagError> {
    let Some(version) = tag.strip_prefix('v') else {
        return Err(TagError::CurrentNotSemver(tag.to_string()));
    };
    let proj_path = crate::proj::find_proj_file(root)?;
    let proj = crate::proj::ProjFile::load(&proj_path)?;
    let private = crate::proj::private_root(root, &proj)?;
    let tag_date = crate::git::GitCommand::new(&private)
        .args(["log", "-1", "--format=%ad", "--date=short", tag])
        .run()?
        .stdout_str();
    let mut doc = crate::proj::ProjDocument::load(&proj_path)?;
    let added = doc.push_release_history(tag, version, &tag_date);
    doc.save(&proj_path)?;
    Ok(added)
}

/// Create the annotated release tag on the private branch, then optionally push it. The checks
/// run first; nothing is created when any of them fails. Pushing is a network operation and is
/// reported separately: a push failure does not delete the tag.
pub fn create_tag(root: &Path, push: bool) -> Result<TagResult, TagError> {
    let preview = tag_preview(root)?;
    if preview.branch != preview.expected_branch {
        return Err(TagError::WrongBranch {
            branch: preview.branch,
            expected: preview.expected_branch,
        });
    }
    if !preview.clean {
        return Err(TagError::DirtyTree);
    }
    if preview.tag_exists {
        return Err(TagError::TagExists(preview.tag));
    }

    let proj_path = crate::proj::find_proj_file(root)?;
    let proj = crate::proj::ProjFile::load(&proj_path)?;
    let private = crate::proj::private_root(root, &proj)?;

    let message = preview.tag.clone();
    crate::git::GitCommand::new(&private)
        .args(["tag", "-a", &preview.tag, "-m", &message])
        .run()?;

    // Record the release in the project file: the entry is written before the push (the tag
    // exists, the record is local, a push failure must not lose it). Idempotent: a tag
    // recorded by an earlier run is recorded once.
    record_release_history(root, &preview.tag)?;

    let pushed = if push {
        let remote = crate::proj::ProjDocument::load(&crate::proj::find_proj_file(root)?)?
            .to_proj()?
            .workflow
            .private
            .remote
            .clone();
        crate::git::GitCommand::new(&private)
            .args([
                "push",
                &remote,
                &format!("refs/tags/{}:refs/tags/{}", preview.tag, preview.tag),
            ])
            .run()?;
        true
    } else {
        false
    };

    Ok(TagResult {
        tag: preview.tag,
        branch: preview.branch,
        pushed,
    })
}

// ===== the release zip (5.3) =====

/// One staged public file inside the release zip. The paths are relative to the public side's
/// repository root (in two-repo mode, the public repo; in single-repo mode, the project root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ZipFileEntry {
    /// Repository-relative path.
    pub path: String,
    /// Size of the file in bytes.
    pub size: u64,
    /// Lowercase hex sha256 of the file's content.
    pub sha256: String,
    /// Git mode (`120000` for a symlink, `100644`/`100755` otherwise). A symlink's content is
    /// its target path; the manifest records the mode so a reader can tell.
    pub mode: String,
}

/// The outcome of building a release zip. The zip is written to the app data dir
/// (`<data_dir>/releases/<asset_name>`); the repository is not written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ZipOutcome {
    /// File name of the written zip.
    pub file_name: String,
    /// Absolute path of the written zip.
    pub path: PathBuf,
    /// The project files in the zip, repository order.
    pub entries: Vec<ZipFileEntry>,
    /// Lowercase hex sha256 of the written zip file.
    pub sha256: String,
    /// Size of the written zip file in bytes.
    pub bytes: u64,
    /// Project files the staging allowlist would not stage (a drift check; empty when clean).
    pub violations: Vec<String>,
}

/// The staging allowlist's check for the zipped files: which of them would not be staged.
fn allowlist_violations(root: &Path, entries: &[ZipFileEntry]) -> Vec<String> {
    let allowlist = match crate::staging::allowlist(root) {
        Ok(a) => a,
        // No allowlist means we cannot verify; report nothing and let the caller decide.
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for e in entries {
        // `.proj` itself is not part of the staged set (it is project configuration, not a
        // public file) and is skipped naturally because it is not in the public branch.
        if !allowlist.public_staged(&e.path) {
            out.push(e.path.clone());
        }
    }
    out
}

/// The entry name the zip's own manifest is written under, inside the zip. Kept distinct from
/// any project file (the old `manifest.json` collided with a project file of that name).
const RELEASE_MANIFEST_NAME: &str = "release-manifest.json";

/// The public side's repository root: the configured public repo in two-repo mode, the project
/// root in single-repo mode (both share the same relative file paths).
pub(crate) fn public_root(
    root: &Path,
    proj: &crate::proj::ProjFile,
) -> Result<PathBuf, ReleaseError> {
    match proj.workflow.mode {
        crate::proj::WorkflowMode::TwoRepo => {
            let rel = proj
                .workflow
                .public
                .path
                .as_deref()
                .ok_or(ReleaseError::MissingPublicPath)?;
            Ok(root.join(rel))
        }
        crate::proj::WorkflowMode::SingleRepo => Ok(root.to_path_buf()),
    }
}

/// Read the public branch's HEAD commit as a full sha.
fn public_head(public_root: &Path, branch: &str) -> Result<String, ReleaseError> {
    let out = crate::git::GitCommand::new(public_root)
        .args(["rev-parse", branch])
        .run()?;
    Ok(out.stdout_str())
}

/// Build the release zip: every file on the public branch at its HEAD, with a manifest inside
/// the zip and a checksum of the zip itself, verified against the staging allowlist.
///
/// The zip lands in `<data_dir>/releases/<asset_name>` (nothing is written to the repository).
/// In two-repo mode the public repo is read; in single-repo mode, the public branch of the
/// project repository. Symlinks are stored as small files containing the link target.
pub fn build_release_zip(root: &Path, data_dir: &Path) -> Result<ZipOutcome, ReleaseError> {
    let proj_path = crate::proj::find_proj_file(root)?;
    let doc = crate::proj::ProjDocument::load(&proj_path)?;
    let proj = doc.to_proj()?;

    let public_root = public_root(root, &proj)?;
    let branch = proj.workflow.public.branch.clone();

    // List the public branch's files with their git modes. `-z` keeps paths with spaces and
    // newlines unquoted; each entry is `<mode> blob <oid>\t<path>\0`.
    let out = crate::git::GitCommand::new(&public_root)
        .args(["ls-tree", "-r", "-z", &branch])
        .run()?;

    // Read every blob into memory; the zip keeps the same bytes the branch has.
    let mut pending: Vec<(String, String, Vec<u8>)> = Vec::new();
    for entry in String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|e| !e.is_empty())
    {
        let Some((meta, path)) = entry.split_once('\t') else {
            return Err(ReleaseError::Git(crate::git::GitError::Failed {
                command: "git ls-tree -r -z".to_string(),
                code: Some(0),
                stderr: format!("could not parse ls-tree entry {entry:?}"),
            }));
        };
        let mut parts = meta.split(' ');
        let mode = parts.next().unwrap_or("").to_string();
        let ftype = parts.next().unwrap_or("").to_string();
        if ftype != "blob" {
            // Submodule gitlinks (mode 160000, type commit) are not files in the zip.
            continue;
        }
        // A symlink's blob is the target path, not the target's content; keep the raw bytes.
        let scope = format!("{branch}:{path}");
        let blob = crate::git::GitCommand::new(&public_root)
            .args(["cat-file", "blob", &scope])
            .run()?;
        pending.push((path.to_string(), mode, blob.stdout));
    }

    let head = public_head(&public_root, &branch)?;

    // The manifest: every file in the zip and the commit it was built from. No build
    // timestamp - the branch head is the deterministic fact.
    let mut entries = Vec::new();
    for (path, mode, bytes) in &pending {
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        entries.push(ZipFileEntry {
            path: path.clone(),
            size: bytes.len() as u64,
            sha256,
            mode: mode.clone(),
        });
    }

    let manifest = serde_json::json!({
        "project": proj.project.name,
        "version": proj.project.version,
        "branch": branch,
        "commit": head,
        "files": entries,
    });
    let manifest_text = serde_json::to_string_pretty(&manifest)?;

    let out_dir = data_dir.join("releases");
    std::fs::create_dir_all(&out_dir).map_err(|e| ReleaseError::Io {
        path: out_dir.clone(),
        source: e,
    })?;
    let file_name = proj.asset_file_name();
    let zip_path = out_dir.join(&file_name);

    {
        let mut file = std::fs::File::create(&zip_path).map_err(|e| ReleaseError::Io {
            path: zip_path.clone(),
            source: e,
        })?;
        let mut writer = zip::ZipWriter::new(&mut file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, _mode, bytes) in &pending {
            // Normalise separators: the zip stores forward-slash paths.
            let entry_name = path.replace('\\', "/");
            writer.start_file(&entry_name, options)?;
            writer.write_all(bytes).map_err(|e| ReleaseError::Io {
                path: zip_path.clone(),
                source: e,
            })?;
        }
        writer.start_file(RELEASE_MANIFEST_NAME, options)?;
        writer
            .write_all(manifest_text.as_bytes())
            .map_err(|e| ReleaseError::Io {
                path: zip_path.clone(),
                source: e,
            })?;
        writer.finish()?;
    }

    // Stream the zip from disk: the checksum is over the file as written.
    let bytes = std::fs::metadata(&zip_path)
        .map_err(|e| ReleaseError::Io {
            path: zip_path.clone(),
            source: e,
        })?
        .len();
    let sha256 = file_sha256(&zip_path).map_err(|e| ReleaseError::Io {
        path: zip_path.clone(),
        source: e,
    })?;

    let violations = allowlist_violations(root, &entries);

    Ok(ZipOutcome {
        file_name,
        path: zip_path,
        entries,
        sha256,
        bytes,
        violations,
    })
}

/// The SHA-256 of every byte in a file, streamed in 64 KB blocks. A read-only checksum:
/// the release zip's checksum is the whole file, not the manifest's per-file hashes.
pub fn file_sha256(path: &Path) -> Result<String, std::io::Error> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

// --- the changelog (5.4) --------------------------------------------------------------------
///
/// One commit in a changelog group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChangelogCommit {
    /// Full sha.
    pub sha: String,
    /// Short sha for display.
    pub short_sha: String,
    /// The first line of the commit message (the subject).
    pub subject: String,
    /// The configured conventional-commit prefix (e.g. `"feat"`) or `"other"`.
    pub group: String,
}

/// One group of commits — one configured prefix, or `Other changes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChangelogGroup {
    /// The configured prefix (`"other"` when nothing matched).
    pub name: String,
    /// The display heading used in the markdown.
    pub title: String,
    /// The group's commits, oldest first.
    pub commits: Vec<ChangelogCommit>,
}

/// The generated release notes: every commit on the private branch since the last
/// tag, grouped by the configured conventional-commit prefixes, plus the markdown
/// ready to publish as a GitHub release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Changelog {
    /// The last tag on the branch before `HEAD` (`null` when the branch has no tag).
    pub from_tag: Option<String>,
    /// The version the release moves to (`.proj`'s `project.version`).
    pub to_version: String,
    /// Groups in the display order of `changelog_groups`, each holding its commits
    /// oldest first.
    pub groups: Vec<ChangelogGroup>,
    /// The markdown ready for the GitHub release editor.
    pub markdown: String,
}

/// The display heading for a group prefix (`"other"` renders as `Other changes`).
pub(crate) fn group_title(prefix: &str) -> String {
    let known: &[(&str, &str)] = &[
        ("feat", "Features"),
        ("fix", "Bug fixes"),
        ("perf", "Performance"),
        ("docs", "Documentation"),
        ("refactor", "Refactoring"),
        ("test", "Tests"),
        ("build", "Build"),
        ("ci", "CI"),
        ("chore", "Housekeeping"),
        ("style", "Style"),
        ("revert", "Reverts"),
    ];
    if let Some((_, title)) = known.iter().find(|(k, _)| *k == prefix) {
        return (*title).to_string();
    }
    if prefix == "other" {
        return "Other changes".to_string();
    }
    let mut out = String::new();
    for (i, c) in prefix.chars().enumerate() {
        let upper = c.to_uppercase().next().unwrap_or(c);
        let lower = c.to_lowercase().next().unwrap_or(c);
        out.push(if i == 0 { upper } else { lower });
    }
    out
}

/// The conventional-commit prefix of a subject, e.g. `"feat(scope): add thing"` ->
/// `"feat"` (case-insensitive). `None` when the subject carries no prefix.
pub(crate) fn subject_prefix(subject: &str) -> Option<String> {
    let head = subject.split_once(':').map(|(h, _)| h).unwrap_or(subject);
    let token = head.split_once('(').map(|(h, _)| h).unwrap_or(head).trim();
    let token = token.trim_end_matches('!').trim();
    if token.is_empty()
        || !token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return None;
    }
    Some(token.to_lowercase())
}

fn private_repo(root: &Path) -> Result<std::path::PathBuf, ReleaseError> {
    let proj = crate::proj::ProjFile::load(&crate::proj::find_proj_file(root)?)?;
    Ok(crate::proj::private_root(root, &proj)?)
}

/// The nearest tag reachable from `rev`, or `None` (no tag, or `rev` does not resolve).
fn nearest_tag(repo: &Path, rev: &str) -> Option<String> {
    match crate::git::GitCommand::new(repo)
        .args(["describe", "--tags", "--abbrev=0", rev])
        .output()
    {
        Ok(out) if out.code == Some(0) => {
            let tag = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Some(tag).filter(|t| !t.is_empty())
        }
        _ => None,
    }
}

/// The commit `rev` points at, or `None` when it does not resolve.
fn commit_of(repo: &Path, rev: &str) -> Option<String> {
    let spec = format!("{rev}^{{commit}}");
    match crate::git::GitCommand::new(repo)
        .args(["rev-parse", "--verify", "--quiet", spec.as_str()])
        .output()
    {
        Ok(out) if out.code == Some(0) => {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|s| !s.is_empty())
        }
        _ => None,
    }
}

/// Where the notes for `version` start. Normally the last tag; but once this version's own
/// tag sits on HEAD (the Tag step ran before the notes were written), the notes run from the
/// tag before it, so tagging does not empty them.
fn changelog_base(root: &Path, version: &str) -> Result<Option<String>, ReleaseError> {
    let private = private_repo(root)?;
    let last = nearest_tag(&private, "HEAD");
    let own = tag_name(version);
    if last.as_deref() == Some(own.as_str())
        && commit_of(&private, &own).is_some()
        && commit_of(&private, &own) == commit_of(&private, "HEAD")
    {
        return Ok(nearest_tag(&private, "HEAD^"));
    }
    Ok(last)
}

/// Every commit on the branch since the last tag (or the whole branch), oldest first.
/// Merge commits are excluded: a release's own commit history is the useful part.
pub(crate) fn commits_since(
    root: &Path,
    since: Option<&str>,
) -> Result<Vec<(String, String, String)>, ReleaseError> {
    let mut args = vec![
        "log",
        "--reverse",
        "--no-merges",
        "--no-abbrev-commit",
        "--pretty=format:%H%x00%h%x00%s",
    ];
    let range = since.map(|tag| format!("{tag}..HEAD"));
    if let Some(range) = &range {
        args.push(range.as_str());
    } else {
        args.push("HEAD");
    }
    let proj = crate::proj::ProjFile::load(&crate::proj::find_proj_file(root)?)?;
    let private = crate::proj::private_root(root, &proj)?;
    let out = crate::git::GitCommand::new(&private).args(&args).output()?;
    if out.code != Some(0) {
        return Err(ReleaseError::Git(crate::git::GitError::Failed {
            command: "git log".to_string(),
            code: out.code,
            stderr: out.stderr.trim().to_string(),
        }));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut commits = Vec::new();
    for line in text.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\0');
        let sha = parts.next().unwrap_or("").to_string();
        let short_sha = parts.next().unwrap_or("").to_string();
        let subject = parts.next().unwrap_or("").to_string();
        if sha.is_empty() {
            continue;
        }
        commits.push((sha, short_sha, subject));
    }
    Ok(commits)
}

/// Generate the release notes for `.proj` at `root`: commits since the last tag,
/// grouped by the configured conventional-commit prefixes, rendered as markdown.
pub fn generate_changelog(root: &Path) -> Result<Changelog, ReleaseError> {
    let proj_path = crate::proj::find_proj_file(root)?;
    let proj = crate::proj::ProjFile::load(&proj_path)?;
    let version = proj.project.version.clone();
    let from_tag = changelog_base(root, &version)?;
    let commits = commits_since(root, from_tag.as_deref())?;

    let configured = &proj.release.changelog_groups;
    let mut groups: Vec<ChangelogGroup> = configured
        .iter()
        .map(|name| ChangelogGroup {
            name: name.to_lowercase(),
            title: group_title(&name.to_lowercase()),
            commits: Vec::new(),
        })
        .collect();
    groups.push(ChangelogGroup {
        name: "other".to_string(),
        title: group_title("other"),
        commits: Vec::new(),
    });

    for (sha, short_sha, subject) in &commits {
        let prefix = subject_prefix(subject);
        let group = prefix
            .as_deref()
            .and_then(|p| configured.iter().position(|c| c.trim().to_lowercase() == p))
            .unwrap_or(configured.len());
        let group_name = groups[group].name.clone();
        groups[group].commits.push(ChangelogCommit {
            sha: sha.to_string(),
            short_sha: short_sha.to_string(),
            subject: subject.to_string(),
            group: group_name,
        });
    }

    // The group list may end with an empty group; drop it so the editor never renders
    // an empty heading (the "Other changes" group stays whenever there is anything to show).
    groups.retain(|g| !g.commits.is_empty());

    let mut markdown = String::new();
    markdown.push_str(&format!("## v{version}\n\n"));
    for group in &groups {
        markdown.push_str(&format!("### {}\n", group.title));
        for commit in &group.commits {
            markdown.push_str(&format!("- {} ({})\n", commit.subject, commit.short_sha));
        }
        markdown.push('\n');
    }
    if commits.is_empty() {
        markdown.push_str(&match from_tag.as_deref() {
            Some(tag) => format!("_No commits since `{tag}`._\n"),
            None => "_No commits on the branch yet._\n".to_string(),
        });
    }
    let since_line = match from_tag.as_deref() {
        Some(tag) => format!("Commits since `{tag}`."),
        None => "All commits on the branch (no tag yet).".to_string(),
    };
    markdown.push_str(&format!("\n---\n_{since_line}_\n"));

    Ok(Changelog {
        from_tag,
        to_version: version,
        groups,
        markdown,
    })
}

#[cfg(test)]
mod zip_tests {
    use super::*;
    use crate::testing::FixtureRepo;

    const SINGLE_REPO_PROJ: &str = r#"schema_version = 1

[project]
name = "Example"
version = "1.2.3"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"

[release]
version_files = []
"#;

    const TWO_REPO_PROJ: &str = r#"schema_version = 1

[project]
name = "Example"
version = "1.2.3"

[workflow]
mode = "two-repo"

[workflow.private]
path = "."
branch = "main"
remote = "origin"

[workflow.public]
path = "../public"
branch = "main"
remote = "origin"

[release]
version_files = []
"#;

    fn fixture() -> FixtureRepo {
        let r = FixtureRepo::new();
        r.write("example.proj", SINGLE_REPO_PROJ);
        r.commit_all("add proj");
        r
    }

    fn out_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    fn public_git(dir: &Path, args: &[&str]) -> std::process::Output {
        std::process::Command::new("git")
            .args(["-C", dir.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    }

    fn public_commit(dir: &Path, message: &str) {
        // The public repo has no commits yet; seed a deterministic author + commit.
        assert!(public_git(dir, &["add", "-A"]).status.success());
        let out = public_git(
            dir,
            &[
                "-c",
                "user.name=tester",
                "-c",
                "user.email=tester@test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
        assert!(out.status.success(), "git commit failed: {out:?}");
    }

    #[test]
    fn zip_builds_the_public_branch_with_manifest_and_checksum() {
        let r = fixture();
        // Stage files on the public branch.
        r.git(&["checkout", "-b", "public"]);
        r.write("index.js", "// index\n");
        r.write("README.md", "# readme\n");
        r.commit_all("public files");

        let (_keep, out_dir) = out_dir();
        let outcome = build_release_zip(r.path(), &out_dir).unwrap();

        // Asset name comes from the default `{name}-{version}.zip`.
        assert_eq!(outcome.file_name, "Example-1.2.3.zip");
        assert!(outcome.path.ends_with("releases/Example-1.2.3.zip"));

        // Every public-branch file is in the manifest and the zip.
        let paths: Vec<&str> = outcome.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"index.js"), "{paths:?}");
        assert!(paths.contains(&"README.md"), "{paths:?}");

        // Per-file checksums are correct.
        for e in &outcome.entries {
            let bytes = std::fs::read(r.path().join(&e.path)).unwrap();
            assert_eq!(e.sha256, format!("{:x}", Sha256::digest(&bytes)), "{e:?}");
        }

        // The zip exists, its size and checksum match what was returned.
        let zip_bytes = std::fs::read(&outcome.path).unwrap();
        assert_eq!(outcome.bytes as usize, zip_bytes.len());
        assert_eq!(outcome.sha256, format!("{:x}", Sha256::digest(&zip_bytes)));

        // The zip's manifest is inside the zip and matches the outcome.
        let mut archive =
            zip::ZipArchive::new(std::fs::File::open(&outcome.path).unwrap()).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("release-manifest.json").unwrap(),
            &mut text,
        )
        .unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(manifest["version"], "1.2.3");
        assert_eq!(manifest["branch"], "public");
        // The commit is the public branch's full HEAD sha.
        let commit = manifest["commit"].as_str().unwrap();
        assert_eq!(commit.len(), 40, "{commit}");
        assert_eq!(
            manifest["files"].as_array().unwrap().len(),
            outcome.entries.len()
        );

        // The zip has no allowlist violations (index.js and README.md are both staged).
        assert!(outcome.violations.is_empty(), "{:?}", outcome.violations);
    }

    #[test]
    fn zip_reports_files_the_allowlist_would_not_stage() {
        let r = fixture();
        r.git(&["checkout", "-b", "public"]);
        // A markdown file that the default allowlist excludes, plus one that is allowed.
        r.write("notes.md", "# scratch\n");
        r.write("index.js", "// index\n");
        r.commit_all("public files");

        let (_keep, out_dir) = out_dir();
        let outcome = build_release_zip(r.path(), &out_dir).unwrap();

        // notes.md is excluded by the default rules (only README/CHANGELOG/LICENSE are kept).
        assert_eq!(outcome.violations, vec!["notes.md"]);
    }

    #[test]
    fn zip_handles_a_project_file_named_manifest_json() {
        // The project's own manifest.json and the zip's own manifest must not collide.
        let r = FixtureRepo::new();
        r.write("example.proj", SINGLE_REPO_PROJ);
        r.write("manifest.json", "{\"version\": \"1.2.3\"}\n");
        r.commit_all("add proj");

        // The public branch carries the version file plus a public file.
        r.git(&["checkout", "-q", "-b", "public"]);
        r.write("src/index.js", "// public\n");
        r.commit_all("public");

        let (_keep, out_dir) = out_dir();
        let outcome = build_release_zip(r.path(), &out_dir).unwrap();

        let paths: Vec<&str> = outcome.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"manifest.json"), "{paths:?}");

        // Both the project's manifest.json and the zip's release-manifest.json are present.
        let mut archive =
            zip::ZipArchive::new(std::fs::File::open(&outcome.path).unwrap()).unwrap();
        assert!(archive.by_name("manifest.json").is_ok());
        assert!(archive.by_name("release-manifest.json").is_ok());
    }

    #[test]
    fn zip_handles_two_repo_mode() {
        let private = FixtureRepo::new();
        let sibling = private.path().parent().unwrap().join("public");
        let out = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&sibling)
            .output()
            .unwrap();
        assert!(out.status.success(), "git init failed: {out:?}");

        private.write("example.proj", TWO_REPO_PROJ);
        private.commit_all("add proj");

        // Stage files into the public repo and commit them there.
        std::fs::write(sibling.join("index.js"), "// index\n").unwrap();
        std::fs::write(sibling.join("README.md"), "# readme\n").unwrap();
        public_commit(&sibling, "stage files");

        let (_keep, out_dir) = out_dir();
        let outcome = build_release_zip(private.path(), &out_dir).unwrap();

        let paths: Vec<&str> = outcome.entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"index.js"), "{paths:?}");
        assert!(paths.contains(&"README.md"), "{paths:?}");
    }
}

#[cfg(test)]
mod changelog_tests {
    use super::*;
    use crate::testing::FixtureRepo;

    const CHANGELOG_PROJ: &str = r#"schema_version = 1

[project]
name = "Example"
version = "1.2.3"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"

[release]
changelog_groups = ["feat", "fix", "docs", "chore"]
"#;

    fn changelog_fixture() -> FixtureRepo {
        let r = FixtureRepo::new();
        r.write("example.proj", CHANGELOG_PROJ);
        r.commit_all("chore: add proj");
        r.write("a.txt", "a\n");
        r.commit_all("feat: add the thing");
        r.write("b.txt", "b\n");
        r.commit_all("fix: the thing again");
        r.git(&["tag", "v1.2.3"]);
        r.write("c.txt", "c\n");
        r.commit_all("feat: one more thing");
        r.write("d.txt", "d\n");
        r.commit_all("random message");
        r
    }

    #[test]
    fn changelog_groups_commits_since_the_last_tag() {
        let r = changelog_fixture();
        let cl = generate_changelog(r.path()).unwrap();

        assert_eq!(cl.from_tag.as_deref(), Some("v1.2.3"));
        assert_eq!(cl.to_version, "1.2.3");

        assert_eq!(cl.groups.len(), 2, "{cl:?}");
        assert_eq!(cl.groups[0].name, "feat");
        assert_eq!(cl.groups[0].title, "Features");
        assert_eq!(cl.groups[0].commits.len(), 1);
        assert_eq!(cl.groups[0].commits[0].subject, "feat: one more thing");
        assert_eq!(cl.groups[0].commits[0].group, "feat");
        assert_eq!(cl.groups[1].name, "other");
        assert_eq!(cl.groups[1].title, "Other changes");
        assert_eq!(cl.groups[1].commits.len(), 1);
        assert_eq!(cl.groups[1].commits[0].subject, "random message");

        // The commits before the tag are not in the notes.
        assert!(!cl.markdown.contains("feat: add the thing"), "{cl:?}");
        assert!(cl.markdown.contains("## v1.2.3"), "{cl:?}");
        assert!(cl.markdown.contains("### Features"), "{cl:?}");
        assert!(cl.markdown.contains("### Other changes"), "{cl:?}");
        assert!(cl.markdown.contains("feat: one more thing"), "{cl:?}");
        assert!(cl.markdown.contains("Commits since `v1.2.3`."), "{cl:?}");
    }

    #[test]
    fn changelog_with_no_tag_covers_the_whole_branch() {
        let r = changelog_fixture();
        r.git(&["tag", "-d", "v1.2.3"]);
        let cl = generate_changelog(r.path()).unwrap();
        assert_eq!(cl.from_tag, None);
        assert_eq!(cl.groups.len(), 4, "{cl:?}");
        let names: Vec<&str> = cl.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["feat", "fix", "chore", "other"]);
        // The two feat commits (before and after the tag position) are both present.
        let feat: Vec<&str> = cl.groups[0]
            .commits
            .iter()
            .map(|c| c.subject.as_str())
            .collect();
        assert_eq!(feat, vec!["feat: add the thing", "feat: one more thing"]);
        assert!(
            cl.markdown
                .contains("All commits on the branch (no tag yet)."),
            "{cl:?}"
        );
    }

    #[test]
    fn changelog_excludes_merge_commits() {
        let r = changelog_fixture();
        r.git(&["checkout", "-b", "feature"]);
        r.write("feature.txt", "f\n");
        r.commit_all("feat: from a branch");
        r.git(&["checkout", "main"]);
        r.git(&[
            "merge",
            "--no-ff",
            "feature",
            "-m",
            "Merge branch 'feature'",
        ]);
        let merge_sha = r.head();
        let cl = generate_changelog(r.path()).unwrap();
        let all_shas: Vec<&str> = cl
            .groups
            .iter()
            .flat_map(|g| g.commits.iter().map(|c| c.sha.as_str()))
            .collect();
        assert!(
            !all_shas.contains(&merge_sha.as_str()),
            "merge commit leaked: {all_shas:?}"
        );
        assert_eq!(cl.groups.iter().map(|g| g.commits.len()).sum::<usize>(), 3);
    }

    #[test]
    fn changelog_empty_between_tag_and_head() {
        // HEAD is past the last tag, which belongs to another version: nothing new yet.
        let r = changelog_fixture();
        r.git(&["tag", "-d", "v1.2.3"]);
        r.git(&["tag", "v1.2.2"]);
        let cl = generate_changelog(r.path()).unwrap();
        assert_eq!(cl.from_tag.as_deref(), Some("v1.2.2"));
        assert!(cl.groups.is_empty(), "{cl:?}");
        assert!(
            cl.markdown.contains("_No commits since `v1.2.2`._"),
            "{cl:?}"
        );
    }

    #[test]
    fn changelog_after_tagging_this_version_runs_from_the_previous_tag() {
        // The Tag step put v1.2.3 (this version) on HEAD: the notes must not go empty.
        let r = changelog_fixture();
        r.git(&["tag", "-d", "v1.2.3"]);
        r.git(&["tag", "v1.2.2", "HEAD~2"]);
        r.git(&["tag", "-a", "v1.2.3", "-m", "v1.2.3"]);
        let cl = generate_changelog(r.path()).unwrap();
        assert_eq!(cl.from_tag.as_deref(), Some("v1.2.2"));
        let subjects: Vec<&str> = cl
            .groups
            .iter()
            .flat_map(|g| g.commits.iter().map(|c| c.subject.as_str()))
            .collect();
        assert_eq!(subjects, vec!["feat: one more thing", "random message"]);
    }

    #[test]
    fn changelog_after_tagging_the_first_release_covers_the_whole_branch() {
        let r = changelog_fixture();
        r.git(&["tag", "-d", "v1.2.3"]);
        r.git(&["tag", "v1.2.3"]);
        let cl = generate_changelog(r.path()).unwrap();
        assert_eq!(cl.from_tag, None);
        assert_eq!(cl.groups.iter().map(|g| g.commits.len()).sum::<usize>(), 5);
    }

    #[test]
    fn changelog_respects_a_custom_group_list() {
        let r = FixtureRepo::new();
        r.write(
            "example.proj",
            CHANGELOG_PROJ.replace(
                "changelog_groups = [\"feat\", \"fix\", \"docs\", \"chore\"]",
                "changelog_groups = [\"feat\"]",
            ),
        );
        r.commit_all("chore: add proj");
        r.write("a.txt", "a\n");
        r.commit_all("feat: a");
        r.write("b.txt", "b\n");
        r.commit_all("fix: b");
        let cl = generate_changelog(r.path()).unwrap();
        let names: Vec<&str> = cl.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["feat", "other"], "{cl:?}");
        // Everything else lands in Other, in chronological order.
        let other: Vec<&str> = cl.groups[1]
            .commits
            .iter()
            .map(|c| c.subject.as_str())
            .collect();
        assert_eq!(other, vec!["chore: add proj", "fix: b"], "{cl:?}");
    }
}
