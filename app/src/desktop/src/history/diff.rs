//! Commit details and file diffs.
//!
//! A commit is compared with one of its parents (the first by default, like `git show
//! --first-parent`), or with the empty tree for a root commit. Renames are detected as git does
//! by default (50% similarity). Line diffs use Myers, like git, with word-level emphasis inside
//! changed lines.
//!
//! Limits keep a huge commit or file from freezing the app: line stats are skipped for large
//! files and for commits with very many files; a file diff over [`LARGE_FILE`] is shown only on
//! request, and never over [`MAX_FILE`].

use std::time::Duration;

use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::object::tree::EntryKind;
use serde::{Deserialize, Serialize};
use similar::{Algorithm, ChangeTag, InlineChangeOptions, TextDiff};
use ts_rs::TS;

use super::graph::commit_text;
use super::search::clean_path;
use crate::ipc::{AppError, ErrorKind};

/// Files over this size are diffed only when the user asks.
pub const LARGE_FILE: u64 = 1024 * 1024;
/// Files over this size are never diffed in the app.
pub const MAX_FILE: u64 = 16 * 1024 * 1024;
/// Commits with more changed files than this list them without line stats.
const STATS_MAX_FILES: usize = 1000;
/// Changed files listed at most.
const MAX_FILES: usize = 10_000;
/// Bytes git looks at to decide whether a file is binary.
const BINARY_PROBE: usize = 8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    /// Same path, different kind (file, symlink, submodule).
    TypeChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    /// Path before a rename.
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// Lines added and removed; `None` for binary files and when stats were skipped.
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub binary: bool,
    /// Rename similarity in percent.
    pub similarity: Option<u8>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub name: String,
    pub email: String,
    /// Seconds since the Unix epoch.
    #[ts(type = "number")]
    pub time: i64,
    /// Offset from UTC in seconds, as recorded in the commit.
    pub offset: i32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetail {
    pub id: String,
    pub parents: Vec<String>,
    /// Index into `parents` of the side the file list compares with.
    pub compared_parent: Option<u32>,
    pub author: Person,
    pub committer: Person,
    pub subject: String,
    /// Full message, subject included.
    pub message: String,
    pub files: Vec<ChangedFile>,
    /// More files changed than are listed.
    pub files_truncated: bool,
    /// Line stats were skipped (too many files).
    pub stats_skipped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    Context,
    Added,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    /// The line without its line ending.
    pub text: String,
    /// Changed words, as `[start, end)` offsets in UTF-16 code units (JavaScript string indices).
    pub emphasis: Vec<[u32; 2]>,
    /// The file ends here without a newline.
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    /// Names this hunk to a stage or unstage action, which refuses it if the file has changed
    /// since (the hunk's position or any of its lines). A check value, not a secret.
    pub id: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    pub binary: bool,
    /// Over the size limit; `hunks` is empty. Ask again with `force` (up to the hard limit).
    pub too_large: bool,
    #[ts(type = "number | null")]
    pub old_size: Option<u64>,
    #[ts(type = "number | null")]
    pub new_size: Option<u64>,
    pub old_mode: Option<String>,
    pub new_mode: Option<String>,
    pub hunks: Vec<Hunk>,
}

fn git_err(what: &str, e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorKind::Git, format!("could not read {what}"))
        .with_detail(crate::git::url::redact(&e.to_string()))
}

/// A full 40-digit commit id from the frontend.
pub fn parse_commit(commit: &str) -> Result<ObjectId, AppError> {
    if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::invalid(
            "a commit id must be 40 hexadecimal digits",
        ));
    }
    ObjectId::from_hex(commit.to_ascii_lowercase().as_bytes())
        .map_err(|e| AppError::invalid(e.to_string()))
}

struct Sides<'r> {
    parents: Vec<ObjectId>,
    parent: Option<u32>,
    old_tree: gix::Tree<'r>,
    new_tree: gix::Tree<'r>,
}

