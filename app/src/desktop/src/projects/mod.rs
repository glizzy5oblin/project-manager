//! The hub's project registry and what the hub shows for each project.
//!
//! The registry is a JSON file in the app data folder listing the projects the user added. A
//! project is a git repository with exactly one `.proj` file at its root. Removing a project
//! only forgets it; files are never deleted.

mod create;
mod store;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use create::{clone_project, create_project, name_from_url, validate_project_name};
pub use store::{ProjectView, Session, Settings, TabState, read_json, write_json};

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
    /// Repository root (where the `.proj` file is), canonical.
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

    /// Register the repository at `root`. With `create` set and no `.proj` file present, a
    /// starter `.proj` file for that workflow mode is written first. Adding a folder that is
    /// already registered returns the existing entry.
    pub fn add(
        &mut self,
        root: &Path,
        create: Option<WorkflowMode>,
        now: u64,
    ) -> Result<ProjectEntry, AppError> {
        let root = canonical_dir(root)?;
        if let Some(existing) = self.data.projects.iter().find(|p| p.path == root) {
            return Ok(existing.clone());
        }
        let proj = match inspect(&root) {
            Err(e) if e.kind == ErrorKind::NoProjFile && create.is_some() => {
                write_starter_proj(&root, create.unwrap_or(WorkflowMode::TwoRepo))?
            }
            other => other?,
        };
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

/// Check that `root` is a non-bare git repository root holding one valid `.proj` file.
pub fn inspect(root: &Path) -> Result<ProjFile, AppError> {
    match crate::git::state::open(root) {
        Ok(repo) if repo.workdir().is_none() => {
            return Err(AppError::invalid(format!(
                "{} is a bare repository; pick a folder with a working tree",
                root.display()
            )));
        }
        Ok(_) => {}
        Err(e) => {
            return Err(AppError::new(
                ErrorKind::NotARepo,
                format!(
                    "{} is not the top folder of a git repository",
                    root.display()
                ),
            )
            .with_detail(e.to_string()));
        }
    }
    let file = find_proj_file(root)?;
    Ok(ProjFile::load(&file)?)
}

/// Write `<folder>.proj` for an existing repository. Never overwrites a file.
fn write_starter_proj(root: &Path, mode: WorkflowMode) -> Result<ProjFile, AppError> {
    let folder = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".into());
    let name = if validate_project_name(&folder).is_ok() {
        folder
    } else {
        "project".to_string()
    };
    let public = format!("../{name}-public");
    write_new_proj(root, &name, mode, &public)
}

/// Write a starter `.proj` for `name` into `root`, with `public_path` as the public repo path in
/// two-repo mode.
pub(crate) fn write_new_proj(
    root: &Path,
    name: &str,
    mode: WorkflowMode,
    public_path: &str,
) -> Result<ProjFile, AppError> {
    let path = root.join(format!("{name}.{PROJ_EXTENSION}"));
    let mut doc = ProjDocument::parse(&ProjFile::template(name, mode))?;
    if mode == WorkflowMode::TwoRepo {
        doc.set_string(&["workflow", "public", "path"], public_path);
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
    match find_proj_file(&entry.path).and_then(|f| ProjFile::load(&f)) {
        Ok(p) => {
            s.name = p.project.name;
            s.version = Some(p.project.version);
            s.mode = Some(p.workflow.mode);
        }
        Err(e) => s.proj_error = Some(e.into()),
    }
    match read_state(&entry.path, true) {
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
            s.last_release = last_release(state.tags.iter().map(|t| t.name.as_str()));
        }
        Err(e) => s.repo_error = Some(e.into()),
    }
    s
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
