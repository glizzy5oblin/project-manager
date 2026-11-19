//! The one error type the frontend sees. `kind` drives the UI (a missing git gets an install
//! hint, an untrusted repo gets a confirm dialog); `message` is ready to show as is.

use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

use crate::backup::BackupError;
use crate::git::GitError;
use crate::git::url::UrlError;
use crate::github::GitHubError;
use crate::proj::{Issue, ProjError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// git is not installed or not on PATH.
    GitMissing,
    /// git is older than the supported minimum.
    GitTooOld,
    /// A git command failed; `detail` has its stderr.
    Git,
    /// A remote rejected the credentials, or none were available.
    Auth,
    /// The repository is owned by another user; the action needs explicit confirmation.
    Untrusted,
    Cancelled,
    /// Another task is already working on this repository.
    Busy,
    /// No such project, backup, file or folder.
    NotFound,
    /// The folder is not a git repository.
    NotARepo,
    /// The repository has no `.proj` file; the UI offers to create one.
    NoProjFile,
    /// The `.proj` file is invalid; `issues` lists the problems.
    InvalidProject,
    /// The target (folder, project) already exists.
    AlreadyExists,
    /// An argument from the frontend was rejected.
    InvalidInput,
    /// The request named data that has since changed (e.g. a graph generation); reload it.
    Stale,
    /// GitHub could not be reached.
    Network,
    /// GitHub's rate limit was reached; the message says when to try again.
    RateLimited,
    /// The action needs a GitHub account and none is signed in.
    NotSignedIn,
    /// A needed service is missing: the system keychain, the GitHub CLI, or this build's
    /// GitHub sign-in setup.
    Unavailable,
    Io,
    /// A bug: something that should not happen.
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
    /// Extra text for a "details" disclosure, e.g. git's stderr.
    pub detail: Option<String>,
    /// `.proj` validation problems, each located by key path.
    pub issues: Vec<ProjIssue>,
}

/// Mirror of [`crate::proj::Issue`] for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ProjIssue {
    pub key: String,
    pub message: String,
}

impl From<&Issue> for ProjIssue {
    fn from(i: &Issue) -> Self {
        ProjIssue {
            key: i.key.clone(),
            message: i.message.clone(),
        }
    }
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        AppError {
            kind,
            message: message.into(),
            detail: None,
            issues: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let d = detail.into();
        self.detail = (!d.trim().is_empty()).then_some(d);
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    pub fn io(path: &Path, e: &std::io::Error) -> Self {
        let kind = match e.kind() {
            std::io::ErrorKind::NotFound => ErrorKind::NotFound,
            std::io::ErrorKind::AlreadyExists => ErrorKind::AlreadyExists,
            _ => ErrorKind::Io,
        };
        Self::new(kind, format!("{}: {e}", path.display()))
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

impl From<GitError> for AppError {
    fn from(e: GitError) -> Self {
        let message = e.to_string();
        match e {
            GitError::NotInstalled { .. } | GitError::Spawn { .. } => {
                AppError::new(ErrorKind::GitMissing, message)
            }
            GitError::TooOld { .. } | GitError::UnrecognisedVersion { .. } => {
                AppError::new(ErrorKind::GitTooOld, message)
            }
            GitError::Io { path, source } => AppError::io(&path, &source),
            GitError::Failed {
                command, stderr, ..
            } => AppError::new(ErrorKind::Git, format!("`{command}` failed")).with_detail(stderr),
            GitError::Read { .. } => AppError::new(ErrorKind::Git, message),
            GitError::AuthFailed { hint, stderr, .. } => {
                AppError::new(ErrorKind::Auth, hint).with_detail(stderr)
            }
            GitError::Untrusted { .. } => AppError::new(ErrorKind::Untrusted, message),
            GitError::Cancelled { .. } => AppError::new(ErrorKind::Cancelled, "Cancelled"),
        }
    }
}

impl From<BackupError> for AppError {
    fn from(e: BackupError) -> Self {
        let message = e.to_string();
        match e {
            BackupError::Git(g) => g.into(),
            BackupError::Io { path, source } => AppError::io(&path, &source),
            BackupError::NotFound(_) | BackupError::NoBundle(_) => {
                AppError::new(ErrorKind::NotFound, message)
            }
            BackupError::BadAction(_) | BackupError::UnsafePath { .. } => {
                AppError::new(ErrorKind::InvalidInput, message)
            }
            BackupError::Index { .. } => AppError::new(ErrorKind::Io, message),
        }
    }
}

impl From<ProjError> for AppError {
    fn from(e: ProjError) -> Self {
        let message = e.to_string();
        match e {
            ProjError::Io { path, source } => AppError::io(&path, &source),
            ProjError::NotFound(dir) => AppError::new(
                ErrorKind::NoProjFile,
                format!("{} has no .proj file", dir.display()),
            ),
            ProjError::Invalid(issues) => {
                let mut err = AppError::new(ErrorKind::InvalidProject, "The .proj file is invalid");
                err.issues = issues.iter().map(ProjIssue::from).collect();
                err
            }
            ProjError::Parse { .. }
            | ProjError::UnsupportedSchema { .. }
            | ProjError::Ambiguous { .. } => AppError::new(ErrorKind::InvalidProject, message),
        }
    }
}

impl From<UrlError> for AppError {
    fn from(e: UrlError) -> Self {
        AppError::invalid(e.to_string())
    }
}

impl From<GitHubError> for AppError {
    fn from(e: GitHubError) -> Self {
        let message = e.to_string();
        let kind = match e {
            GitHubError::Unauthorized
            | GitHubError::Denied
            | GitHubError::Expired
            | GitHubError::Forbidden(_) => ErrorKind::Auth,
            GitHubError::NotConfigured(_) | GitHubError::Keychain(_) | GitHubError::Gh(_) => {
                ErrorKind::Unavailable
            }
            GitHubError::NotSignedIn => ErrorKind::NotSignedIn,
            GitHubError::NotFound => ErrorKind::NotFound,
            GitHubError::RateLimited { .. } => ErrorKind::RateLimited,
            GitHubError::Network(_) | GitHubError::Unexpected(_) => ErrorKind::Network,
            GitHubError::Cancelled => ErrorKind::Cancelled,
        };
        AppError::new(kind, message)
    }
}
