//! The Files view: browse a project's folder and edit text files in it. Every path is relative
//! to the project root and checked so it cannot reach outside it (by `..` or by a symlink), and
//! `.git` is never listed or opened.

use std::path::{Path, PathBuf};

use serde::Serialize;
use ts_rs::TS;

use crate::actions::check_path;
use crate::ipc::AppError;

/// Largest file the editor opens.
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// A folder with more entries than this is cut short (and says so).
const MAX_ENTRIES: usize = 5000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// Relative to the project, with `/` between folders.
    pub path: String,
    pub dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FileList {
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FileText {
    pub path: String,
    /// The contents; None for a binary, very large or not-UTF-8 file.
    pub text: Option<String>,
    pub size: u64,
    /// Why there is no text: "binary", "large" or "encoding".
    pub reason: Option<String>,
}

/// The real location of `rel` under `root`, refusing anything that leaves it. The file need not
/// exist (a save may create it), but its folder must.
fn locate(root: &Path, rel: &str) -> Result<PathBuf, AppError> {
    if !rel.is_empty() {
        check_path(rel)?;
    }
    let real_root = root.canonicalize().map_err(|e| AppError::io(root, &e))?;
    let target = real_root.join(rel);
    let probe = if target.exists() {
        target.clone()
    } else {
        target.parent().unwrap_or(&real_root).to_path_buf()
    };
    let real = probe.canonicalize().map_err(|e| AppError::io(&probe, &e))?;
    if !real.starts_with(&real_root) {
        return Err(AppError::invalid(format!(
            "{rel:?} is outside the project folder"
        )));
    }
    Ok(target)
}

/// The entries of a folder (`""` is the project root): folders first, then files, by name.
pub fn list_dir(root: &Path, dir: &str) -> Result<FileList, AppError> {
    let path = locate(root, dir)?;
    let mut entries = Vec::new();
    let mut truncated = false;
    let read = std::fs::read_dir(&path).map_err(|e| AppError::io(&path, &e))?;
    for item in read {
        let item = item.map_err(|e| AppError::io(&path, &e))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if dir.is_empty() && name == ".git" {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        // A symlink to a folder is shown as a file: following it could leave the project.
        let is_dir = item.file_type().is_ok_and(|t| t.is_dir());
        entries.push(FileEntry {
            path: if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            },
            name,
            dir: is_dir,
        });
    }
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(FileList { entries, truncated })
}

pub fn read_text(root: &Path, rel: &str) -> Result<FileText, AppError> {
    let path = locate(root, rel)?;
    let meta = std::fs::metadata(&path).map_err(|e| AppError::io(&path, &e))?;
    if meta.is_dir() {
        return Err(AppError::invalid(format!("{rel} is a folder")));
    }
    let none = |reason: &str| FileText {
        path: rel.to_string(),
        text: None,
        size: meta.len(),
        reason: Some(reason.to_string()),
    };
    if meta.len() > MAX_BYTES {
        return Ok(none("large"));
    }
    let bytes = std::fs::read(&path).map_err(|e| AppError::io(&path, &e))?;
    if bytes.contains(&0) {
        return Ok(none("binary"));
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok(FileText {
            path: rel.to_string(),
            size: meta.len(),
            text: Some(text),
            reason: None,
        }),
        Err(_) => Ok(none("encoding")),
    }
}

/// Save `content` as the file. A file that is a folder, or in `.git`, is refused.
pub fn write_text(root: &Path, rel: &str, content: &str) -> Result<(), AppError> {
    if rel.is_empty() {
        return Err(AppError::invalid("no file was given"));
    }
    let path = locate(root, rel)?;
    if path.is_dir() {
        return Err(AppError::invalid(format!("{rel} is a folder")));
    }
    std::fs::write(&path, content).map_err(|e| AppError::io(&path, &e))
}

/// Make a new empty file, or a folder, at `rel`, with any folders on the way. It never replaces
/// anything that is there already (a broken symlink counts).
pub fn create(root: &Path, rel: &str, dir: bool) -> Result<(), AppError> {
    if rel.is_empty() {
        return Err(AppError::invalid("no name was given"));
    }
    check_path(rel)?;
    let real_root = root.canonicalize().map_err(|e| AppError::io(root, &e))?;
    let target = real_root.join(rel);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(AppError::invalid(format!("{rel} already exists")));
    }
    // The nearest folder that exists must be inside the project (not reached through a symlink).
    let mut near = target.clone();
    while !near.exists() {
        near = near.parent().unwrap_or(&real_root).to_path_buf();
    }
    let real = near.canonicalize().map_err(|e| AppError::io(&near, &e))?;
    if !real.starts_with(&real_root) || !real.is_dir() {
        return Err(AppError::invalid(format!(
            "{rel:?} is outside the project folder, or under a file"
        )));
    }
    if dir {
        return std::fs::create_dir_all(&target).map_err(|e| AppError::io(&target, &e));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, &e))?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map(|_| ())
        .map_err(|e| AppError::io(&target, &e))
}

#[cfg(test)]
mod tests;
