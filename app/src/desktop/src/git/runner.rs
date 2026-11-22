//! Runs git CLI commands (used for all writes): structured results, cancellation, streamed
//! progress from stderr, and an environment that never waits for terminal input.
//!
//! Credentials are left to the user's own setup: configured credential helpers (which may show
//! their own login windows) and SSH config/agent keep working. Only git's terminal prompts are
//! disabled, because a GUI app has no terminal to answer them.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::GitError;
use super::url::redact;
use super::version::git_program;

/// Shared flag to cancel a running command. Clone it into the task that may cancel.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// One progress line from git's stderr, e.g. `Receiving objects:  45% (450/1000)`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Progress {
    /// Text before the colon, e.g. `Receiving objects`. Empty for plain messages.
    pub phase: String,
    pub percent: Option<u8>,
    pub current: Option<u64>,
    pub total: Option<u64>,
    /// The whole line as git printed it.
    pub line: String,
}

impl Progress {
    pub fn parse(line: &str) -> Progress {
        let line = line.trim_end().to_string();
        let body = line.strip_prefix("remote: ").unwrap_or(&line);
        let (phase, rest) = match body.split_once(": ") {
            Some((p, r)) if !p.is_empty() && p.len() < 60 => (p.trim().to_string(), r),
            _ => (String::new(), body),
        };
        let percent = rest
            .trim_start()
            .split('%')
            .next()
            .filter(|p| p.len() < rest.trim_start().len())
            .and_then(|p| p.trim().parse().ok());
        let (current, total) = match (rest.find('('), rest.find(')')) {
            (Some(open), Some(close)) if open < close => {
                match rest[open + 1..close].split_once('/') {
                    Some((c, t)) => (c.trim().parse().ok(), t.trim().parse().ok()),
                    None => (None, None),
                }
            }
            _ => (None, None),
        };
        Progress {
            phase: if percent.is_some() || current.is_some() {
                phase
            } else {
                String::new()
            },
            percent,
            current,
            total,
            line,
        }
    }
}

/// Result of a finished git command.
#[derive(Debug, Clone)]
pub struct GitOutput {
    /// The command as it would be typed, for the operation log.
    pub command: String,
    /// Exit code; `None` if killed by a signal.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub duration: Duration,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
    /// `Ok(self)` on exit code 0, otherwise the failure as an error carrying stderr (with a
    /// sign-in hint when it was an authentication failure).
    pub fn into_result(self) -> Result<GitOutput, GitError> {
        if self.success() {
            Ok(self)
        } else if let Some(hint) = auth_failure_hint(&self.stderr) {
            Err(GitError::AuthFailed {
                command: self.command,
                stderr: self.stderr.trim().to_string(),
                hint,
            })
        } else {
            Err(GitError::Failed {
                command: self.command,
                code: self.code,
                stderr: self.stderr.trim().to_string(),
            })
        }
    }

    /// Stdout as text, trailing newline removed.
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout)
            .trim_end_matches(['\n', '\r'])
            .to_string()
    }
}

/// A git invocation. Build it, then call [`run`](Self::run) or [`output`](Self::output).
#[derive(Debug, Clone)]
pub struct GitCommand {
    program: PathBuf,
    cwd: PathBuf,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
    stdin: Option<Vec<u8>>,
}