/// The commit and the trees to compare. `parent` defaults to the first parent.
fn sides<'r>(
    repo: &'r gix::Repository,
    id: ObjectId,
    parent: Option<u32>,
) -> Result<Sides<'r>, AppError> {
    let commit = repo.find_commit(id).map_err(|e| {
        AppError::not_found(format!("commit {id} is not in this repository"))
            .with_detail(e.to_string())
    })?;
    let parents: Vec<ObjectId> = commit.parent_ids().map(|p| p.detach()).collect();
    let parent = match parent {
        Some(p) if p as usize >= parents.len() => {
            return Err(AppError::invalid(format!(
                "commit {id} has {} parent(s); there is no parent {p}",
                parents.len()
            )));
        }
        Some(p) => Some(p),
        None if parents.is_empty() => None,
        None => Some(0),
    };
    let new_tree = commit
        .tree()
        .map_err(|e| git_err("the commit's files", e))?;
    let old_tree = match parent {
        Some(p) => repo
            .find_commit(parents[p as usize])
            .and_then(|c| c.tree())
            .map_err(|e| git_err("the parent commit's files", e))?,
        None => repo.empty_tree(),
    };
    Ok(Sides {
        parents,
        parent,
        old_tree,
        new_tree,
    })
}

/// Whether two entry kinds are the same type of thing; a plain and an executable file are.
fn same_type(a: EntryKind, b: EntryKind) -> bool {
    let norm = |k| match k {
        EntryKind::BlobExecutable => EntryKind::Blob,
        k => k,
    };
    norm(a) == norm(b)
}

fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(BINARY_PROBE)].contains(&0)
}

fn mode_str(mode: gix::object::tree::EntryMode) -> String {
    format!("{:06o}", mode.value())
}

/// Content git would diff for an entry: blob bytes, or `Subproject commit <id>` for submodules.
fn entry_content(
    repo: &gix::Repository,
    kind: EntryKind,
    id: ObjectId,
) -> Result<Vec<u8>, AppError> {
    if kind == EntryKind::Commit {
        return Ok(format!("Subproject commit {id}\n").into_bytes());
    }
    let blob = repo.find_blob(id).map_err(|e| git_err("a file", e))?;
    Ok(blob.data.clone())
}

fn blob_size(repo: &gix::Repository, kind: EntryKind, id: ObjectId) -> Result<u64, AppError> {
    if kind == EntryKind::Commit {
        return Ok(0);
    }
    let h = repo.find_header(id).map_err(|e| git_err("a file", e))?;
    Ok(h.size())
}

fn line_diff<'a>(old: &'a str, new: &'a str) -> TextDiff<'a, 'a, str> {
    TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(Duration::from_secs(2))
        .diff_lines(old, new)
}

fn count_lines(old: &[u8], new: &[u8]) -> (u32, u32) {
    let (old, new) = (String::from_utf8_lossy(old), String::from_utf8_lossy(new));
    let diff = line_diff(&old, &new);
    let (mut add, mut del) = (0, 0);
    for op in diff.ops() {
        for c in diff.iter_changes(op) {
            match c.tag() {
                ChangeTag::Insert => add += 1,
                ChangeTag::Delete => del += 1,
                ChangeTag::Equal => {}
            }
        }
    }
    (add, del)
}

