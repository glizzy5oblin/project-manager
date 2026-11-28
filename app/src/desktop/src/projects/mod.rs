//! The hub's project registry and what the hub shows for each project.
//!
//! The registry is a JSON file in the app data folder listing the projects the user added. A
//! project is rooted at the folder holding its `.proj` file. In single-repo mode the repository
//! is that folder. In two-repo mode the private repository lives in a `private/` child, and the
//! public repository (a `public/` child) in a sibling, with the `.proj` at the shared root.
//! Removing a project only forgets it; files are never deleted.

mod create;
mod store;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use create::{clone_project, create_project, name_from_url, validate_project_name};
pub use store::{ProjectView, RepoSide, Session, Settings, TabState, read_json, write_json};

use crate::git::state::{Operation, read_state};
use crate::ipc::{AppError, ErrorKind};
use crate::proj::{PROJ_EXTENSION, ProjDocument, ProjFile, WorkflowMode, find_proj_file};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEntry {
    pub id: String,
    /// `project.name` from the `.proj` file when it was last read.
    pub name: String,
    /// Project root (where the `.proj` file is), canonical.
    #[ts(type = "string")]
    pub path: PathBuf,
    /// Unix seconds.
    #[ts(type = "number")]
    pub added: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryFile {
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    projects: Vec<ProjectEntry>,
}

pub struct Registry {
    file: PathBuf,
    data: RegistryFile,
}

impl Registry {
    /// Load the registry, or start empty when the file is missing. A damaged file is kept aside
    /// (see [`read_json`]) rather than overwritten.
    pub fn load(file: PathBuf) -> Registry {
        let data = read_json(&file);
        Registry { file, data }
    }

    pub fn list(&self) -> &[ProjectEntry] {
        &self.data.projects
    }

    pub fn get(&self, id: &str) -> Result<&ProjectEntry, AppError> {
        self.data
            .projects
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| AppError::not_found(format!("No project with id {id:?}")))
    }

    /// Register the project at `root`. The project is rooted at the folder holding the
    /// `.proj` file — a `<name>/<name>.proj` layout whose repositories (`private/` and, in
    /// two-repo mode, `public/`) live beside it, or a legacy layout with the `.proj` inside the
    /// private repository. With `create` set and no `.proj` file present, a starter `.proj`
    /// file for that workflow mode is written first. Adding a folder that is already
    /// registered returns the existing entry.
    pub fn add(
        &mut self,
        root: &Path,
        create: Option<WorkflowMode>,
        now: u64,
    ) -> Result<ProjectEntry, AppError> {
        let root = project_root(canonical_dir(root)?);
        if let Some(existing) = self.data.projects.iter().find(|p| p.path == root) {
            return Ok(existing.clone());
        }
        let proj = match load_proj(&root) {
            Err(e) if e.kind == ErrorKind::NoProjFile && create.is_some() => {
                write_starter_proj(&root, create.unwrap_or(WorkflowMode::TwoRepo))?
            }
            other => other?,
        };
        ensure_repo(&crate::proj::private_root(&root, &proj)?)?;
        self.data.next_id += 1;
        let entry = ProjectEntry {
            id: format!("p{}", self.data.next_id),
            name: proj.project.name,
            path: root,
            added: now,
        };
        self.data.projects.push(entry.clone());
        self.save()?;
        Ok(entry)
    }

    pub fn remove(&mut self, id: &str) -> Result<(), AppError> {
        self.get(id)?;
        self.data.projects.retain(|p| p.id != id);
        self.save()
    }

    /// Move a project to `index` in the hub order.
    pub fn reorder(&mut self, id: &str, index: usize) -> Result<(), AppError> {
        let from = self
            .data
            .projects
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| AppError::not_found(format!("No project with id {id:?}")))?;
        let entry = self.data.projects.remove(from);
        let to = index.min(self.data.projects.len());
        self.data.projects.insert(to, entry);
        self.save()
    }

    /// Keep the stored name in step with the `.proj` file. Saves only when it changed.
    pub fn update_name(&mut self, id: &str, name: &str) -> Result<(), AppError> {
        if let Some(p) = self.data.projects.iter_mut().find(|p| p.id == id)
            && p.name != name
        {
            p.name = name.to_string();
            self.save()?;
        }
        Ok(())
    }

    fn save(&self) -> Result<(), AppError> {
        write_json(&self.file, &self.data)
    }
}

fn canonical_dir(path: &Path) -> Result<PathBuf, AppError> {
    let p = std::fs::canonicalize(path).map_err(|e| AppError::io(path, &e))?;
    if !p.is_dir() {
        return Err(AppError::invalid(format!(
            "{} is not a folder",
            path.display()
        )));
    }
    Ok(dunce_simplified(p))
}

/// `canonicalize` on Windows returns `\\?\C:\...`; keep the familiar form when it is plain.
pub(crate) fn dunce_simplified(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && rest.as_bytes().get(1) == Some(&b':')
            && rest.len() < 260
        {
            return PathBuf::from(rest.to_string());
        }
    }
    p
}

