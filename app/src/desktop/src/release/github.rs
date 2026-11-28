//! GitHub Releases for this project.
//!
//! A GitHub release publishes the public side of the workflow (the two-repo public repo, or the
//! project repo in single-repo mode): the `.proj` version (as `v<version>`), the generated
//! release notes as the body, and the release zip as an asset. Signing-in and token handling
//! all live in the `github` module; this is a thin layer over it.
//!
//! The repository is resolved from the public side's remote URL — only GitHub remotes are
//! supported. Wait-points: the HTTP plumbing (origin checks, redirect policy, rate limits) is
//! all in `crate::github::http`; this module adds the Releases API path construction and the
//! typed bodies.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{ReleaseError, public_root};
use crate::github::http::Reply;
use crate::github::{GitHub, GitHubError};
use crate::proj::ProjFile;

/// One asset on a GitHub release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReleaseAsset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    /// The asset's content type, when GitHub says one.
    pub content_type: Option<String>,
    pub browser_download_url: String,
}

/// A published (or draft) release of the public repository. Field names match GitHub's API
/// payload — there is no deliberate rename, and the bindings follow the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitHubRelease {
    pub id: u64,
    pub tag_name: String,
    /// The release's display name; GitHub shows the tag name when this is null.
    pub name: Option<String>,
    pub draft: bool,
    pub prerelease: bool,
    /// The release's web page, where the notes render.
    pub html_url: String,
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

/// "owner/repo" for the public side, parsed from its remote URL.
///
/// Only GitHub remotes are supported: HTTPS (`https://github.com/owner/repo.git`), SSH
/// (`git@github.com:owner/repo.git`, `ssh://git@github.com/owner/repo.git`) and friends. Any
/// other host, or a URL that does not parse to exactly one owner/repo pair, is refused — the
/// release can only be published to a repository a URL can name.
pub fn parse_repo_slug(remote_url: &str) -> Result<String, GitHubError> {
    let url = remote_url.trim();
    let bare = url.strip_suffix(".git").unwrap_or(url).trim();
    let Some((_, rest)) = bare.split_once("github.com") else {
        return Err(GitHubError::Unexpected(format!(
            "the public repo's remote {url:?} is not a GitHub URL"
        )));
    };
    let mut parts = rest
        .trim_start_matches(['/', ':'])
        .split('/')
        .filter(|s| !s.is_empty());
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() || parts.next().is_some() {
        return Err(GitHubError::Unexpected(format!(
            "could not parse the owner and repo from the public repo's remote URL {url:?}"
        )));
    }
    Ok(format!("{owner}/{repo}"))
}

/// The remote URL of the public side, read from git.
fn public_remote_url(root: &Path, proj: &ProjFile) -> Result<String, ReleaseError> {
    let public = public_root(root, proj)?;
    let remote = proj.workflow.public.remote.trim();
    let out = crate::git::GitCommand::new(&public)
        .arg("remote")
        .arg("get-url")
        .arg(remote)
        .output()?;
    if out.code != Some(0) {
        return Err(ReleaseError::Git(crate::git::GitError::Failed {
            command: "git remote get-url".into(),
            code: out.code,
            stderr: out.stderr.trim().to_string(),
        }));
    }
    Ok(out.stdout_str().trim().to_string())
}

/// The "owner/repo" slug of the public side, from its remote URL.
pub fn public_repo_slug(root: &Path) -> Result<String, ReleaseError> {
    let proj_path = crate::proj::find_proj_file(root)?;
    let proj = ProjFile::load(&proj_path)?;
    let url = public_remote_url(root, &proj)?;
    Ok(parse_repo_slug(&url)?)
}

fn repo_path(repo: &str, suffix: &str) -> String {
    format!("/repos/{repo}{suffix}")
}

