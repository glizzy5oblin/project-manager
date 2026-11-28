//! App-level state export and restore: the files the app writes as its own state
//! (the project registry, the session, the settings and the operation log), as a
//! single zip one the user picks — so a bug report, a backup or a fresh install
//! can carry the app's state with it.
//!
//! The app data folder also holds WebView caches, the log file and release zips,
//! which are not state; only the four files here move. Export skips files that
//! don't exist yet (a fresh install exports an empty zip); restore refuses a zip
//! that is not one, that holds anything other than these files, or that holds a
//! path that escapes the archive.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::ipc::AppError;

/// The app-level state files, in the order they are written to the zip.
pub const STATE_FILES: &[&str] = &[
    "projects.json",
    "session.json",
    "settings.json",
    "oplog.jsonl",
];

/// Write the app-level state as a zip at `path` (a path the user picked in a dialog).
/// Returns the path written.
pub fn export_state(data_dir: &Path, path: &Path) -> Result<PathBuf, AppError> {
    if path.as_os_str().is_empty() {
        return Err(AppError::invalid("no file path given"));
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::invalid(format!(
            "{} is not a folder",
            parent.display()
        )));
    }
    let mut files: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for name in STATE_FILES {
        let p = data_dir.join(name);
        if p.is_file() {
            let bytes = std::fs::read(&p).map_err(|e| AppError::io(&p, &e))?;
            files.push((name, bytes));
        }
    }

    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(
        std::fs::File::create(path).map_err(|e| AppError::io(path, &e))?,
    ));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in &files {
        zip.start_file(name, options)
            .map_err(|e| AppError::internal(e.to_string()))?;
        zip.write_all(bytes).map_err(|e| AppError::io(path, &e))?;
    }
    zip.finish()
        .map_err(|e| AppError::internal(e.to_string()))?;
    Ok(path.to_path_buf())
}

