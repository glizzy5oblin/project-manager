//! Small JSON files in the app data folder: the open tabs (session) and user settings.

use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::Registry;
use crate::ipc::AppError;

/// Largest settings or session file read; anything bigger is treated as damaged.
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;

/// Read a JSON file, or the default when it is missing. A file that cannot be parsed is renamed
/// to `<name>.damaged-<unix time>` so the user's data is kept for inspection, and the default is
/// used.
pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    let text = match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return T::default(),
        Ok(m) if m.len() > MAX_JSON_BYTES => Err(format!("{} bytes is too large", m.len())),
        _ => std::fs::read_to_string(path).map_err(|e| e.to_string()),
    };
    let parsed = text.and_then(|t| serde_json::from_str::<T>(&t).map_err(|e| e.to_string()));
    match parsed {
        Ok(v) => v,
        Err(e) => {
            let aside = path.with_extension(format!("damaged-{}", crate::backup::unix_now()));
            log::warn!(
                "{} could not be read ({e}); moved to {}",
                path.display(),
                aside.display()
            );
            let _ = std::fs::rename(path, &aside);
            T::default()
        }
    }
}

/// Write a JSON file atomically (temporary file in the same folder, then rename), creating the
/// folder if needed.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    let dir = path
        .parent()
        .ok_or_else(|| AppError::internal(format!("{} has no folder", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| AppError::io(dir, &e))?;
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| AppError::internal(format!("could not encode {}: {e}", path.display())))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| AppError::io(&tmp, &e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AppError::io(path, &e)
    })
}

/// The view shown inside a project tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum ProjectView {
    #[default]
    Overview,
    Graph,
    Log,
    /// Uncommitted work: stage, unstage, discard and commit.
    Changes,
    /// Browse the project's folder and edit text files.
    Files,
    Staging,
    Releases,
    /// The operation log (and later the backups to undo with).
    Activity,
    Settings,
}

/// Which repository of a two-repo project a tab is showing. In single-repo mode only the
/// private side exists (the public side is a branch, not a separate repository).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum RepoSide {
    #[default]
    Private,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TabState {
    pub project_id: String,
    #[serde(default)]
    pub view: ProjectView,
    #[serde(default)]
    pub side: RepoSide,
}

/// Open project tabs, in order, and which one is showing (`None` for the hub).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", default)]
pub struct Session {
    pub tabs: Vec<TabState>,
    pub active: Option<String>,
}

impl Session {
    /// Drop tabs for projects that are no longer registered and duplicate tabs, and point
    /// `active` at the hub when its tab is gone.
    pub fn sanitize(&mut self, registry: &Registry) {
        let mut seen = std::collections::HashSet::new();
        self.tabs
            .retain(|t| registry.get(&t.project_id).is_ok() && seen.insert(t.project_id.clone()));
        if let Some(a) = &self.active
            && !self.tabs.iter().any(|t| &t.project_id == a)
        {
            self.active = None;
        }
    }
}

/// User settings. Every field has a default so older files keep loading as fields are added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Theme id: `dark`, `light`, or the file name (without `.json`) of a user theme.
    pub theme: String,
    /// Take a backup before dangerous and auto-backup actions (restored by the undo flow).
    pub backups: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "dark".into(),
            backups: true,
        }
    }
}