/// Commit metadata and changed files, compared with `parent` (default: the first parent).
pub fn commit_detail(
    repo: &gix::Repository,
    id: ObjectId,
    parent: Option<u32>,
) -> Result<CommitDetail, AppError> {
    let s = sides(repo, id, parent)?;
    let text = commit_text(repo, id).map_err(|e| git_err("the commit", e))?;
    let mut opts = gix::diff::Options::default();
    opts.track_path()
        .track_rewrites(Some(gix::diff::Rewrites::default()));
    let changes = repo
        .diff_tree_to_tree(&s.old_tree, &s.new_tree, opts)
        .map_err(|e| git_err("the commit's changes", e))?;

    use gix::object::tree::diff::ChangeDetached as C;
    // (file, old side, new side) — sides as (kind, id) for the stats pass.
    type Side = Option<(EntryKind, ObjectId)>;
    let mut files: Vec<(ChangedFile, Side, Side)> = Vec::new();
    let mut total = 0usize;
    for change in changes {
        let item = match change {
            C::Addition {
                location,
                entry_mode,
                id,
                ..
            } => {
                if entry_mode.is_tree() {
                    continue;
                }
                (
                    location.to_str_lossy().into_owned(),
                    None,
                    FileStatus::Added,
                    None,
                    Some((entry_mode.kind(), id)),
                    None,
                )
            }
            C::Deletion {
                location,
                entry_mode,
                id,
                ..
            } => {
                if entry_mode.is_tree() {
                    continue;
                }
                (
                    location.to_str_lossy().into_owned(),
                    None,
                    FileStatus::Deleted,
                    Some((entry_mode.kind(), id)),
                    None,
                    None,
                )
            }
            C::Modification {
                location,
                previous_entry_mode,
                previous_id,
                entry_mode,
                id,
            } => {
                if entry_mode.is_tree() && previous_entry_mode.is_tree() {
                    continue;
                }
                let status = if same_type(previous_entry_mode.kind(), entry_mode.kind()) {
                    FileStatus::Modified
                } else {
                    FileStatus::TypeChanged
                };
                (
                    location.to_str_lossy().into_owned(),
                    None,
                    status,
                    Some((previous_entry_mode.kind(), previous_id)),
                    Some((entry_mode.kind(), id)),
                    None,
                )
            }
            C::Rewrite {
                source_location,
                source_entry_mode,
                source_id,
                diff,
                entry_mode,
                id,
                location,
                copy,
                ..
            } => {
                if entry_mode.is_tree() {
                    continue;
                }
                let similarity = match &diff {
                    None => Some(100),
                    Some(d) => Some((d.similarity * 100.0).round().clamp(0.0, 100.0) as u8),
                };
                (
                    location.to_str_lossy().into_owned(),
                    Some(source_location.to_str_lossy().into_owned()),
                    if copy {
                        FileStatus::Added
                    } else {
                        FileStatus::Renamed
                    },
                    Some((source_entry_mode.kind(), source_id)),
                    Some((entry_mode.kind(), id)),
                    similarity,
                )
            }
        };
        total += 1;
        if files.len() >= MAX_FILES {
            continue;
        }
        let (path, old_path, status, old, new, similarity) = item;
        files.push((
            ChangedFile {
                path,
                old_path,
                status,
                additions: None,
                deletions: None,
                binary: false,
                similarity,
            },
            old,
            new,
        ));
    }
    files.sort_by(|a, b| a.0.path.cmp(&b.0.path));

    let stats_skipped = files.len() > STATS_MAX_FILES;
    if !stats_skipped {
        for (f, old, new) in &mut files {
            let read = |side: &Option<(EntryKind, ObjectId)>| -> Result<Option<Vec<u8>>, AppError> {
                match side {
                    None => Ok(Some(Vec::new())),
                    Some((kind, id)) => {
                        if blob_size(repo, *kind, *id)? > LARGE_FILE {
                            return Ok(None);
                        }
                        entry_content(repo, *kind, *id).map(Some)
                    }
                }
            };
            let (Some(a), Some(b)) = (read(old)?, read(new)?) else {
                continue;
            };
            if is_binary(&a) || is_binary(&b) {
                f.binary = true;
                continue;
            }
            let (add, del) = count_lines(&a, &b);
            f.additions = Some(add);
            f.deletions = Some(del);
        }
    }

    Ok(CommitDetail {
        id: id.to_string(),
        parents: s.parents.iter().map(ToString::to_string).collect(),
        compared_parent: s.parent,
        author: Person {
            name: text.author_name,
            email: text.author_email,
            time: text.author_time,
            offset: text.author_offset,
        },
        committer: Person {
            name: text.committer_name,
            email: text.committer_email,
            time: text.committer_time,
            offset: text.committer_offset,
        },
        subject: text.subject,
        message: text.message,
        files_truncated: total > files.len(),
        files: files.into_iter().map(|f| f.0).collect(),
        stats_skipped,
    })
}

