//! The `.proj` project file: TOML holding a project's identity, workflow mode, staging globs and
//! release settings.
//!
//! - [`ProjFile`] is the typed, validated view used by the rest of the app (read side).
//! - [`ProjDocument`] edits the file in place with `toml_edit`, so comments and layout survive
//!   writes (write side), and runs `schema_version` migrations.
//!
//! The full schema is documented in `docs/proj-format.md`.

mod document;
mod schema;
mod validate;

use std::fmt;
use std::path::{Path, PathBuf};

pub use document::{ProjDocument, migrate};
pub use schema::*;
pub(crate) use validate::check_ref_name;

/// Newest `schema_version` this build reads and writes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// File extension of project files (`<name>.proj`).
pub const PROJ_EXTENSION: &str = "proj";

/// One problem found while validating a `.proj` file, located by its dotted key path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Issue {
    /// Dotted key path, e.g. `workflow.public.path`.
    pub key: String,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.message)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjError {
    #[error("could not read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Not valid TOML, or a value has the wrong type or an unknown key.
    #[error("{}", format_parse(.line, .column, .message))]
    Parse {
        message: String,
        /// 1-based position of the problem, when known.
        line: Option<usize>,
        column: Option<usize>,
    },

    #[error(
        "schema_version {found} is newer than this app supports ({supported}); update Project Manager"
    )]
    UnsupportedSchema { found: u32, supported: u32 },

    #[error("invalid project file:\n{}", format_issues(.0))]
    Invalid(Vec<Issue>),

    #[error("no .proj file found in {0}")]
    NotFound(PathBuf),

    #[error("more than one .proj file in {dir}: {}", format_paths(.files))]
    Ambiguous { dir: PathBuf, files: Vec<PathBuf> },
}

fn format_parse(line: &Option<usize>, column: &Option<usize>, message: &str) -> String {
    match (line, column) {
        (Some(l), Some(c)) => format!("line {l}, column {c}: {message}"),
        _ => message.to_string(),
    }
}

fn format_issues(issues: &[Issue]) -> String {
    issues
        .iter()
        .map(|i| format!("  - {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

impl ProjError {
    pub(crate) fn parse_at(
        text: &str,
        message: String,
        span: Option<std::ops::Range<usize>>,
    ) -> Self {
        let (line, column) = match span {
            Some(span) => {
                let (l, c) = line_col(text, span.start);
                (Some(l), Some(c))
            }
            None => (None, None),
        };
        ProjError::Parse {
            message,
            line,
            column,
        }
    }
}

/// 1-based line and column (in characters) of a byte offset.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = before[line_start..].chars().count() + 1;
    (line, column)
}

impl ProjFile {
    /// Parse and validate `.proj` text. Older schema versions are migrated in memory first.
    pub fn parse(text: &str) -> Result<Self, ProjError> {
        let mut doc = ProjDocument::parse(text)?;
        migrate(&mut doc)?;
        let text = doc.to_string();
        let proj: ProjFile = toml::from_str(&text)
            .map_err(|e| ProjError::parse_at(&text, e.message().to_string(), e.span()))?;
        let issues = validate::validate(&proj);
        if issues.is_empty() {
            Ok(proj)
        } else {
            Err(ProjError::Invalid(issues))
        }
    }

    pub fn load(path: &Path) -> Result<Self, ProjError> {
        let text = std::fs::read_to_string(path).map_err(|source| ProjError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text)
    }
}

/// Find the single `*.proj` file directly inside `dir`.
pub fn find_proj_file(dir: &Path) -> Result<PathBuf, ProjError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ProjError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == PROJ_EXTENSION))
        .collect();
    found.sort();
    match found.len() {
        0 => Err(ProjError::NotFound(dir.to_path_buf())),
        1 => Ok(found.remove(0)),
        _ => Err(ProjError::Ambiguous {
            dir: dir.to_path_buf(),
            files: found,
        }),
    }
}

/// Write `contents` to `path` atomically (temporary file in the same folder, then rename).
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), ProjError> {
    let io = |source| ProjError::Io {
        path: path.to_path_buf(),
        source,
    };
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{file_name}.tmp-{}", std::process::id()));
    std::fs::write(&tmp, contents).map_err(io)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(e));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
