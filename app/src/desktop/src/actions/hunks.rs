//! Staging and unstaging part of a file: the hunks the Changes view showed, named by their ids.
//!
//! The diff is computed again exactly as `work_diff` computed it for the view, and each hunk must
//! still be there with the same id, so what is staged is what the user saw. The patch is built
//! from the file's bytes (not the shown text), so line endings and text that is not UTF-8 are
//! kept, and applied to the index alone with `git apply --cached`. Unstaging applies the reverse
//! of the chosen hunks of the HEAD-to-index diff the same way. The working tree is never touched.
//!
//! Files git would not store as they are on disk are refused, with a hint to stage the whole
//! file instead: a `filter` (Git LFS and similar), `ident` or `working-tree-encoding` attribute,
//! and CRLF line endings that git would keep while the diff shows them as LF.

use std::collections::BTreeSet;

use super::{Plan, Repo, check_path, split_nul};
use crate::git::state::open;
use crate::history::diff::{FileStatus, Hunk, LineKind, WorkSide, work_compare};
use crate::ipc::{AppError, ErrorKind};

/// The context the Changes view asks `work_diff` for; hunk ids depend on it.
pub(crate) const WORK_CONTEXT: u32 = 3;

const LINK_MODE: &str = "120000";

fn whole_file(path: &str, why: &str) -> AppError {
    AppError::invalid(format!(
        "{path} {why}, so it cannot be staged or unstaged in parts: use the whole file instead"
    ))
}

pub(super) fn plan_hunks(
    repo: &Repo,
    path: &str,
    ids: &[String],
    side: WorkSide,
) -> Result<Plan, AppError> {
    check_path(path)?;
    let ids: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    if ids.is_empty() {
        return Err(AppError::invalid("no hunks were given"));
    }
    if repo
        .status()?
        .iter()
        .any(|e| e.path == path && e.conflicted())
    {
        return Err(whole_file(path, "has a conflict"));
    }
    let git = open(repo.root())?;
    let c = work_compare(&git, path, side, WORK_CONTEXT, true)?;
    let d = &c.diff;
    if d.binary || d.too_large {
        return Err(whole_file(path, "has no line diff"));
    }
    if d.status != FileStatus::Modified {
        return Err(whole_file(
            path,
            match d.status {
                FileStatus::Added => "is new",
                FileStatus::Deleted => "is deleted",
                _ => "changed type",
            },
        ));
    }
    if [&d.old_mode, &d.new_mode]
        .iter()
        .any(|m| m.as_deref() == Some(LINK_MODE))
    {
        return Err(whole_file(path, "is a symbolic link"));
    }
    if side == WorkSide::Unstaged {
        check_attributes(repo, path, c.crlf_as_lf)?;
    }

    let chosen: Vec<&Hunk> = d
        .hunks
        .iter()
        .filter(|h| ids.contains(h.id.as_str()))
        .collect();
    if chosen.len() != ids.len() {
        return Err(AppError::new(
            ErrorKind::Stale,
            format!("{path} changed since its diff was shown. Look at the new diff and try again."),
        ));
    }
    let patch = build_patch(&d.path, &c.old, &c.new, &chosen, side == WorkSide::Staged)?;
    let what = match chosen.len() {
        1 => "1 hunk".to_string(),
        n => format!("{n} hunks"),
    };
    let verb = match side {
        WorkSide::Unstaged => "Stage",
        WorkSide::Staged => "Unstage",
    };
    // --whitespace=nowarn: stage the lines exactly as they are, whatever apply.whitespace says.
    Ok(Plan::new(format!("{verb} {what} of {path}"))
        .impact("Only the staging area changes; the file on disk is kept")
        .step(
            repo.git()
                .args(["apply", "--cached", "--whitespace=nowarn", "-"])
                .stdin(patch),
        ))
}