/// One file's diff in a commit. `old_path` names the file before a rename; otherwise the same
/// path is read on both sides. `force` lifts the [`LARGE_FILE`] limit (not [`MAX_FILE`]).
pub fn file_diff(
    repo: &gix::Repository,
    id: ObjectId,
    parent: Option<u32>,
    path: &str,
    old_path: Option<&str>,
    context: u32,
    force: bool,
) -> Result<FileDiff, AppError> {
    let new_parts = clean_path(path)?;
    let old_parts = old_path.map(clean_path).transpose()?;
    let s = sides(repo, id, parent)?;
    let lookup = |tree: &gix::Tree<'_>, parts: &[String]| -> Result<_, AppError> {
        let e = tree
            .lookup_entry(parts.iter().map(|c| c.as_bytes()))
            .map_err(|e| git_err("the commit's files", e))?;
        Ok(e.filter(|e| !e.mode().is_tree())
            .map(|e| (e.mode(), e.object_id())))
    };
    let new = lookup(&s.new_tree, &new_parts)?;
    let old = lookup(&s.old_tree, old_parts.as_deref().unwrap_or(&new_parts))?;
    let status = match (&old, &new) {
        (None, None) => {
            return Err(AppError::not_found(format!(
                "{path} is not in this commit or its parent"
            )));
        }
        (None, Some(_)) => FileStatus::Added,
        (Some(_), None) => FileStatus::Deleted,
        (Some(_), Some(_)) if old_path.is_some_and(|o| o != path) => FileStatus::Renamed,
        (Some((om, _)), Some((nm, _))) if !same_type(om.kind(), nm.kind()) => {
            FileStatus::TypeChanged
        }
        _ => FileStatus::Modified,
    };
    let size = |side: &Option<(gix::object::tree::EntryMode, ObjectId)>| {
        side.map(|(m, id)| blob_size(repo, m.kind(), id))
            .transpose()
    };
    let (old_size, new_size) = (size(&old)?, size(&new)?);
    let mut out = FileDiff {
        path: path.to_string(),
        old_path: old_path.map(str::to_string),
        status,
        binary: false,
        too_large: false,
        old_size,
        new_size,
        old_mode: old.map(|(m, _)| mode_str(m)),
        new_mode: new.map(|(m, _)| mode_str(m)),
        hunks: Vec::new(),
    };
    let biggest = old_size.unwrap_or(0).max(new_size.unwrap_or(0));
    if biggest > MAX_FILE || (biggest > LARGE_FILE && !force) {
        out.too_large = true;
        return Ok(out);
    }
    let content = |side: &Option<(gix::object::tree::EntryMode, ObjectId)>| match side {
        None => Ok(Vec::new()),
        Some((m, id)) => entry_content(repo, m.kind(), *id),
    };
    let (a, b) = (content(&old)?, content(&new)?);
    if is_binary(&a) || is_binary(&b) {
        out.binary = true;
        return Ok(out);
    }
    out.hunks = hunks(
        &String::from_utf8_lossy(&a),
        &String::from_utf8_lossy(&b),
        context.min(1000) as usize,
    );
    Ok(out)
}

