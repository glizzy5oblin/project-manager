//! The hub's "create new" and "clone" flows.

use std::path::{Path, PathBuf};

use crate::git::GitCommand;
use crate::git::url::validate_clone_url;
use crate::ipc::{AppError, ErrorKind, TaskCtx};
use crate::proj::WorkflowMode;

/// A project name doubles as a folder and file name, so it must be valid on every OS.
pub fn validate_project_name(name: &str) -> Result<(), AppError> {
    let bad = |why: &str| Err(AppError::invalid(format!("Project name {name:?} {why}")));
    if name.trim().is_empty() {
        return Err(AppError::invalid("Enter a project name"));
    }
    if name.chars().count() > 100 {
        return bad("is longer than 100 characters");
    }
    if name != name.trim() {
        return bad("starts or ends with a space");
    }
    if name.starts_with(['.', '-']) || name.ends_with('.') {
        return bad("cannot start with . or -, or end with .");
    }
    if name.chars().any(|c| {
        c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        return bad("contains a character not allowed in file names (< > : \" / \\ | ? *)");
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return bad("is reserved on Windows");
    }
    Ok(())
}

/// Folder name git would pick for a clone of `url`: the last path segment without `.git`.
pub fn name_from_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches(['/', '\\']);
    let last = url.rsplit(['/', '\\', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    validate_project_name(name).ok().map(|_| name.to_string())
}

/// Removes the listed folders on drop unless disarmed: undoes a half-finished create or clone.
struct Cleanup(Vec<PathBuf>);

impl Cleanup {
    fn disarm(mut self) {
        self.0.clear();
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

fn must_not_exist(p: &Path) -> Result<(), AppError> {
    if p.symlink_metadata().is_ok() {
        return Err(AppError::new(
            ErrorKind::AlreadyExists,
            format!("{} already exists", p.display()),
        ));
    }
    Ok(())
}

fn parent_dir(parent: &Path) -> Result<PathBuf, AppError> {
    let p = std::fs::canonicalize(parent).map_err(|e| AppError::io(parent, &e))?;
    if !p.is_dir() {
        return Err(AppError::invalid(format!(
            "{} is not a folder",
            parent.display()
        )));
    }
    // Git for Windows cannot use `\\?\` paths as a clone destination.
    Ok(super::dunce_simplified(p))
}

/// Create the project folder `<parent>/<name>` holding the `<name>.proj`, a private repository
/// beside it (the project folder IS the repo root in single-repo mode; in two-repo mode the
/// private repository lives in `private/`). In two-repo mode the public repository `public/`
/// is created beside it. Returns the project root (the folder holding the `.proj`).
/// Nothing is left behind on failure.
pub fn create_project(parent: &Path, name: &str, mode: WorkflowMode) -> Result<PathBuf, AppError> {
    validate_project_name(name)?;
    let parent = parent_dir(parent)?;
    let folder = parent.join(name);
    must_not_exist(&folder)?;
    // create_dir (not _all) fails if something appeared since the check above, so cleanup only
    // ever removes a folder made here.
    std::fs::create_dir(&folder).map_err(|e| AppError::io(&folder, &e))?;
    let cleanup = Cleanup(vec![folder.clone()]);

    match mode {
        WorkflowMode::SingleRepo => {
            GitCommand::new(&folder)
                .args(["init", "-q", "-b", "main"])
                .run()?;
            super::write_new_proj(&folder, name, mode, None, None)?;
        }
        WorkflowMode::TwoRepo => {
            let root = folder.join("private");
            let public = folder.join("public");
            for dir in std::iter::once(&root).chain(std::iter::once(&public)) {
                std::fs::create_dir(dir).map_err(|e| AppError::io(dir, &e))?;
                GitCommand::new(dir)
                    .args(["init", "-q", "-b", "main"])
                    .run()?;
            }
            super::write_new_proj(&folder, name, mode, Some("./private"), Some("./public"))?;
        }
    }
    cleanup.disarm();
    Ok(folder)
}

/// Clone `url` into `<parent>/<name>` (name defaults to the one in the URL), reporting progress
/// through the task. Returns the new project root (the folder holding the `.proj` file). A
/// failed or cancelled clone leaves no folder behind.
pub fn clone_project(
    url: &str,
    parent: &Path,
    name: Option<&str>,
    ctx: &TaskCtx,
) -> Result<PathBuf, AppError> {
    let url = url.trim();
    validate_clone_url(url)?;
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => {
            validate_project_name(n)?;
            n.to_string()
        }
        None => name_from_url(url).ok_or_else(|| {
            AppError::invalid("Could not tell the folder name from the URL; enter one")
        })?,
    };
    let parent = parent_dir(parent)?;
    let dest = parent.join(&name);
    must_not_exist(&dest)?;
    // Claim the folder first (git clones into an empty folder), so cleanup can only remove a
    // folder this call created.
    std::fs::create_dir(&dest).map_err(|e| AppError::io(&dest, &e))?;
    let cleanup = Cleanup(vec![dest.clone()]);
    ctx.message(format!("Cloning into {}", dest.display()));
    GitCommand::new(&parent)
        .args(["clone", "--progress", "--"])
        .arg(url)
        .arg(&dest)
        .run_with(&ctx.cancel, |p| ctx.progress(p))?;
    ctx.check()?;
    cleanup.disarm();
    Ok(dest)
}
