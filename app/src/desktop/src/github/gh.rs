//! Optional fallback: reuse the GitHub CLI's login (`gh auth token`) instead of signing in
//! separately. The token stays in `gh`'s own storage and is read again when needed; Project Manager does
//! not copy it into its keychain entry.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use ts_rs::TS;

use super::GitHubError;
use super::token::Token;

const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GhStatus {
    pub installed: bool,
    /// e.g. `2.62.0`.
    pub version: Option<String>,
    /// `gh` has a login for github.com.
    pub signed_in: bool,
}

pub struct GhCli {
    program: PathBuf,
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl GhCli {
    /// `gh` on PATH, or `PM_GH` when set (for tests and unusual installs).
    pub fn from_env() -> Self {
        GhCli {
            program: std::env::var_os("PM_GH").map_or_else(|| PathBuf::from("gh"), PathBuf::from),
        }
    }

    #[cfg(test)]
    pub fn at(program: impl Into<PathBuf>) -> Self {
        GhCli {
            program: program.into(),
        }
    }

    /// Run `gh` without a shell, prompts, colours or update checks, killed after [`TIMEOUT`].
    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        let mut cmd = Command::new(&self.program);
        crate::host_env::apply(&mut cmd);
        cmd.args(args)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_SPINNER_DISABLED", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        // Read both pipes on threads so a chatty `gh` cannot block on a full pipe.
        let mut out = child.stdout.take().expect("piped");
        let mut err = child.stderr.take().expect("piped");
        let out_t = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = out.by_ref().take(64 * 1024).read_to_string(&mut s);
            s
        });
        let err_t = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.by_ref().take(64 * 1024).read_to_string(&mut s);
            s
        });
        let start = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if start.elapsed() > TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "gh did not answer in time",
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Ok(Output {
            ok: status.success(),
            stdout: out_t.join().unwrap_or_default(),
            stderr: err_t.join().unwrap_or_default(),
        })
    }

    /// `gh --version` → `2.62.0`, or `None` when gh is not installed.
    pub fn version(&self) -> Option<String> {
        let out = self.run(&["--version"]).ok().filter(|o| o.ok)?;
        parse_version(&out.stdout)
    }

    /// The token of `gh`'s github.com login.
    pub fn token(&self) -> Result<Token, GitHubError> {
        let out = self
            .run(&["auth", "token", "--hostname", "github.com"])
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    GitHubError::Gh("The GitHub CLI (gh) is not installed".into())
                } else {
                    GitHubError::Gh(format!("Could not run the GitHub CLI: {e}"))
                }
            })?;
        if !out.ok {
            // stderr only: stdout would hold the token on success, and is never shown.
            let why = out.stderr.lines().next().unwrap_or("").trim().to_string();
            let why: String = why.chars().take(200).collect();
            return Err(GitHubError::Gh(if why.is_empty() {
                "The GitHub CLI is not signed in; run `gh auth login`".to_string()
            } else {
                format!("The GitHub CLI is not signed in ({why}); run `gh auth login`")
            }));
        }
        Token::new(out.stdout)
            .map_err(|_| GitHubError::Gh("The GitHub CLI returned no usable token".into()))
    }

    pub fn status(&self) -> GhStatus {
        let version = self.version();
        let signed_in = version.is_some() && self.token().is_ok();
        GhStatus {
            installed: version.is_some(),
            version,
            signed_in,
        }
    }
}

/// `gh version 2.62.0 (2024-11-14)` → `2.62.0`.
pub(crate) fn parse_version(out: &str) -> Option<String> {
    let rest = out.lines().next()?.trim().strip_prefix("gh version ")?;
    let v = rest.split_whitespace().next()?;
    (v.chars().next()?.is_ascii_digit()).then(|| v.to_string())
}