/// Takes spaces and tabs off the ends of each emphasised range and drops ranges that hold only
/// whitespace. A re-indented line would otherwise show a pale block over its indentation, which
/// reads as a change to the code and is not one.
fn trim_emphasis(text: &str, emphasis: &mut Vec<[u32; 2]>) {
    if emphasis.is_empty() {
        return;
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let blank = |u: u16| u == 0x20 || u == 0x09 || u == 0xA0;
    emphasis.retain_mut(|r| {
        let (mut start, mut end) = (r[0] as usize, (r[1] as usize).min(units.len()));
        while start < end && blank(units[start]) {
            start += 1;
        }
        while end > start && blank(units[end - 1]) {
            end -= 1;
        }
        *r = [start as u32, end as u32];
        start < end
    });
}

/// Unified-diff hunks with `context` lines around each change and word emphasis inside
/// replaced lines.
pub fn hunks(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let diff = line_diff(old, new);
    let mut inline = InlineChangeOptions::new();
    inline.semantic_cleanup(true);
    let mut out = Vec::new();
    for group in diff.grouped_ops(context) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old_range = first.old_range().start..last.old_range().end;
        let new_range = first.new_range().start..last.new_range().end;
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_inline_changes_with_options(op, inline) {
                let kind = match change.tag() {
                    ChangeTag::Equal => LineKind::Context,
                    ChangeTag::Insert => LineKind::Added,
                    ChangeTag::Delete => LineKind::Deleted,
                };
                let mut text = String::new();
                let mut emphasis: Vec<[u32; 2]> = Vec::new();
                let mut pos: u32 = 0;
                for (emph, part) in change.iter_strings_lossy() {
                    let len = part.encode_utf16().count() as u32;
                    if emph && kind != LineKind::Context && len > 0 {
                        match emphasis.last_mut() {
                            Some(last) if last[1] == pos => last[1] = pos + len,
                            _ => emphasis.push([pos, pos + len]),
                        }
                    }
                    text.push_str(&part);
                    pos += len;
                }
                // Drop the line ending; clip emphasis to what is left.
                let trimmed = text.trim_end_matches(['\n', '\r']).len();
                text.truncate(trimmed);
                let end = text.encode_utf16().count() as u32;
                emphasis.retain_mut(|r| {
                    r[1] = r[1].min(end);
                    r[0] < r[1]
                });
                trim_emphasis(&text, &mut emphasis);
                // Emphasis on a whole line says nothing; show it as a plain change.
                if emphasis.len() == 1 && emphasis[0] == [0, end] {
                    emphasis.clear();
                }
                lines.push(DiffLine {
                    kind,
                    old_line: change.old_index().map(|i| i as u32 + 1),
                    new_line: change.new_index().map(|i| i as u32 + 1),
                    text,
                    emphasis,
                    no_newline: change.missing_newline(),
                });
            }
        }
        let old_lines = old_range.len() as u32;
        let new_lines = new_range.len() as u32;
        // Like git: an empty side starts at the line before (0 for an empty file).
        let old_start = old_range.start as u32 + u32::from(old_lines > 0);
        let new_start = new_range.start as u32 + u32::from(new_lines > 0);
        out.push(Hunk {
            id: hunk_id([old_start, old_lines, new_start, new_lines], &lines),
            old_start,
            old_lines,
            new_start,
            new_lines,
            lines,
        });
    }
    out
}

