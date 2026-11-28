//! Environment for the programs Project Manager runs (git, gh, and whatever git runs in turn:
//! hooks, credential helpers, editors).
//!
//! An AppImage launches the app with its bundled libraries first in `LD_LIBRARY_PATH` and its own
//! folders in `PATH`, `PYTHONPATH`, `GTK_PATH` and others. The app needs them, but children
//! inherit them too, so a system git or node would load the AppImage's older libraries:
//! `libffi.so.8: version 'LIBFFI_CALL_PLAN_8.4' not found (required by node)` fails a pre-commit
//! hook, and every git call warns about `libpcre2-8.so.0`. [`apply`] takes the AppImage's entries
//! back out, so child programs run as if started from the user's shell.
//!
//! Outside an AppImage (no `APPDIR`) it changes nothing.

use std::ffi::{OsStr, OsString};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// What to do with one variable: set it to the cleaned value, or remove it.
type Override = (OsString, Option<OsString>);

/// Variables set by the AppImage runtime that hold no path, so nothing in them names `APPDIR`.
const APPIMAGE_ONLY: &[&str] = &["PYTHONDONTWRITEBYTECODE"];

/// Remove the AppImage's own entries from `cmd`'s environment. Call it on every `Command` that
/// runs a program from the user's system.
pub fn apply(cmd: &mut Command) -> &mut Command {
    for (key, value) in overrides() {
        match value {
            Some(v) => cmd.env(key, v),
            None => cmd.env_remove(key),
        };
    }
    cmd
}

/// The overrides for this process, worked out once: the environment does not change.
fn overrides() -> &'static [Override] {
    static CACHE: OnceLock<Vec<Override>> = OnceLock::new();
    CACHE.get_or_init(|| match std::env::var_os("APPDIR") {
        Some(appdir) => compute(&appdir, std::env::vars_os()),
        None => Vec::new(),
    })
}

#[cfg(not(unix))]
fn compute(_: &OsStr, _: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<Override> {
    Vec::new()
}

/// Overrides that take every `appdir` entry out of `vars`. In a `:`-separated value, only the
/// entries under `appdir` are dropped; a variable left with nothing is removed.
#[cfg(unix)]
fn compute(appdir: &OsStr, vars: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<Override> {
    let appdir = Path::new(appdir);
    // An empty or root APPDIR would match every path: treat it as "not an AppImage".
    if appdir.as_os_str().is_empty() || appdir == Path::new("/") || !appdir.is_absolute() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (key, value) in vars {
        if APPIMAGE_ONLY.iter().any(|k| key == OsStr::new(k)) {
            out.push((key, None));
            continue;
        }
        let entries: Vec<&[u8]> = value.as_bytes().split(|b| *b == b':').collect();
        let kept: Vec<&[u8]> = entries
            .iter()
            .copied()
            .filter(|e| !Path::new(OsStr::from_bytes(e)).starts_with(appdir))
            .collect();
        if kept.len() == entries.len() {
            continue;
        }
        let kept: Vec<&[u8]> = kept.into_iter().filter(|e| !e.is_empty()).collect();
        if kept.is_empty() {
            out.push((key, None));
        } else {
            out.push((key, Some(OsString::from_vec(kept.join(&b':')))));
        }
    }
    out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn run(vars: &[(&str, &str)]) -> Vec<(String, Option<String>)> {
        let vars = vars.iter().map(|(k, v)| (k.into(), v.into()));
        let mut out: Vec<_> = compute(OsStr::new("/tmp/.mount_ProjecAbc"), vars)
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn strips_appimage_entries_and_keeps_the_rest() {
        let got = run(&[
            (
                "LD_LIBRARY_PATH",
                "/tmp/.mount_ProjecAbc/usr/lib/:/tmp/.mount_ProjecAbc/lib64/:/opt/mine/lib",
            ),
            (
                "PATH",
                "/tmp/.mount_ProjecAbc/usr/bin/:/tmp/.mount_ProjecAbc/bin/:/usr/local/bin:/usr/bin",
            ),
            ("GTK_PATH", "/tmp/.mount_ProjecAbc//usr/lib/gtk-3.0"),
            ("PYTHONHOME", "/tmp/.mount_ProjecAbc/usr/"),
            ("PYTHONDONTWRITEBYTECODE", "1"),
            ("HOME", "/home/me"),
            ("SSH_AUTH_SOCK", "/run/user/1000/ssh"),
        ]);
        assert_eq!(
            got,
            vec![
                ("GTK_PATH".into(), None),
                ("LD_LIBRARY_PATH".into(), Some("/opt/mine/lib".into())),
                ("PATH".into(), Some("/usr/local/bin:/usr/bin".into())),
                ("PYTHONDONTWRITEBYTECODE".into(), None),
                ("PYTHONHOME".into(), None),
            ]
        );
    }

    #[test]
    fn a_trailing_colon_left_by_the_runtime_does_not_survive() {
        // AppRun writes `…/lib64/:$LD_LIBRARY_PATH`, which ends in ":" when it was unset.
        let got = run(&[("LD_LIBRARY_PATH", "/tmp/.mount_ProjecAbc/usr/lib/:")]);
        assert_eq!(got, vec![("LD_LIBRARY_PATH".into(), None)]);
    }

    #[test]
    fn a_sibling_folder_with_the_same_prefix_is_kept() {
        let got = run(&[(
            "PATH",
            "/tmp/.mount_ProjecAbcd/bin:/tmp/.mount_ProjecAbc/bin",
        )]);
        assert_eq!(
            got,
            vec![("PATH".into(), Some("/tmp/.mount_ProjecAbcd/bin".into()))]
        );
    }

    #[test]
    fn unsafe_appdir_values_change_nothing() {
        for appdir in ["", "/", "relative"] {
            let vars = vec![("PATH".into(), "/usr/bin".into())];
            assert!(compute(OsStr::new(appdir), vars).is_empty(), "{appdir:?}");
        }
    }
}