/// Restore the app-level state from a zip written by `export_state`. Only the state
/// files are read back; anything else in the archive (or a path that escapes the
/// archive) is refused, so a stray zip cannot drop files into the app data folder.
/// Returns the files restored, in archive order.
pub fn restore_state(data_dir: &Path, path: &Path) -> Result<Vec<String>, AppError> {
    if path.as_os_str().is_empty() {
        return Err(AppError::invalid("no file path given"));
    }
    if !path.is_file() {
        return Err(AppError::not_found(format!(
            "{} is not a file",
            path.display()
        )));
    }
    // Read every entry before writing anything: a corrupt zip must not leave the
    // app data folder half-restored.
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut archive =
        zip::ZipArchive::new(std::fs::File::open(path).map_err(|e| AppError::io(path, &e))?)
            .map_err(|e| AppError::invalid(format!("{} is not a zip: {e}", path.display())))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::invalid(format!("{}: {e}", path.display())))?;
        let name = entry
            .name()
            .map_err(|e| {
                AppError::invalid(format!(
                    "{}: entry name is not valid UTF-8: {e}",
                    path.display()
                ))
            })?
            .into_owned();
        // A state file is a bare file name: no directories, no path traversal.
        if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(AppError::invalid(format!(
                "{}: unexpected entry {name:?}",
                path.display()
            )));
        }
        if !STATE_FILES.contains(&name.as_str()) {
            return Err(AppError::invalid(format!(
                "{}: unexpected entry {name:?}",
                path.display()
            )));
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|e| AppError::io(path, &e))?;
        entries.push((name, bytes));
    }
    if entries.is_empty() {
        return Err(AppError::invalid(format!(
            "{}: no app-level state files in the zip",
            path.display()
        )));
    }
    let mut restored = Vec::new();
    for (name, bytes) in &entries {
        let dest = data_dir.join(name);
        std::fs::write(&dest, bytes).map_err(|e| AppError::io(&dest, &e))?;
        restored.push(name.clone());
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique directory per test so the parallel cases never share one.
    fn unique_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pm-appdata-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_state(dir: &Path, files: &[(&str, &str)]) {
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
    }

    #[test]
    fn export_writes_only_the_state_files() {
        let dir = unique_dir("export");
        write_state(&dir, &[("projects.json", "[]"), ("settings.json", "{}")]);
        // A log and a WebView cache file: not state, must not be in the zip.
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::write(dir.join("logs").join("project-manager.log"), "log line").unwrap();
        std::fs::write(dir.join("storage"), "cache").unwrap();

        let zip_path = dir.join("data.zip");
        export_state(&dir, &zip_path).unwrap();

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        assert!(names.contains(&"projects.json".to_string()), "{names:?}");
        assert!(names.contains(&"settings.json".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.contains('/')), "{names:?}");
        assert!(names.len() == 2, "{names:?}");
    }

    #[test]
    fn export_skips_missing_state_files() {
        let dir = unique_dir("missing");
        write_state(&dir, &[("settings.json", "{}")]);

        let zip_path = dir.join("data.zip");
        export_state(&dir, &zip_path).unwrap();

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["settings.json".to_string()]);
    }

    #[test]
    fn export_rejects_an_empty_path() {
        let dir = unique_dir("empty");
        write_state(&dir, &[("settings.json", "{}")]);
        let err = export_state(&dir, Path::new("")).unwrap_err();
        assert!(err.message.contains("no file path"), "{err:?}");
    }

    #[test]
    fn restore_round_trips_the_state_files() {
        let dir = unique_dir("roundtrip");
        write_state(
            &dir,
            &[
                ("projects.json", "[1]"),
                ("session.json", "{}"),
                ("settings.json", "{\"theme\":\"dark\"}"),
                ("oplog.jsonl", "line1\nline2\n"),
            ],
        );
        let zip_path = dir.join("data.zip");
        export_state(&dir, &zip_path).unwrap();

        // Restore into a fresh folder: every state file comes back byte-identical.
        let fresh = unique_dir("roundtrip-fresh");
        let restored = restore_state(&fresh, &zip_path).unwrap();
        assert_eq!(restored.len(), 4, "{restored:?}");
        for name in STATE_FILES {
            let from_fresh = std::fs::read_to_string(fresh.join(name)).unwrap();
            let from_export = std::fs::read_to_string(dir.join(name)).unwrap();
            assert_eq!(from_fresh, from_export, "{name}");
        }
    }

    #[test]
    fn restore_refuses_a_non_state_zip() {
        let dir = unique_dir("stray");
        // A zip with a foreign file must be refused, not imported.
        let zip_path = dir.join("foreign.zip");
        let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(
            std::fs::File::create(&zip_path).unwrap(),
        ));
        zip.start_file("README.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"# readme\n").unwrap();
        zip.finish().unwrap();

        let err = restore_state(&dir, &zip_path).unwrap_err();
        assert!(err.message.contains("unexpected entry"), "{err:?}");
    }

    #[test]
    fn restore_refuses_a_zip_missing_the_state() {
        let dir = unique_dir("emptyzip");
        let zip_path = dir.join("empty.zip");
        let zip = zip::ZipWriter::new(std::io::BufWriter::new(
            std::fs::File::create(&zip_path).unwrap(),
        ));
        zip.finish().unwrap();
        let err = restore_state(&dir, &zip_path).unwrap_err();
        assert!(err.message.contains("no app-level state files"), "{err:?}");
    }

    #[test]
    fn restore_refuses_a_missing_path() {
        let dir = unique_dir("missing-path");
        let err = restore_state(&dir, &dir.join("nope.zip")).unwrap_err();
        assert!(err.message.contains("is not a file"), "{err:?}");
    }

    #[test]
    fn restore_refuses_a_non_utf8_entry_name() {
        let dir = unique_dir("nonutf8");
        // Build a zip whose entry name is not valid UTF-8.
        let zip_path = dir.join("binary.zip");
        let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(
            std::fs::File::create(&zip_path).unwrap(),
        ));
        zip.start_file_with_options(zip::write::SimpleFileOptions::default().name(b"\xff\xfe"))
            .unwrap();
        std::io::Write::write_all(&mut zip, b"data").unwrap();
        zip.finish().unwrap();

        // The zip reader cannot decode a non-UTF-8 entry name as-is; either it surfaces a
        // name-level error or the decoded name simply misses the state allowlist. Either way
        // restore fails as a typed error instead of panicking.
        let err = restore_state(&dir, &zip_path).unwrap_err();
        assert!(
            err.message.contains("UTF-8") || err.message.contains("unexpected entry"),
            "{err:?}"
        );
    }
}