/// FNV-1a over a hunk's header and lines: stable across runs and builds, unlike `DefaultHasher`.
fn hunk_id(header: [u32; 4], lines: &[DiffLine]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for n in header {
        eat(&n.to_le_bytes());
    }
    for l in lines {
        let kind = match l.kind {
            LineKind::Context => b' ',
            LineKind::Added => b'+',
            LineKind::Deleted => b'-',
        };
        eat(&[kind, u8::from(l.no_newline)]);
        eat(&(l.text.len() as u64).to_le_bytes());
        eat(l.text.as_bytes());
    }
    format!("{h:016x}")
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BlameHunk {
    /// First line in the blamed file (1-based).
    pub start: u32,
    pub lines: u32,
    pub commit: String,
    pub author_name: String,
    pub author_email: String,
    #[ts(type = "number")]
    pub author_time: i64,
    pub subject: String,
    /// Path in `commit` when the file had another name there.
    pub source_path: Option<String>,
    /// First line in `commit`'s version of the file (1-based).
    pub source_start: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Blame {
    pub path: String,
    pub commit: String,
    pub binary: bool,
    pub too_large: bool,
    /// The file's lines at `commit`, without line endings.
    pub lines: Vec<String>,
    pub hunks: Vec<BlameHunk>,
}

/// Who last changed each line of `path` as of `commit`. Follows renames.
pub fn blame(repo: &gix::Repository, id: ObjectId, path: &str) -> Result<Blame, AppError> {
    let parts = clean_path(path)?;
    let commit = repo.find_commit(id).map_err(|e| {
        AppError::not_found(format!("commit {id} is not in this repository"))
            .with_detail(e.to_string())
    })?;
    let tree = commit
        .tree()
        .map_err(|e| git_err("the commit's files", e))?;
    let entry = tree
        .lookup_entry(parts.iter().map(|c| c.as_bytes()))
        .map_err(|e| git_err("the commit's files", e))?
        .filter(|e| e.mode().is_blob())
        .ok_or_else(|| AppError::not_found(format!("{path} is not a file in this commit")))?;
    let clean = parts.join("/");
    let mut out = Blame {
        path: clean.clone(),
        commit: id.to_string(),
        binary: false,
        too_large: false,
        lines: Vec::new(),
        hunks: Vec::new(),
    };
    if blob_size(repo, EntryKind::Blob, entry.object_id())? > LARGE_FILE {
        out.too_large = true;
        return Ok(out);
    }
    let data = entry_content(repo, EntryKind::Blob, entry.object_id())?;
    if is_binary(&data) {
        out.binary = true;
        return Ok(out);
    }
    let options = gix::repository::blame_file::Options {
        rewrites: Some(gix::diff::Rewrites::default()),
        ..Default::default()
    };
    let outcome = repo
        .blame_file(clean.as_bytes().as_bstr(), id, options)
        .map_err(|e| git_err("the file's history", e))?;
    out.lines = outcome
        .blob
        .lines()
        .map(|l| l.to_str_lossy().trim_end_matches('\r').to_string())
        .collect();
    let mut texts = std::collections::HashMap::new();
    for e in &outcome.entries {
        let t = match texts.get(&e.commit_id) {
            Some(t) => t,
            None => {
                let t = commit_text(repo, e.commit_id).map_err(|err| git_err("a commit", err))?;
                texts.entry(e.commit_id).or_insert(t)
            }
        };
        out.hunks.push(BlameHunk {
            start: e.start_in_blamed_file + 1,
            lines: e.len.get(),
            commit: e.commit_id.to_string(),
            author_name: t.author_name.clone(),
            author_email: t.author_email.clone(),
            author_time: t.author_time,
            subject: t.subject.clone(),
            source_path: e
                .source_file_name
                .as_ref()
                .map(|n| n.to_str_lossy().into_owned())
                .filter(|n| *n != clean),
            source_start: e.start_in_source_file + 1,
        });
    }
    Ok(out)
}

/// Which uncommitted change of a file to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum WorkSide {
    /// HEAD to the index: what the next commit takes.
    Staged,
    /// The index to the working tree (untracked files compare with nothing). A conflicted file
    /// compares "our" side with the file as it is now, conflict markers included.
    Unstaged,
}

/// A side of a working-tree diff: mode, content and size (`None` when the file is not there).
/// Content over [`MAX_FILE`] is not read, so it is empty while the size is real.
type WorkEntry = Option<(gix::object::tree::EntryMode, Vec<u8>, u64)>;

fn index_entry(
    repo: &gix::Repository,
    index: &gix::index::File,
    path: &str,
) -> Result<WorkEntry, AppError> {
    use gix::index::entry::Stage;
    let p = path.as_bytes().as_bstr();
    let Some(e) = index
        .entry_by_path_and_stage(p, Stage::Unconflicted)
        .or_else(|| index.entry_by_path_and_stage(p, Stage::Ours))
    else {
        return Ok(None);
    };
    let Some(mode) = e.mode.to_tree_entry_mode() else {
        return Ok(None);
    };
    let size = blob_size(repo, mode.kind(), e.id)?;
    if size > MAX_FILE {
        return Ok(Some((mode, Vec::new(), size)));
    }
    Ok(Some((mode, entry_content(repo, mode.kind(), e.id)?, size)))
}

fn head_entry(repo: &gix::Repository, parts: &[String]) -> Result<WorkEntry, AppError> {
    let tree = match repo.head_tree() {
        Ok(t) => t,
        // An unborn branch has no commit yet, so nothing is in HEAD.
        Err(_) if repo.head_id().is_err() => return Ok(None),
        Err(e) => return Err(git_err("the current commit", e)),
    };
    let Some(e) = tree
        .lookup_entry(parts.iter().map(|c| c.as_bytes()))
        .map_err(|e| git_err("the current commit's files", e))?
        .filter(|e| !e.mode().is_tree())
    else {
        return Ok(None);
    };
    let (mode, id) = (e.mode(), e.object_id());
    let size = blob_size(repo, mode.kind(), id)?;
    if size > MAX_FILE {
        return Ok(Some((mode, Vec::new(), size)));
    }
    Ok(Some((mode, entry_content(repo, mode.kind(), id)?, size)))
}

/// The file as it is on disk. Refuses a path that goes through a symbolic link, so nothing
/// outside the working tree is read; a symbolic link itself shows its target, as git does.
fn worktree_entry(
    root: &std::path::Path,
    parts: &[String],
    index_mode: Option<gix::object::tree::EntryMode>,
) -> Result<WorkEntry, AppError> {
    use gix::object::tree::EntryKind as K;
    let io =
        |e: std::io::Error| AppError::new(ErrorKind::Io, format!("could not read the file: {e}"));
    let mut full = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        full.push(part);
        let meta = match std::fs::symlink_metadata(&full) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let last = i + 1 == parts.len();
        if meta.file_type().is_symlink() {
            if !last {
                return Err(AppError::invalid(format!(
                    "{} goes through a symbolic link",
                    parts.join("/")
                )));
            }
            let target = std::fs::read_link(&full).map_err(io)?;
            let text = target.to_string_lossy().replace('\\', "/").into_bytes();
            let size = text.len() as u64;
            return Ok(Some((K::Link.into(), text, size)));
        }
        if !last {
            if !meta.is_dir() {
                return Ok(None);
            }
            continue;
        }
        if meta.is_dir() {
            // A nested repository or submodule; there is no file content to show.
            return Ok(None);
        }
        let size = meta.len();
        #[cfg(unix)]
        let exec = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        };
        // Windows has no executable bit; git keeps the index's (core.fileMode is false there).
        #[cfg(not(unix))]
        let exec = index_mode.is_some_and(|m| m.kind() == K::BlobExecutable);
        #[cfg(unix)]
        let _ = index_mode;
        let kind = if exec { K::BlobExecutable } else { K::Blob };
        if size > MAX_FILE {
            return Ok(Some((kind.into(), Vec::new(), size)));
        }
        let data = std::fs::read(&full).map_err(io)?;
        return Ok(Some((kind.into(), data, size)));
    }
    Ok(None)
}

