//! Copyable diagnostics: everything needed to report a problem, without secrets.
//!
//! `get_diagnostics` builds a bounded, redacted summary of the app: version, platform, project
//! count, the log file path and its tail. The log file only holds lines the app already redacted
//! (command lines and stderr run through the git runner, which redacts URL credentials); the
//! tail is redacted again here as a guarantee. The report never fails: a missing or unreadable
//! log file yields an empty tail rather than an error.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::git::url::redact;

/// Largest number of lines read into the report.
const MAX_TAIL_LINES: usize = 40;
/// Largest span of the log file read for the tail.
const MAX_TAIL_BYTES: u64 = 128 * 1024;

/// The diagnostics report. `text` is a ready-made plain-text block the UI can copy; the other
/// fields let the UI render it separately.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    /// Full plain-text block, ready to copy.
    pub text: String,
    /// The app version (the crate's own `Cargo.toml` version).
    pub version: String,
    /// e.g. `linux/x86_64`.
    pub platform: String,
    /// Project count in the registry.
    pub projects: usize,
    /// Absolute path of the log file (empty when the app data folder is missing).
    pub log_file: String,
    /// The last `MAX_TAIL_LINES` lines of the log file, redacted, oldest first.
    pub log_tail: Vec<String>,
}

/// Where the app's log file lives. The plugin writes it there in every build.
pub fn log_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join("project-manager.log")
}

/// The tail of the log file: the last 40 lines inside the last 128 KiB, redacted. Missing or
/// unreadable files yield an empty tail.
pub fn tail(log_path: &Path) -> Vec<String> {
    use std::io::{Read, Seek};
    let mut file = match std::fs::File::open(log_path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return Vec::new();
    }
    let read_from = len.saturating_sub(MAX_TAIL_BYTES);
    if file.seek(std::io::SeekFrom::Start(read_from)).is_err() {
        return Vec::new();
    }
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = buf.lines().collect();
    if read_from > 0 && !lines.is_empty() {
        // The window may start mid-line; the first line is a fragment, drop it.
        lines.remove(0);
    }
    let start = lines.len().saturating_sub(MAX_TAIL_LINES);
    lines[start..].iter().map(|l| redact(l)).collect()
}

/// Build the report. `projects` is the registry count.
pub fn build(data_dir: &Path, projects: usize) -> Diagnostics {
    let log_file = log_file_path(data_dir);
    let log_tail = tail(&log_file);
    let version = env!("CARGO_PKG_VERSION").to_string();
    let platform = format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH);
    let text = format!(
        "Project Manager v{version} ({platform})\nProjects: {projects}\nLog file: {}\n\nLog tail:\n{}",
        log_file.display(),
        if log_tail.is_empty() {
            "(empty)".to_string()
        } else {
            log_tail.join("\n")
        }
    );
    Diagnostics {
        text,
        version,
        platform,
        projects,
        log_file: log_file.display().to_string(),
        log_tail,
    }
}

/// Write the diagnostics report to `path` as a plain-text file. Never fails on an empty tail.
pub fn save(data_dir: &Path, path: &Path, projects: usize) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("no file path given".to_string());
    }
    let text = build(data_dir, projects).text;
    std::fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_log(dir: &Path, body: &str) -> PathBuf {
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("project-manager.log");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// A unique directory per test so the parallel cases never share one.
    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pm-diag-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_log_file_yields_an_empty_tail() {
        let dir = unique_dir("missing");
        let tail = tail(&log_file_path(&dir));
        assert!(tail.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tail_is_bounded_to_the_last_lines() {
        let dir = unique_dir("bounded");
        let body: String = (1..=80).map(|i| format!("line {i}\n")).collect();
        let path = write_log(&dir, &body);
        let tail = tail(&path);
        assert_eq!(tail.len(), MAX_TAIL_LINES);
        assert_eq!(tail.first().unwrap(), "line 41");
        assert_eq!(tail.last().unwrap(), "line 80");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_tail_redacts_url_credentials() {
        let dir = unique_dir("redact");
        let path = write_log(
            &dir,
            "info: cloned https://user:secret-token@github.com/org/repo.git\n",
        );
        let tail = tail(&path);
        assert_eq!(tail.len(), 1);
        assert!(tail[0].contains("***"));
        assert!(!tail[0].contains("secret-token"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_near_the_end_line_is_not_dropped_and_a_fragment_is() {
        let dir = unique_dir("fragment");
        // A line just over the read window, plus one plain line: the first must be dropped as
        // a fragment and the visible lines kept.
        let mut body = String::new();
        body.push_str(&"x".repeat(MAX_TAIL_BYTES as usize + 10));
        body.push('\n');
        body.push_str("visible line\n");
        let path = write_log(&dir, &body);
        let tail = tail(&path);
        // The first line fills the read window, so the fragment is dropped and only the
        // whole lines at the end stay.
        assert_eq!(tail, vec!["visible line".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_text_block_names_the_version_platform_and_projects() {
        let dir = unique_dir("text");
        let d = build(&dir, 7);
        assert!(d.text.contains("Project Manager v"));
        assert!(d.text.contains("("));
        assert!(d.text.contains("projects: 7") || d.text.contains("Projects: 7"));
        assert!(d.text.contains("Log file:"));
        assert_eq!(d.projects, 7);
        assert_eq!(d.version, env!("CARGO_PKG_VERSION"));
        assert!(d.platform.contains('/'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_writes_the_report_to_the_given_path() {
        let dir = unique_dir("save");
        let out = dir.join("report.txt");
        save(&dir, &out, 3).unwrap();
        let saved = std::fs::read_to_string(&out).unwrap();
        assert!(saved.contains("Projects: 3"));
        assert!(saved.contains("Log file:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_refuses_an_empty_path_and_unwritable_directories() {
        let dir = unique_dir("save-bad");
        assert!(save(&dir, Path::new(""), 0).is_err());
        // A path inside a missing directory refuses to write.
        let missing = dir.join("nope/report.txt");
        assert!(save(&dir, &missing, 0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
