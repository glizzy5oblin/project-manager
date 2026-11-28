//! Find the git binary and check its version against the supported minimum.

use std::fmt;
use std::path::PathBuf;
use std::process::Command;

use super::GitError;

/// Oldest git Project Manager supports.
pub const MIN_GIT_VERSION: GitVersion = GitVersion {
    major: 2,
    minor: 30,
    patch: 0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl fmt::Display for GitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The git binary that was found and checked.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct GitInstall {
    /// The program run (`git`, or the `PM_GIT` override).
    #[ts(type = "string")]
    pub program: PathBuf,
    pub version: GitVersion,
    /// Full `git --version` output, e.g. `git version 2.45.1.windows.1`.
    pub raw: String,
}

impl GitVersion {
    /// Parse `git --version` output. Accepts vendor suffixes such as `2.39.3 (Apple Git-146)` and
    /// `2.45.1.windows.1`, and missing patch numbers (`2.30`).
    pub fn parse(output: &str) -> Option<GitVersion> {
        let rest = output.trim().strip_prefix("git version ")?;
        let token = rest.split_whitespace().next()?;
        let mut nums = token.split('.').map(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        });
        let major = nums.next()??;
        let minor = nums.next()??;
        let patch = nums.next().flatten().unwrap_or(0);
        Some(GitVersion {
            major,
            minor,
            patch,
        })
    }
}

/// The git program to run: `PM_GIT` when set (for testing other versions), else `git` on PATH.
pub fn git_program() -> PathBuf {
    std::env::var_os("PM_GIT").map_or_else(|| PathBuf::from("git"), PathBuf::from)
}

/// Run `git --version` and check the minimum. Errors say what to do next.
pub fn detect() -> Result<GitInstall, GitError> {
    detect_program(git_program())
}

pub fn detect_program(program: PathBuf) -> Result<GitInstall, GitError> {
    let output = crate::host_env::apply(&mut Command::new(&program))
        .arg("--version")
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GitError::NotInstalled {
                    program: program.clone(),
                }
            } else {
                GitError::Spawn {
                    program: program.clone(),
                    source: e,
                }
            }
        })?;
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let version = GitVersion::parse(&raw).ok_or_else(|| GitError::UnrecognisedVersion {
        output: raw.clone(),
    })?;
    if version < MIN_GIT_VERSION {
        return Err(GitError::TooOld {
            found: version,
            minimum: MIN_GIT_VERSION,
        });
    }
    Ok(GitInstall {
        program,
        version,
        raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(major: u32, minor: u32, patch: u32) -> GitVersion {
        GitVersion {
            major,
            minor,
            patch,
        }
    }

    #[test]
    fn parses_common_formats() {
        assert_eq!(GitVersion::parse("git version 2.56.1\n"), Some(v(2, 56, 1)));
        assert_eq!(
            GitVersion::parse("git version 2.39.3 (Apple Git-146)"),
            Some(v(2, 39, 3))
        );
        assert_eq!(
            GitVersion::parse("git version 2.45.1.windows.1"),
            Some(v(2, 45, 1))
        );
        assert_eq!(GitVersion::parse("git version 2.30"), Some(v(2, 30, 0)));
        assert_eq!(
            GitVersion::parse("git version 2.43.0.rc1"),
            Some(v(2, 43, 0))
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(GitVersion::parse(""), None);
        assert_eq!(GitVersion::parse("hg version 6.0"), None);
        assert_eq!(GitVersion::parse("git version x.y"), None);
    }

    #[test]
    fn ordering_against_minimum() {
        assert!(v(2, 29, 9) < MIN_GIT_VERSION);
        assert!(v(2, 30, 0) >= MIN_GIT_VERSION);
        assert!(v(3, 0, 0) > MIN_GIT_VERSION);
    }

    #[test]
    fn detects_installed_git() {
        let git = detect().expect("git must be installed to run the tests");
        assert!(git.version >= MIN_GIT_VERSION);
    }

    #[test]
    fn missing_binary_is_reported_as_not_installed() {
        let err = detect_program(PathBuf::from("definitely-not-a-git-binary-xyz")).unwrap_err();
        assert!(matches!(err, GitError::NotInstalled { .. }), "{err:?}");
        assert!(err.to_string().contains("install git"), "{err}");
    }
}