/// One file's uncommitted change: staged (HEAD to index) or unstaged (index to working tree).
/// Working-tree text with CRLF line endings is compared as LF when the other side has none, so
/// a file checked out with `core.autocrlf` does not show every line as changed.
pub fn work_diff(
    repo: &gix::Repository,
    path: &str,
    side: WorkSide,
    context: u32,
    force: bool,
) -> Result<FileDiff, AppError> {
    work_compare(repo, path, side, context, force).map(|c| c.diff)
}

/// Most files [`work_stats`] counts in one call; the rest show no count.
pub const WORK_STATS_MAX: usize = 300;

/// Lines added and removed in one uncommitted file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct WorkStat {
    pub path: String,
    pub side: WorkSide,
    /// `None` for a binary or very large file, or one that could not be read.
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
}

/// Line counts for the files of the Changes list, each compared as [`work_diff`] would.
pub fn work_stats(repo: &gix::Repository, files: &[(String, WorkSide)]) -> Vec<WorkStat> {
    files
        .iter()
        .enumerate()
        .map(|(i, (path, side))| {
            let counts = (i < WORK_STATS_MAX)
                .then(|| work_compare(repo, path, *side, 0, false).ok())
                .flatten()
                .filter(|c| !c.diff.binary && !c.diff.too_large)
                .map(|c| {
                    let mut add = 0;
                    let mut del = 0;
                    for l in c.diff.hunks.iter().flat_map(|h| &h.lines) {
                        match l.kind {
                            LineKind::Added => add += 1,
                            LineKind::Deleted => del += 1,
                            LineKind::Context => {}
                        }
                    }
                    (add, del)
                });
            WorkStat {
                path: path.clone(),
                side: *side,
                additions: counts.map(|c| c.0),
                deletions: counts.map(|c| c.1),
            }
        })
        .collect()
}

