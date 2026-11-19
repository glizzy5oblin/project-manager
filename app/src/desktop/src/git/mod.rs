//! Git access. Reads use gix (in-process, fast); writes go through the git CLI so they behave
//! exactly like the user's own git, including hooks, credential helpers and SSH config.

mod runner;
pub mod state;
mod trust;
pub mod url;
mod version;
pub mod watch;

use std::path::PathBuf;

pub use runner::{CancelToken, GitCommand, GitOutput, Progress, auth_failure_hint};
pub use trust::{ensure_trusted, is_trusted};
pub use version::{GitInstall, GitVersion, MIN_GIT_VERSION, detect, detect_program, git_program};

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("git was not found ({}). Please install git {MIN_GIT_VERSION} or newer and make sure it is on PATH", program.display())]
    NotInstalled { program: PathBuf },

    #[error(
        "git {found} is too old; Project Manager needs git {minimum} or newer. Please update git"
    )]
    TooOld {
        found: GitVersion,
        minimum: GitVersion,
    },

    #[error("could not understand `git --version` output: {output:?}")]
    UnrecognisedVersion { output: String },

    #[error("could not run {}: {source}", program.display())]
    Spawn {
        program: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("`{command}` failed{}: {stderr}", code.map(|c| format!(" (exit {c})")).unwrap_or_default())]
    Failed {
        command: String,
        code: Option<i32>,
        stderr: String,
    },

    #[error("could not read the repository at {}: {message}", path.display())]
    Read { path: PathBuf, message: String },

    #[error("`{command}` could not authenticate: {hint}")]
    AuthFailed {
        command: String,
        stderr: String,
        hint: String,
    },

    #[error("{} is owned by another user. Running git commands there could run programs its config names. Add it to git's safe.directory, or confirm to continue", path.display())]
    Untrusted { path: PathBuf },

    #[error("`{command}` was cancelled")]
    Cancelled { command: String },
}

#[cfg(test)]
mod tests;