impl GitCommand {
    pub fn new(cwd: impl AsRef<Path>) -> Self {
        GitCommand {
            program: git_program(),
            cwd: cwd.as_ref().to_path_buf(),
            args: Vec::new(),
            envs: Vec::new(),
            stdin: None,
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.envs
            .push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    /// Display form, e.g. `git push origin main`, with URL credentials redacted.
    pub fn display(&self) -> String {
        redact(&self.display_raw())
    }

    fn display_raw(&self) -> String {
        let mut s = String::from("git");
        for a in &self.args {
            let a = a.to_string_lossy();
            if a.is_empty() || a.contains([' ', '"', '\'']) {
                s.push_str(&format!(" \"{}\"", a.replace('"', "\\\"")));
            } else {
                s.push(' ');
                s.push_str(&a);
            }
        }
        s
    }

    /// Run to completion; a non-zero exit is an error carrying stderr.
    pub fn run(&self) -> Result<GitOutput, GitError> {
        self.run_with(&CancelToken::new(), |_| {})
    }

    /// Run with cancellation and a callback for each stderr progress line. A non-zero exit is an
    /// error. Pass `--progress` yourself for commands that only print progress to a terminal.
    pub fn run_with(
        &self,
        cancel: &CancelToken,
        on_progress: impl FnMut(Progress),
    ) -> Result<GitOutput, GitError> {
        self.output_with(cancel, on_progress)?.into_result()
    }

    /// Run to completion and return the output whatever the exit code.
    pub fn output(&self) -> Result<GitOutput, GitError> {
        self.output_with(&CancelToken::new(), |_| {})
    }

    pub fn output_with(
        &self,
        cancel: &CancelToken,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<GitOutput, GitError> {
        if cancel.is_cancelled() {
            return Err(GitError::Cancelled {
                command: self.display(),
            });
        }
        let started = Instant::now();
        let mut cmd = self.build();
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound && !self.cwd.exists() {
                GitError::Io {
                    path: self.cwd.clone(),
                    source: e,
                }
            } else if e.kind() == std::io::ErrorKind::NotFound {
                GitError::NotInstalled {
                    program: self.program.clone(),
                }
            } else {
                GitError::Spawn {
                    program: self.program.clone(),
                    source: e,
                }
            }
        })?;

        // Feed stdin on its own thread so a large input cannot deadlock against full pipes.
        let stdin_thread = match (self.stdin.clone(), child.stdin.take()) {
            (Some(bytes), Some(mut pipe)) => Some(std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            })),
            _ => None,
        };
        let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
        let stdout_thread = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut buf);
            buf
        });
        let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
        let (tx, rx) = mpsc::channel::<String>();
        let stderr_thread = std::thread::spawn(move || {
            let mut all = Vec::new();
            let mut line = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr_pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                all.extend_from_slice(&chunk[..n]);
                for &b in &chunk[..n] {
                    // Progress meters redraw with \r; treat it as a line end.
                    if b == b'\n' || b == b'\r' {
                        if !line.is_empty() {
                            let _ = tx.send(redact(&String::from_utf8_lossy(&line)));
                            line.clear();
                        }
                    } else {
                        line.push(b);
                    }
                }
            }
            if !line.is_empty() {
                let _ = tx.send(redact(&String::from_utf8_lossy(&line)));
            }
            redact(&String::from_utf8_lossy(&all))
        });

        let status = loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(l) => on_progress(Progress::parse(&l)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // stderr closed; keep waiting for exit below.
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            if cancel.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitError::Cancelled {
                    command: self.display(),
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => {
                    return Err(GitError::Spawn {
                        program: self.program.clone(),
                        source: e,
                    });
                }
            }
        };

        let stderr = stderr_thread.join().unwrap_or_default();
        for l in rx.try_iter() {
            on_progress(Progress::parse(&l));
        }
        let stdout = stdout_thread.join().unwrap_or_default();
        if let Some(t) = stdin_thread {
            let _ = t.join();
        }
        Ok(GitOutput {
            command: self.display(),
            code: status.code(),
            stdout,
            stderr,
            duration: started.elapsed(),
        })
    }

    fn build(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.current_dir(&self.cwd)
            // Stable, parseable output with no pager, colour or path quoting.
            .args([
                "--no-pager",
                "-c",
                "color.ui=never",
                "-c",
                "core.quotepath=off",
                // Security: the `ext::` transport runs an arbitrary command from a URL; a
                // malicious remote or submodule URL must never be able to use it.
                "-c",
                "protocol.ext.allow=never",
            ])
            .args(&self.args)
            // Security: allow-list transports. This overrides every config scope, including a
            // later `-c protocol.*.allow`, so `ext::` (runs a command from a URL) and other
            // exotic transports can never be reached through a malicious remote or submodule.
            .env("GIT_ALLOW_PROTOCOL", "file:git:http:https:ssh")
            .env("LC_ALL", "C")
            .env("LANGUAGE", "C")
            // Never block on a terminal prompt; credential helpers still run.
            .env("GIT_TERMINAL_PROMPT", "0")
            // ":" is git's built-in no-op editor: commands that would open an editor keep the
            // prepared message instead of waiting.
            .env("GIT_EDITOR", ":")
            .env("GIT_SEQUENCE_EDITOR", ":")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(if self.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }
}

