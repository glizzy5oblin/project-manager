//! Ownership check before write commands. A repository owned by another user can make the git
//! CLI run programs through its config (`core.fsmonitor`, `core.sshCommand`, filters, hooks).
//! git 2.35.2 and newer refuse such repositories unless `safe.directory` allows them, but Project Manager
//! supports git from 2.30, so the same rule is applied here for every git version.

use std::path::{Path, PathBuf};

use super::{GitCommand, GitError};

/// Whether `repo` (a working tree or git dir) is owned by the current user, or allowed by a
/// `safe.directory` entry in the global or system git config.
pub fn is_trusted(repo: &Path) -> bool {
    let owned = |p: &Path| {
        matches!(
            gix::sec::Trust::from_path_ownership(p),
            Ok(gix::sec::Trust::Full)
        )
    };
    let git_dir = repo.join(".git");
    if owned(repo) && (!git_dir.exists() || owned(&git_dir)) {
        return true;
    }
    safe_directory_allows(&safe_directories(), repo)
}

/// Fail with [`GitError::Untrusted`] unless the repository is trusted or the user confirmed
/// (`allow_untrusted`) after seeing the warning.
pub fn ensure_trusted(repo: &Path, allow_untrusted: bool) -> Result<(), GitError> {
    if allow_untrusted || is_trusted(repo) {
        Ok(())
    } else {
        Err(GitError::Untrusted {
            path: repo.to_path_buf(),
        })
    }
}

/// `safe.directory` values from the global and system config only; git ignores the repository's
/// own config for this key, and so do we. Read from outside any repository.
fn safe_directories() -> Vec<String> {
    let mut out = Vec::new();
    for scope in ["--system", "--global"] {
        if let Ok(o) = GitCommand::new(std::env::temp_dir())
            .args(["config", scope, "--get-all", "safe.directory"])
            .output()
            && o.success()
        {
            out.extend(o.stdout_str().lines().map(str::to_string));
        }
    }
    out
}

/// Git's matching rules: `*` allows everything, an empty value clears earlier entries, a value
/// ending in `/*` allows everything below that folder, otherwise the path must match exactly.
pub(crate) fn safe_directory_allows(entries: &[String], repo: &Path) -> bool {
    let normal =
        |p: &Path| -> PathBuf { std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()) };
    let repo = normal(repo);
    let mut allowed = false;
    for e in entries {
        let e = e.trim();
        if e.is_empty() {
            allowed = false;
        } else if e == "*" {
            allowed = true;
        } else if let Some(prefix) = e.strip_suffix("/*") {
            if repo.starts_with(normal(Path::new(prefix))) {
                allowed = true;
            }
        } else {
            let e = e.trim_end_matches('/');
            if normal(Path::new(e)) == repo {
                allowed = true;
            }
        }
    }
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_repo_is_trusted() {
        let dir = tempfile::tempdir().unwrap();
        GitCommand::new(dir.path())
            .args(["init", "-q"])
            .run()
            .unwrap();
        assert!(is_trusted(dir.path()));
        assert!(ensure_trusted(dir.path(), false).is_ok());
    }

    /// The filesystem root is owned by root, so for a normal user it is untrusted unless the
    /// developer's own config says `safe.directory=*`.
    #[cfg(unix)]
    #[test]
    fn foreign_owned_path_is_refused() {
        let root = Path::new("/");
        let owned_by_us = matches!(
            gix::sec::Trust::from_path_ownership(root),
            Ok(gix::sec::Trust::Full)
        );
        if owned_by_us || safe_directory_allows(&safe_directories(), root) {
            eprintln!("skipped: running as root or safe.directory allows /");
            return;
        }
        assert!(!is_trusted(root));
        assert!(matches!(
            ensure_trusted(root, false),
            Err(GitError::Untrusted { .. })
        ));
        assert!(ensure_trusted(root, true).is_ok());
    }

    #[test]
    fn safe_directory_rules() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("r");
        std::fs::create_dir(&repo).unwrap();
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let r = repo.to_str().unwrap();
        let parent = format!("{}/*", dir.path().to_str().unwrap());
        assert!(!safe_directory_allows(&s(&[]), &repo));
        assert!(safe_directory_allows(&s(&["*"]), &repo));
        assert!(safe_directory_allows(&s(&[r]), &repo));
        assert!(safe_directory_allows(&s(&[&format!("{r}/")]), &repo));
        assert!(safe_directory_allows(&s(&[&parent]), &repo));
        assert!(!safe_directory_allows(&s(&["*", ""]), &repo));
        assert!(!safe_directory_allows(&s(&["/elsewhere"]), &repo));
    }
}