/// A [`work_diff`] with the bytes it compared, so part of it can be staged or unstaged.
pub(crate) struct WorkCompare {
    pub diff: FileDiff,
    /// Empty when the diff has no hunks because the file is binary or too large.
    pub old: Vec<u8>,
    pub new: Vec<u8>,
    /// The working tree's CRLF line endings were compared (and are in `new`) as LF.
    pub crlf_as_lf: bool,
}

pub(crate) fn work_compare(
    repo: &gix::Repository,
    path: &str,
    side: WorkSide,
    context: u32,
    force: bool,
) -> Result<WorkCompare, AppError> {
    crate::actions::check_path(path)?;
    let parts = clean_path(path)?;
    let clean = parts.join("/");
    let root = repo
        .workdir()
        .ok_or_else(|| AppError::invalid("a bare repository has no working tree"))?
        .to_path_buf();
    let index = repo
        .index_or_empty()
        .map_err(|e| git_err("the staging area", e))?;
    let (old, new) = match side {
        WorkSide::Staged => (
            head_entry(repo, &parts)?,
            index_entry(repo, &index, &clean)?,
        ),
        WorkSide::Unstaged => {
            let old = index_entry(repo, &index, &clean)?;
            let new = worktree_entry(&root, &parts, old.as_ref().map(|(m, _, _)| *m))?;
            (old, new)
        }
    };
    let status = match (&old, &new) {
        (None, None) => {
            return Err(AppError::not_found(format!(
                "{clean} has no {} changes",
                match side {
                    WorkSide::Staged => "staged",
                    WorkSide::Unstaged => "unstaged",
                }
            )));
        }
        (None, Some(_)) => FileStatus::Added,
        (Some(_), None) => FileStatus::Deleted,
        (Some((om, _, _)), Some((nm, _, _))) if !same_type(om.kind(), nm.kind()) => {
            FileStatus::TypeChanged
        }
        _ => FileStatus::Modified,
    };
    let mut out = FileDiff {
        path: clean,
        old_path: None,
        status,
        binary: false,
        too_large: false,
        old_size: old.as_ref().map(|(_, _, s)| *s),
        new_size: new.as_ref().map(|(_, _, s)| *s),
        old_mode: old.as_ref().map(|(m, _, _)| mode_str(*m)),
        new_mode: new.as_ref().map(|(m, _, _)| mode_str(*m)),
        hunks: Vec::new(),
    };
    let nothing = |diff| WorkCompare {
        diff,
        old: Vec::new(),
        new: Vec::new(),
        crlf_as_lf: false,
    };
    let biggest = out.old_size.unwrap_or(0).max(out.new_size.unwrap_or(0));
    if biggest > MAX_FILE || (biggest > LARGE_FILE && !force) {
        out.too_large = true;
        return Ok(nothing(out));
    }
    let (a, mut b) = (
        old.map(|(_, d, _)| d).unwrap_or_default(),
        new.map(|(_, d, _)| d).unwrap_or_default(),
    );
    if is_binary(&a) || is_binary(&b) {
        out.binary = true;
        return Ok(nothing(out));
    }
    let crlf_as_lf =
        side == WorkSide::Unstaged && !a.contains(&b'\r') && b.windows(2).any(|w| w == b"\r\n");
    if crlf_as_lf {
        b = b.replace(b"\r\n", b"\n");
    }
    out.hunks = hunks(
        &String::from_utf8_lossy(&a),
        &String::from_utf8_lossy(&b),
        context.min(1000) as usize,
    );
    Ok(WorkCompare {
        diff: out,
        old: a,
        new: b,
        crlf_as_lf,
    })
}