/// Recognise authentication failures in git's stderr and say what to do about them. Git never
/// prompts in a terminal here, so these come from missing or rejected credentials.
pub fn auth_failure_hint(stderr: &str) -> Option<String> {
    let s = stderr.to_ascii_lowercase();
    let https = [
        "authentication failed",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "invalid username or password",
        "http basic: access denied",
        "the requested url returned error: 401",
        "the requested url returned error: 403",
    ];
    // Not "could not read from remote repository": git prints that after every ssh or local
    // transport failure, including a missing repository or a refused connection.
    let ssh = [
        "permission denied (publickey",
        "permission denied, please try again",
        "host key verification failed",
        "no more authentication methods",
        "too many authentication failures",
        "sign_and_send_pubkey: signing failed",
    ];
    if https.iter().any(|p| s.contains(p)) {
        Some(
            "HTTPS authentication failed. Project Manager uses your git credential helper: sign in with \
             it (for GitHub, Git Credential Manager or `gh auth setup-git`), or switch the \
             remote to SSH."
                .into(),
        )
    } else if ssh.iter().any(|p| s.contains(p)) {
        Some(
            "SSH authentication failed. Project Manager uses your SSH config and agent: check that your \
             key is loaded (`ssh-add -l`) and that `ssh -T` to the host works in a terminal, \
             and that the host key is already known."
                .into(),
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_lines() {
        let p = Progress::parse("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s");
        assert_eq!(p.phase, "Receiving objects");
        assert_eq!(
            (p.percent, p.current, p.total),
            (Some(45), Some(450), Some(1000))
        );

        let p = Progress::parse("remote: Counting objects: 100% (12/12), done.");
        assert_eq!(p.phase, "Counting objects");
        assert_eq!(p.percent, Some(100));

        let p = Progress::parse("Cloning into 'x'...");
        assert_eq!((p.phase.as_str(), p.percent, p.current), ("", None, None));
        assert_eq!(p.line, "Cloning into 'x'...");
    }

    #[test]
    fn display_quotes_spaces() {
        let c = GitCommand::new(".").args(["commit", "-m", "two words"]);
        assert_eq!(c.display(), "git commit -m \"two words\"");
    }

    #[test]
    fn runs_and_captures_output() {
        let dir = tempfile::tempdir().unwrap();
        GitCommand::new(dir.path())
            .args(["init", "-q"])
            .run()
            .unwrap();
        let out = GitCommand::new(dir.path())
            .args(["rev-parse", "--is-inside-work-tree"])
            .run()
            .unwrap();
        assert_eq!(out.stdout_str(), "true");
    }

    #[test]
    fn failure_carries_stderr_and_command() {
        let dir = tempfile::tempdir().unwrap();
        let err = GitCommand::new(dir.path()).args(["log"]).run().unwrap_err();
        match err {
            GitError::Failed {
                command,
                code,
                stderr,
            } => {
                assert_eq!(command, "git log");
                assert_eq!(code, Some(128));
                assert!(stderr.contains("not a git repository"), "{stderr}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn stdin_is_passed() {
        let dir = tempfile::tempdir().unwrap();
        GitCommand::new(dir.path())
            .args(["init", "-q"])
            .run()
            .unwrap();
        let out = GitCommand::new(dir.path())
            .args(["hash-object", "--stdin"])
            .stdin("hello\n")
            .run()
            .unwrap();
        assert_eq!(out.stdout_str(), "ce013625030ba8dba906f756967f9e9ca394464a");
    }

    #[test]
    fn already_cancelled_does_not_run() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = GitCommand::new(".")
            .arg("version")
            .run_with(&cancel, |_| {})
            .unwrap_err();
        assert!(matches!(err, GitError::Cancelled { .. }));
    }

    #[test]
    fn cancel_kills_a_running_command() {
        let cancel = CancelToken::new();
        let c2 = cancel.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            c2.cancel();
        });
        // A shell alias that sleeps far longer than the test waits (git for Windows ships sh).
        let started = Instant::now();
        let err = GitCommand::new(".")
            .args(["-c", "alias.pmwait=!sleep 30", "pmwait"])
            .run_with(&cancel, |_| {})
            .unwrap_err();
        t.join().unwrap();
        assert!(matches!(err, GitError::Cancelled { .. }), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "cancel took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn no_terminal_prompt_for_credentials() {
        // An HTTPS remote that needs auth must fail fast instead of waiting for a username.
        let dir = tempfile::tempdir().unwrap();
        GitCommand::new(dir.path())
            .args(["init", "-q"])
            .run()
            .unwrap();
        let started = Instant::now();
        let out = GitCommand::new(dir.path())
            .args(["ls-remote", "http://127.0.0.1:9/nothing.git"])
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            .output()
            .unwrap();
        assert!(!out.success());
        assert!(started.elapsed() < Duration::from_secs(20));
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    /// Credentials are left to the user's helpers: our environment must not stop git from
    /// asking the configured helper.
    #[test]
    fn configured_credential_helper_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let out = GitCommand::new(dir.path())
            .args([
                "-c",
                "credential.helper=",
                "-c",
                "credential.helper=!f() { echo username=helper-user; echo password=helper-pass; }; f",
                "credential",
                "fill",
            ])
            .stdin("protocol=https\nhost=example.invalid\n\n")
            .run()
            .unwrap();
        let text = out.stdout_str();
        assert!(text.contains("username=helper-user"), "{text}");
        assert!(text.contains("password=helper-pass"), "{text}");
    }

    #[test]
    fn auth_failures_get_a_hint() {
        assert!(
            auth_failure_hint(
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
            )
            .unwrap()
            .contains("credential helper")
        );
        assert!(
            auth_failure_hint("git@github.com: Permission denied (publickey).")
                .unwrap()
                .contains("SSH")
        );
        assert_eq!(auth_failure_hint("fatal: not a git repository"), None);
        // Transport failures that are not about credentials.
        for stderr in [
            "fatal: '/tmp/x' does not appear to be a git repository\nfatal: Could not read from \
             remote repository.",
            "ssh: connect to host example.com port 22: Connection refused\nfatal: Could not \
             read from remote repository.",
        ] {
            assert_eq!(auth_failure_hint(stderr), None, "{stderr}");
        }
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;

    /// A token in a URL must not reach the command display, stderr or error text.
    #[test]
    fn url_credentials_never_reach_errors() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = GitCommand::new(dir.path())
            .args(["ls-remote", "https://user:supersecret@127.0.0.1:9/x.git"])
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "");
        assert!(!cmd.display().contains("supersecret"));
        let err = cmd.run().unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(!text.contains("supersecret"), "{text}");
    }

    /// A remote URL using the `ext::` transport would run a shell command. It must be refused.
    #[test]
    fn ext_transport_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("pwned");
        let url = format!("ext::sh -c touch% {}", marker.display());
        let out = GitCommand::new(dir.path())
            .args(["-c", "protocol.ext.allow=always"]) // even if the caller (or repo) allows it
            .args(["ls-remote", "--", &url])
            .output()
            .unwrap();
        assert!(!out.success());
        assert!(!marker.exists(), "ext:: transport executed a command");
    }
}
