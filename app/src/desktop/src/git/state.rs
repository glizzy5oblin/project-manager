//! Repository state read in-process with gix: HEAD, branches with upstream ahead/behind, remote
//! branches, tags, remotes, working-tree status and any operation in progress.
//!
//! Reads go through gix's trust model: a repository owned by another user is opened with
//! reduced trust, so its config cannot point programs at us, and `trusted` is `false` so the UI
//! can warn before any write command runs on it.

use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use serde::Serialize;
use ts_rs::TS;

use super::GitError;
use super::url::redact;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RepoState {
    /// Working tree root (the git dir for a bare repository).
    #[ts(type = "string")]
    pub root: PathBuf,
    pub bare: bool,
    /// `false` when the repository is owned by another user. Write commands must warn first.
    pub trusted: bool,
    pub head: HeadState,
    pub branches: Vec<BranchInfo>,
    pub remote_branches: Vec<RefInfo>,
    pub tags: Vec<TagInfo>,
    pub remotes: Vec<RemoteInfo>,
    /// `None` for bare repositories.
    pub status: Option<StatusSummary>,
    pub in_progress: Option<Operation>,
    /// `.gitmodules` exists. Submodules are detected and warned about only (agreed policy).
    pub has_submodules: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct HeadState {
    /// Short branch name, `None` when detached.
    pub branch: Option<String>,
    /// Commit id, `None` on an unborn branch (no commits yet).
    pub commit: Option<String>,
    pub detached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub commit: String,
    pub is_head: bool,
    /// Short tracking-branch name, e.g. `origin/main`.
    pub upstream: Option<String>,
    /// Commits on this branch not on the upstream; `None` without an upstream or when the
    /// upstream ref is missing (not fetched yet).
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RefInfo {
    pub name: String,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TagInfo {
    pub name: String,
    /// The commit the tag points to after peeling.
    pub commit: String,
    pub annotated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInfo {
    pub name: String,
    /// Credentials embedded in the URL are redacted.
    pub fetch_url: Option<String>,
    pub push_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChange,
    Conflicted,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    /// Repository-relative path with `/` separators.
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StatusSummary {
    /// HEAD tree to index.
    pub staged: Vec<FileChange>,
    /// Index to working tree, tracked files only.
    pub unstaged: Vec<FileChange>,
    pub untracked: Vec<String>,
    pub conflicted: Vec<String>,
}

impl StatusSummary {
    /// Any change at all, including untracked files.
    pub fn is_dirty(&self) -> bool {
        !(self.staged.is_empty()
            && self.unstaged.is_empty()
            && self.untracked.is_empty()
            && self.conflicted.is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
    ApplyMailbox,
}

fn read_err(path: &Path, e: impl std::fmt::Display) -> GitError {
    GitError::Read {
        path: path.to_path_buf(),
        message: redact(&e.to_string()),
    }
}

/// Open a repository at exactly `path` (no upward discovery, so a project folder never
/// silently resolves to a parent repository).
pub fn open(path: &Path) -> Result<gix::Repository, GitError> {
    let opts = gix::open::Options::default();
    gix::open_opts(path, opts).map_err(|e| read_err(path, e))
}

/// Read everything the hub and project views need. `with_status` controls the (comparatively
/// slow) working tree scan.
pub fn read_state(path: &Path, with_status: bool) -> Result<RepoState, GitError> {
    let repo = open(path)?;
    let root = repo
        .workdir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo.path().to_path_buf());
    let trusted = matches!(
        gix::sec::Trust::from_path_ownership(repo.path()),
        Ok(gix::sec::Trust::Full)
    );

    let head_name = repo.head_name().map_err(|e| read_err(path, e))?;
    let head_commit = repo.head_id().ok().map(|id| id.to_string());
    let head = HeadState {
        branch: head_name.as_ref().map(|n| n.shorten().to_string()),
        commit: head_commit,
        detached: head_name.is_none(),
    };

    let refs = repo.references().map_err(|e| read_err(path, e))?;

    let mut branches = Vec::new();
    for r in refs.local_branches().map_err(|e| read_err(path, e))? {
        let mut r = r.map_err(|e| read_err(path, e))?;
        let Ok(id) = r.peel_to_id() else { continue };
        let id = id.detach();
        let full = r.name().to_owned();
        let upstream_full = upstream_of(&repo, full.as_ref());
        let upstream_id = upstream_full.as_ref().and_then(|u| {
            repo.find_reference(u.as_ref())
                .ok()
                .and_then(|mut r| r.peel_to_id().ok().map(|i| i.detach()))
        });
        let (ahead, behind) = match upstream_id {
            Some(up) => {
                let (a, b) = ahead_behind(&repo, id, up).map_err(|e| read_err(path, e))?;
                (Some(a), Some(b))
            }
            None => (None, None),
        };
        branches.push(BranchInfo {
            name: full.shorten().to_string(),
            commit: id.to_string(),
            is_head: head_name.as_ref() == Some(&full),
            upstream: upstream_full.map(|u| u.shorten().to_string()),
            ahead,
            behind,
        });
    }

    let mut remote_branches = Vec::new();
    for r in refs.remote_branches().map_err(|e| read_err(path, e))? {
        let mut r = r.map_err(|e| read_err(path, e))?;
        let name = r.name().shorten().to_string();
        // `origin/HEAD` is a symbolic pointer, not a branch.
        if name.ends_with("/HEAD") {
            continue;
        }
        if let Ok(id) = r.peel_to_id() {
            remote_branches.push(RefInfo {
                name,
                commit: id.to_string(),
            });
        }
    }

    let mut tags = Vec::new();
    for r in refs.tags().map_err(|e| read_err(path, e))? {
        let mut r = r.map_err(|e| read_err(path, e))?;
        let name = r.name().shorten().to_string();
        let annotated = r
            .target()
            .try_id()
            .and_then(|id| repo.find_object(id).ok())
            .is_some_and(|o| o.kind == gix::object::Kind::Tag);
        if let Ok(id) = r.peel_to_id() {
            tags.push(TagInfo {
                name,
                commit: id.to_string(),
                annotated,
            });
        }
    }

    let mut remotes = Vec::new();
    for name in repo.remote_names() {
        let Ok(remote) = repo.find_remote(name.as_bstr()) else {
            continue;
        };
        let url = |d| {
            remote
                .url(d)
                .map(|u| redact(&u.to_bstring().to_str_lossy()))
        };
        remotes.push(RemoteInfo {
            name: name.to_string(),
            fetch_url: url(gix::remote::Direction::Fetch),
            push_url: url(gix::remote::Direction::Push),
        });
    }

    let status = if with_status && !repo.is_bare() {
        Some(read_status(&repo).map_err(|e| read_err(path, e))?)
    } else {
        None
    };

    let in_progress = repo.state().map(|s| {
        use gix::state::InProgress::*;
        match s {
            ApplyMailbox | ApplyMailboxRebase => Operation::ApplyMailbox,
            Bisect => Operation::Bisect,
            CherryPick | CherryPickSequence => Operation::CherryPick,
            Merge => Operation::Merge,
            Rebase | RebaseInteractive => Operation::Rebase,
            Revert | RevertSequence => Operation::Revert,
        }
    });

    Ok(RepoState {
        has_submodules: repo
            .workdir()
            .is_some_and(|w| w.join(".gitmodules").is_file()),
        root,
        bare: repo.is_bare(),
        trusted,
        head,
        branches,
        remote_branches,
        tags,
        remotes,
        status,
        in_progress,
    })
}

/// The full name of a branch's upstream: the remote-tracking ref for a normal remote, or the
/// local branch itself when `branch.<name>.remote` is `.`.
fn upstream_of(
    repo: &gix::Repository,
    branch: &gix::refs::FullNameRef,
) -> Option<gix::refs::FullName> {
    use gix::remote::Direction::Fetch;
    let local_remote = repo
        .branch_remote_name(branch.shorten(), Fetch)
        .is_some_and(|n| n.as_bstr() == ".");
    if local_remote {
        repo.branch_remote_ref_name(branch, Fetch)?.ok()
    } else {
        repo.branch_remote_tracking_ref_name(branch, Fetch)?.ok()
    }
}

/// (ahead, behind) of `local` relative to `upstream`.
pub fn ahead_behind(
    repo: &gix::Repository,
    local: gix::ObjectId,
    upstream: gix::ObjectId,
) -> Result<(u32, u32), Box<dyn std::error::Error + Send + Sync>> {
    if local == upstream {
        return Ok((0, 0));
    }
    let count = |tip: gix::ObjectId,
                 hidden: gix::ObjectId|
     -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
        let mut n = 0u32;
        for info in repo.rev_walk([tip]).with_hidden([hidden]).all()? {
            info?;
            n += 1;
        }
        Ok(n)
    };
    Ok((count(local, upstream)?, count(upstream, local)?))
}

fn path_string(p: &gix::bstr::BStr) -> String {
    p.to_str_lossy().into_owned()
}

fn read_status(
    repo: &gix::Repository,
) -> Result<StatusSummary, Box<dyn std::error::Error + Send + Sync>> {
    use gix::status::index_worktree::Item as Wt;
    use gix_status_summary as S;

    let mut out = StatusSummary::default();
    let iter = repo
        .status(gix::progress::Discard)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .into_iter(None)?;
    for item in iter {
        let item = item?;
        match item {
            gix::status::Item::TreeIndex(change) => {
                use gix::diff::index::ChangeRef::*;
                let kind = match &change {
                    Addition { .. } => ChangeKind::Added,
                    Deletion { .. } => ChangeKind::Deleted,
                    Modification { .. } => ChangeKind::Modified,
                    Rewrite { .. } => ChangeKind::Renamed,
                };
                out.staged.push(FileChange {
                    path: path_string(change.location()),
                    kind,
                });
            }
            gix::status::Item::IndexWorktree(wt) => {
                let path = path_string(wt.rela_path());
                let Some(summary) = wt.summary() else {
                    continue;
                };
                let kind = match summary {
                    S::Added => {
                        if matches!(wt, Wt::DirectoryContents { .. }) {
                            out.untracked.push(path);
                            continue;
                        }
                        ChangeKind::Added
                    }
                    S::Conflict => {
                        if !out.conflicted.contains(&path) {
                            out.conflicted.push(path);
                        }
                        continue;
                    }
                    S::Removed => ChangeKind::Deleted,
                    S::Modified => ChangeKind::Modified,
                    S::TypeChange => ChangeKind::TypeChange,
                    S::Renamed | S::Copied => ChangeKind::Renamed,
                    S::IntentToAdd => ChangeKind::Added,
                };
                out.unstaged.push(FileChange { path, kind });
            }
        }
    }
    // Conflicted paths also show up as staged changes; list them once, as conflicts.
    out.staged.retain(|c| !out.conflicted.contains(&c.path));
    out.staged.sort_by(|a, b| a.path.cmp(&b.path));
    out.unstaged.sort_by(|a, b| a.path.cmp(&b.path));
    out.untracked.sort();
    out.conflicted.sort();
    Ok(out)
}

use gix::status::index_worktree::iter::Summary as gix_status_summary;