/// Percent-encode a query-parameter value.
fn encode_query(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// List the public repo's releases (GitHub's order).
pub fn list_releases(github: &GitHub, root: &Path) -> Result<Vec<GitHubRelease>, ReleaseError> {
    let repo = public_repo_slug(root)?;
    let Reply {
        value: releases, ..
    } = github.api_get(&repo_path(&repo, "/releases"))?;
    Ok(releases)
}

/// Create a release. The tag defaults to `v<version>`; the name is left null (GitHub then shows
/// the tag name) unless one is given.
pub fn create_release(
    github: &GitHub,
    root: &Path,
    tag: Option<&str>,
    name: Option<&str>,
    body: &str,
    draft: bool,
    prerelease: bool,
) -> Result<GitHubRelease, ReleaseError> {
    let repo = public_repo_slug(root)?;
    let proj_path = crate::proj::find_proj_file(root)?;
    let proj = ProjFile::load(&proj_path)?;
    let tag = tag.map_or_else(|| format!("v{}", proj.project.version), |t| t.to_string());
    // GitHub rejects a null `name` ("nil is not a string"), so the field is omitted entirely;
    // left out, the release shows its tag name.
    let mut payload = serde_json::json!({
        "tag_name": tag,
        "body": body,
        "draft": draft,
        "prerelease": prerelease,
    });
    if let Some(n) = name {
        payload["name"] = serde_json::json!(n);
    }
    let Reply { value, .. } = github.post(&repo_path(&repo, "/releases"), &payload)?;
    Ok(value)
}

/// Edit a release. Only the given fields are sent (an absent `name` leaves the old one).
pub fn update_release(
    github: &GitHub,
    root: &Path,
    release_id: u64,
    name: Option<&str>,
    body: Option<&str>,
    draft: Option<bool>,
    prerelease: Option<bool>,
) -> Result<GitHubRelease, ReleaseError> {
    let repo = public_repo_slug(root)?;
    let mut patch = serde_json::json!({});
    if let Some(n) = name {
        patch["name"] = serde_json::json!(n);
    }
    if let Some(b) = body {
        patch["body"] = serde_json::json!(b);
    }
    if let Some(d) = draft {
        patch["draft"] = serde_json::json!(d);
    }
    if let Some(p) = prerelease {
        patch["prerelease"] = serde_json::json!(p);
    }
    let Reply { value, .. } = github.patch(
        &repo_path(&repo, &format!("/releases/{release_id}")),
        &patch,
    )?;
    Ok(value)
}

/// Delete a release.
pub fn delete_release(github: &GitHub, root: &Path, release_id: u64) -> Result<(), ReleaseError> {
    let repo = public_repo_slug(root)?;
    github.delete(&repo_path(&repo, &format!("/releases/{release_id}")))?;
    Ok(())
}

/// Upload the release zip (from the app data dir) as an asset on the release. Errors when the
/// zip is not built yet, or is missing.
pub fn upload_asset(
    github: &GitHub,
    root: &Path,
    data_dir: &Path,
    release_id: u64,
) -> Result<ReleaseAsset, ReleaseError> {
    let repo = public_repo_slug(root)?;
    let proj_path = crate::proj::find_proj_file(root)?;
    let proj = ProjFile::load(&proj_path)?;
    let file_name = proj.asset_file_name();
    let zip = data_dir.join("releases").join(&file_name);
    let data = std::fs::read(&zip).map_err(|e| ReleaseError::Io {
        path: zip.clone(),
        source: e,
    })?;
    let Reply { value, .. } = github.upload(
        &repo_path(
            &repo,
            &format!(
                "/releases/{release_id}/assets?name={}",
                encode_query(&file_name)
            ),
        ),
        "application/octet-stream",
        &data,
    )?;
    Ok(value)
}

/// Delete a single asset (used to replace the zip).
pub fn delete_asset(github: &GitHub, root: &Path, asset_id: u64) -> Result<(), ReleaseError> {
    let repo = public_repo_slug(root)?;
    github.delete(&repo_path(&repo, &format!("/releases/assets/{asset_id}")))?;
    Ok(())
}

/// A release's zip asset, if any (matched by file name).
pub fn zip_asset_of<'a>(release: &'a GitHubRelease, file_name: &str) -> Option<&'a ReleaseAsset> {
    release.assets.iter().find(|a| a.name == file_name)
}

/// The app data dir the release zip lands in.
pub fn zip_path(data_dir: &Path, file_name: &str) -> PathBuf {
    data_dir.join("releases").join(file_name)
}

/// Re-upload the zip by replacing the release's existing zip asset.
pub fn replace_asset(
    github: &GitHub,
    root: &Path,
    data_dir: &Path,
    release: &GitHubRelease,
) -> Result<ReleaseAsset, ReleaseError> {
    let proj_path = crate::proj::find_proj_file(root)?;
    let proj = ProjFile::load(&proj_path)?;
    let file_name = proj.asset_file_name();
    if let Some(asset) = zip_asset_of(release, &file_name) {
        delete_asset(github, root, asset.id)?;
    }
    upload_asset(github, root, data_dir, release.id)
}

#[cfg(test)]
mod tests;