/// The folder the project is rooted at: the folder that holds the `.proj` file.
///
/// The project file lives at the project root (the `<name>/<name>.proj` layout), so the
/// selected folder is the answer when it has one. A folder that is not the root, but holds a
/// `private` repository the way [`create_project`] makes projects (the legacy
/// `<name>/private/<name>.proj` layout), stands for that `private` folder. Only a real
/// folder counts, not a symlink, so the project never ends up outside the folder picked.
///
/// Only a `.proj` file decides: a folder with one IS the project folder, and that is what
/// lets a project root (the parent folder holding `private/` and `public/`) stand for the
/// project while its repositories are children.
fn project_root(root: PathBuf) -> PathBuf {
    if find_proj_file(&root).is_ok() {
        return root;
    }
    let private = root.join("private");
    if real_dir(&private) && find_proj_file(&private).is_ok() {
        return private;
    }
    root
}

fn real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// Load the `.proj` file from `root` (the folder that holds it) and return the parsed,
/// validated project. No repository check: a two-repo project's root holds no repository.
pub fn load_proj(root: &Path) -> Result<ProjFile, AppError> {
    let file = find_proj_file(root)?;
    Ok(ProjFile::load(&file)?)
}

/// Check that `path` is a non-bare git repository root (has a working tree).
pub fn ensure_repo(path: &Path) -> Result<(), AppError> {
    match crate::git::state::open(path) {
        Ok(repo) if repo.workdir().is_none() => {
            return Err(AppError::invalid(format!(
                "{} is a bare repository; pick a folder with a working tree",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(e) => {
            return Err(AppError::new(
                ErrorKind::NotARepo,
                format!(
                    "{} is not the top folder of a git repository",
                    path.display()
                ),
            )
            .with_detail(e.to_string()));
        }
    }
    Ok(())
}

/// Write `<name>.proj` for an existing project folder. Never overwrites a file.
///
/// In the `<name>/private` + `<name>/public` layout the proj goes at the project folder
/// and both sides are named inside it. The legacy layout (the root is the private repository
/// itself, `<name>/private`) keeps the proj there and points the public side at the sibling
/// `public` repo when that folder exists.
fn write_starter_proj(root: &Path, mode: WorkflowMode) -> Result<ProjFile, AppError> {
    let file_name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    let name_of = |p: &Path| {
        let folder = file_name(p).unwrap_or_else(|| "project".into());
        if validate_project_name(&folder).is_ok() {
            folder
        } else {
            "project".to_string()
        }
    };
    if real_dir(&root.join("private")) {
        // The project folder: the `.proj` lives here, beside its repositories.
        let name = name_of(root);
        let (private_path, public_path) = match mode {
            WorkflowMode::TwoRepo => (
                Some("./private"),
                real_dir(&root.join("public")).then_some("./public"),
            ),
            WorkflowMode::SingleRepo => (None, None),
        };
        return write_new_proj(root, &name, mode, private_path, public_path);
    }
    // The legacy layout: the root is the private repository itself.
    let split = file_name(root).as_deref() == Some("private");
    let folder = if split {
        root.parent().and_then(file_name)
    } else {
        file_name(root)
    }
    .unwrap_or_else(|| "project".into());
    let name = if validate_project_name(&folder).is_ok() {
        folder
    } else {
        "project".to_string()
    };
    let (private_path, public_path) = match mode {
        WorkflowMode::TwoRepo => {
            let sibling = root.parent().map(|p| p.join("public"));
            let public = if split && sibling.as_deref().is_some_and(real_dir) {
                "../public".into()
            } else {
                format!("../{name}-public")
            };
            (Some("."), Some(public))
        }
        WorkflowMode::SingleRepo => (None, None),
    };
    write_new_proj(root, &name, mode, private_path, public_path.as_deref())
}

/// Write a starter `.proj` for `name` into `root` (the folder the project is rooted at).
/// In two-repo mode the public repo path is written either from the caller or (when the public
/// repo is the conventional beside-the-private sibling) as `./public`.
pub(crate) fn write_new_proj(
    root: &Path,
    name: &str,
    mode: WorkflowMode,
    private_path: Option<&str>,
    public_path: Option<&str>,
) -> Result<ProjFile, AppError> {
    let path = root.join(format!("{name}.{PROJ_EXTENSION}"));
    let mut doc = ProjDocument::parse(&ProjFile::template(name, mode))?;
    if let Some(p) = private_path {
        doc.set_string(&["workflow", "private", "path"], p);
    }
    if mode == WorkflowMode::TwoRepo
        && let Some(p) = public_path
    {
        doc.set_string(&["workflow", "public", "path"], p);
    }
    let proj = doc.to_proj()?;
    // create_new: refuse to replace a file that appeared meanwhile.
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| AppError::io(&path, &e))?;
    f.write_all(doc.to_string().as_bytes())
        .map_err(|e| AppError::io(&path, &e))?;
    Ok(proj)
}

/// One hub card. Built from the registry entry, the `.proj` file and the repository; problems
/// with either are reported in the card instead of failing the whole hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub id: String,
    pub name: String,
    #[ts(type = "string")]
    pub path: PathBuf,
    pub version: Option<String>,
    pub mode: Option<WorkflowMode>,
    /// Branch HEAD is on; `None` when detached or the repo could not be read.
    pub branch: Option<String>,
    pub detached: bool,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub in_progress: Option<Operation>,
    /// Highest `vX.Y.Z` (or `X.Y.Z`) tag.
    pub last_release: Option<String>,
    pub trusted: bool,
    /// The `.proj` file is missing or invalid.
    pub proj_error: Option<AppError>,
    /// The repository could not be read (moved, deleted, damaged).
    pub repo_error: Option<AppError>,
}

pub fn summarize(entry: &ProjectEntry) -> ProjectSummary {
    let mut s = ProjectSummary {
        id: entry.id.clone(),
        name: entry.name.clone(),
        path: entry.path.clone(),
        version: None,
        mode: None,
        branch: None,
        detached: false,
        upstream: None,
        ahead: None,
        behind: None,
        staged: 0,
        unstaged: 0,
        untracked: 0,
        conflicted: 0,
        in_progress: None,
        last_release: None,
        trusted: true,
        proj_error: None,
        repo_error: None,
    };
    if !entry.path.is_dir() {
        s.repo_error = Some(AppError::not_found(format!(
            "{} no longer exists. Remove the project, or move the folder back",
            entry.path.display()
        )));
        return s;
    }
    // The private repo is what the hub reads (branch, status, tags); the proj lives at the
    // project root. Fall back to the project root when the proj is unreadable.
    let loaded = find_proj_file(&entry.path).and_then(|f| ProjFile::load(&f));
    let private = match &loaded {
        Ok(proj) => {
            crate::proj::private_root(&entry.path, proj).unwrap_or_else(|_| entry.path.clone())
        }
        Err(_) => entry.path.clone(),
    };
    match loaded {
        Ok(p) => {
            let public_tag = public_release_tag(&entry.path, &p);
            s.name = p.project.name;
            s.version = Some(p.project.version);
            s.mode = Some(p.workflow.mode);
            // Releases live on the public side; the private tags are only the fallback.
            s.last_release = public_tag;
        }
        Err(e) => s.proj_error = Some(e.into()),
    }
    match read_state(&private, true) {
        Ok(state) => {
            s.trusted = state.trusted;
            s.branch = state.head.branch.clone();
            s.detached = state.head.detached;
            if let Some(b) = state.branches.iter().find(|b| b.is_head) {
                s.upstream = b.upstream.clone();
                s.ahead = b.ahead;
                s.behind = b.behind;
            }
            if let Some(st) = &state.status {
                let n = |len: usize| u32::try_from(len).unwrap_or(u32::MAX);
                s.staged = n(st.staged.len());
                s.unstaged = n(st.unstaged.len());
                s.untracked = n(st.untracked.len());
                s.conflicted = n(st.conflicted.len());
            }
            s.in_progress = state.in_progress;
            if s.last_release.is_none() {
                s.last_release = last_release(state.tags.iter().map(|t| t.name.as_str()));
            }
        }
        Err(e) => s.repo_error = Some(e.into()),
    }
    s
}

/// The latest release tag from the public side: in two-repo mode the highest release tag of
/// the separate public repository; in single-repo mode the highest one reachable from the
/// public branch (a release tag sits on the public branch). `None` when the public side
/// cannot be read, so the caller falls back to the private tags.
fn public_release_tag(root: &Path, proj: &ProjFile) -> Option<String> {
    let out = match proj.workflow.mode {
        WorkflowMode::TwoRepo => {
            let rel = proj.workflow.public.path.as_deref()?;
            crate::git::GitCommand::new(root.join(rel))
                .args(["tag", "--list"])
                .run()
                .ok()?
        }
        WorkflowMode::SingleRepo => crate::git::GitCommand::new(root)
            .args(["tag", "--merged", &proj.workflow.public.branch])
            .run()
            .ok()?,
    };
    last_release(out.stdout_str().lines())
}

/// The highest tag that is a release version (`v1.2.3` or `1.2.3`, pre-releases included).
pub fn last_release<'a>(tags: impl Iterator<Item = &'a str>) -> Option<String> {
    tags.filter_map(|t| {
        let v = t.strip_prefix('v').unwrap_or(t);
        semver::Version::parse(v).ok().map(|v| (v, t))
    })
    .max_by(|a, b| a.0.cmp(&b.0))
    .map(|(_, t)| t.to_string())
}

#[cfg(test)]
mod tests;