/// Refuse a file whose staged form git would make differently from its bytes on disk.
fn check_attributes(repo: &Repo, path: &str, crlf_as_lf: bool) -> Result<(), AppError> {
    let out = repo
        .git()
        .args([
            "check-attr",
            "-z",
            "filter",
            "ident",
            "working-tree-encoding",
            "text",
            "eol",
            "--",
        ])
        .arg(path)
        .run()?;
    // -z output: path, attribute, value, repeated.
    let fields = split_nul(&out.stdout);
    let set = |name: &str| {
        fields
            .chunks(3)
            .find(|f| f.len() == 3 && f[1] == name)
            .is_some_and(|f| f[2] != "unspecified" && f[2] != "unset")
    };
    for (attr, why) in [
        ("filter", "goes through a git filter (such as Git LFS)"),
        ("ident", "has the ident attribute"),
        ("working-tree-encoding", "has a working-tree encoding"),
    ] {
        if set(attr) {
            return Err(whole_file(path, why));
        }
    }
    if crlf_as_lf {
        let autocrlf = repo
            .query(["config", "--get", "core.autocrlf"])?
            .stdout_str();
        let converts = matches!(autocrlf.trim(), "true" | "input") || set("text") || set("eol");
        if !converts {
            return Err(whole_file(
                path,
                "has CRLF line endings on disk but LF in the staging area",
            ));
        }
    }
    Ok(())
}

/// Lines with their endings, as the diff numbered them (it splits after each `\n`).
fn lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|&b| b == b'\n').collect()
}

/// A path in a patch header, always quoted the way git quotes, so any name parses.
fn quoted(prefix: &str, path: &str) -> Vec<u8> {
    let mut out = vec![b'"'];
    for &b in prefix.as_bytes().iter().chain(path.as_bytes()) {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\n' => out.extend_from_slice(b"\\n"),
            0x20..=0x7e => out.push(b),
            _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
        }
    }
    out.push(b'"');
    out
}

/// Line `n` (1-based) of a side; a hunk always names lines that exist, so a miss is a bug.
fn line<'a>(side: &[&'a [u8]], n: Option<u32>) -> Result<&'a [u8], AppError> {
    n.and_then(|n| side.get((n as usize).checked_sub(1)?))
        .copied()
        .ok_or_else(|| AppError::internal("a hunk names a line the file does not have"))
}

/// A unified patch of the chosen hunks of the `old` to `new` diff of `path`, to apply to the
/// index. With `reverse` it undoes them: the index then holds `new`, and the patch leads back to
/// `old`. Either way the base side is the index as it is, so each hunk keeps its base position
/// and the other side's start shifts by what the earlier chosen hunks added or removed.
pub(crate) fn build_patch(
    path: &str,
    old: &[u8],
    new: &[u8],
    chosen: &[&Hunk],
    reverse: bool,
) -> Result<Vec<u8>, AppError> {
    let (old_side, new_side) = (lines(old), lines(new));
    let (a, b) = (quoted("a/", path), quoted("b/", path));
    let mut out = Vec::new();
    for part in [
        &b"diff --git "[..],
        &a,
        b" ",
        &b,
        b"\n--- ",
        &a,
        b"\n+++ ",
        &b,
        b"\n",
    ] {
        out.extend_from_slice(part);
    }

    let mut shift: i64 = 0;
    for h in chosen {
        let (base_start, base_len, other_len) = if reverse {
            (h.new_start, h.new_lines, h.old_lines)
        } else {
            (h.old_start, h.old_lines, h.new_lines)
        };
        // Back to a 0-based position (an empty side names the line before), shift, and forward.
        let base0 = i64::from(base_start) - i64::from(base_len > 0);
        let other_start = base0 + shift + i64::from(other_len > 0);
        shift += i64::from(other_len) - i64::from(base_len);
        out.extend_from_slice(
            format!("@@ -{base_start},{base_len} +{other_start},{other_len} @@\n").as_bytes(),
        );
        for l in &h.lines {
            let old_line = || line(&old_side, l.old_line);
            let new_line = || line(&new_side, l.new_line);
            let (sign, text) = match (l.kind, reverse) {
                (LineKind::Context, _) => (b' ', old_line()?),
                (LineKind::Deleted, false) => (b'-', old_line()?),
                (LineKind::Added, false) => (b'+', new_line()?),
                (LineKind::Added, true) => (b'-', new_line()?),
                (LineKind::Deleted, true) => (b'+', old_line()?),
            };
            out.push(sign);
            out.extend_from_slice(text);
            if !text.ends_with(b"\n") {
                out.extend_from_slice(b"\n\\ No newline at end of file\n");
            }
        }
    }
    Ok(out)
}
