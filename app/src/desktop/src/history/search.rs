//! Log search over a [`Graph`]: message, author, path, date range and branch. Each filter has the
//! same meaning as a `git log` option, and the tests compare results with git:
//!
//! | filter    | git equivalent                                                    |
//! |-----------|-------------------------------------------------------------------|
//! | `message` | `-i -F --grep=<text>` (case-insensitive literal text)             |
//! | `author`  | `-i -F --author=<text>` (matched against `Name <email>`)          |
//! | `path`    | `--full-history -- ':(literal)<path>'` (file, or folder and below) |
//! | `since`   | `--since=<time>` (committer date at or after)                     |
//! | `until`   | `--until=<time>` (committer date at or before)                    |
//! | `branch`  | `<ref>` (reachable from that ref)                                 |
//!
//! Filters combine with AND. Text is matched literally (no regular expressions, so no pattern can
//! make a search slow). File history is a search with only `path` set; renames are not followed.

use std::collections::HashMap;

use gix::ObjectId;
use gix::bstr::{BStr, ByteSlice};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::graph::Graph;
use crate::ipc::{AppError, ErrorKind};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    pub message: Option<String>,
    pub author: Option<String>,
    /// Repository-relative path with `/` separators; a folder matches everything below it.
    pub path: Option<String>,
    /// Seconds since the Unix epoch.
    #[ts(type = "number | null")]
    pub since: Option<i64>,
    #[ts(type = "number | null")]
    pub until: Option<i64>,
    /// Full ref name (e.g. `refs/heads/main`) or `HEAD`.
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// Matching graph rows, top to bottom.
    pub rows: Vec<u32>,
}

/// Longest text a filter may hold; longer input is almost certainly a mistake.
const MAX_TEXT: usize = 1000;

fn clean_text(v: &Option<String>, what: &str) -> Result<Option<String>, AppError> {
    match v.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(t) if t.len() > MAX_TEXT => Err(AppError::invalid(format!(
            "the {what} filter is longer than {MAX_TEXT} characters"
        ))),
        Some(t) if t.contains(['\n', '\r', '\0']) => Err(AppError::invalid(format!(
            "the {what} filter must be a single line"
        ))),
        Some(t) => Ok(Some(t.to_lowercase())),
    }
}

/// Normalise a path filter: `\` to `/`, no leading `./` or `/`, no trailing `/`. Rejects empty,
/// `.` and `..` components rather than resolving them.
pub fn clean_path(path: &str) -> Result<Vec<String>, AppError> {
    let p = path.trim().replace('\\', "/");
    let p = p.trim_start_matches("./").trim_matches('/');
    if p.is_empty() {
        return Err(AppError::invalid("the path filter is empty"));
    }
    if p.len() > 4096 || p.contains('\0') {
        return Err(AppError::invalid("the path filter is not a valid path"));
    }
    let parts: Vec<String> = p.split('/').map(str::to_string).collect();
    if parts.iter().any(|c| c.is_empty() || c == "." || c == "..") {
        return Err(AppError::invalid(
            "the path filter must be a plain repository path (no `.`, `..` or `//`)",
        ));
    }
    Ok(parts)
}

/// Run a search. `cancelled` is polled regularly; when it returns true the search stops with a
/// `cancelled` error.
pub fn search(
    graph: &Graph,
    query: &LogQuery,
    cancelled: impl Fn() -> bool,
) -> Result<SearchResult, AppError> {
    let message = clean_text(&query.message, "message")?;
    let author = clean_text(&query.author, "author")?;
    let path = query.path.as_deref().map(clean_path).transpose()?;
    let reachable = match &query.branch {
        None => None,
        Some(name) => {
            let tip = if name == "HEAD" {
                graph.key.head
            } else {
                graph
                    .key
                    .refs
                    .iter()
                    .find(|(full, _)| full == name)
                    .map(|(_, id)| *id)
            };
            let tip = tip.ok_or_else(|| {
                AppError::not_found(format!("there is no branch or tag named {name:?}"))
            })?;
            Some(match graph.row_of(&tip) {
                Some(row) => graph.reachable(row),
                None => vec![false; graph.len()],
            })
        }
    };

    let repo = graph.repo()?;
    let mut paths = PathCache::new(graph, path);
    let mut rows = Vec::new();
    let read = |e: String| AppError::new(ErrorKind::Git, "could not read a commit").with_detail(e);

    for row in 0..graph.len() {
        if row % 256 == 0 && cancelled() {
            return Err(AppError::new(
                ErrorKind::Cancelled,
                "the search was cancelled",
            ));
        }
        let time = graph.commit_time(row);
        if query.since.is_some_and(|s| time < s) || query.until.is_some_and(|u| time > u) {
            continue;
        }
        if reachable.as_ref().is_some_and(|r| !r[row]) {
            continue;
        }
        if message.is_some() || author.is_some() {
            let commit = repo
                .find_commit(graph.id(row))
                .map_err(|e| read(e.to_string()))?;
            let c = commit.decode().map_err(|e| read(e.to_string()))?;
            if let Some(m) = &message
                && !contains_ci(c.message, m)
            {
                continue;
            }
            if let Some(a) = &author {
                let sig = c.author().map_err(|e| read(e.to_string()))?;
                let who = format!("{} <{}>", sig.name, sig.email);
                if !who.to_lowercase().contains(a.as_str()) {
                    continue;
                }
            }
        }
        if !paths.changed(&repo, row).map_err(read)? {
            continue;
        }
        rows.push(row as u32);
    }
    Ok(SearchResult { rows })
}

/// Case-insensitive literal match of `needle` (already lowercase) in `text`.
fn contains_ci(text: &BStr, needle: &str) -> bool {
    text.to_str_lossy().to_lowercase().contains(needle)
}

/// Answers "did this commit change the path?" with every commit's tree and tree entry looked up
/// once at most.
struct PathCache<'g> {
    graph: &'g Graph,
    path: Option<Vec<String>>,
    /// Root tree of each row, once read.
    trees: Vec<Option<ObjectId>>,
    /// Entry (id, mode) at the path, per root tree.
    entries: HashMap<ObjectId, Option<(ObjectId, u16)>>,
}

impl<'g> PathCache<'g> {
    fn new(graph: &'g Graph, path: Option<Vec<String>>) -> Self {
        PathCache {
            graph,
            trees: if path.is_some() {
                vec![None; graph.len()]
            } else {
                Vec::new()
            },
            path,
            entries: HashMap::new(),
        }
    }

    fn tree(&mut self, repo: &gix::Repository, row: usize) -> Result<ObjectId, String> {
        if let Some(t) = self.trees[row] {
            return Ok(t);
        }
        let commit = repo
            .find_commit(self.graph.id(row))
            .map_err(|e| e.to_string())?;
        let t = commit.tree_id().map_err(|e| e.to_string())?.detach();
        self.trees[row] = Some(t);
        Ok(t)
    }

    fn entry(
        &mut self,
        repo: &gix::Repository,
        tree: ObjectId,
    ) -> Result<Option<(ObjectId, u16)>, String> {
        if let Some(e) = self.entries.get(&tree) {
            return Ok(*e);
        }
        let path = self.path.as_ref().expect("only called with a path");
        let t = repo.find_tree(tree).map_err(|e| e.to_string())?;
        let e = t
            .lookup_entry(path.iter().map(|c| c.as_bytes()))
            .map_err(|e| e.to_string())?
            .map(|e| (e.object_id(), e.mode().value()));
        self.entries.insert(tree, e);
        Ok(e)
    }

    /// Whether `row` changed the path relative to at least one parent (or, for a root commit,
    /// whether the path exists). Always true without a path filter.
    fn changed(&mut self, repo: &gix::Repository, row: usize) -> Result<bool, String> {
        if self.path.is_none() {
            return Ok(true);
        }
        let tree = self.tree(repo, row)?;
        let mine = self.entry(repo, tree)?;
        let parents = self.graph.parents_of(row);
        if parents.is_empty() {
            return Ok(mine.is_some());
        }
        for &p in parents {
            let pt = self.tree(repo, p as usize)?;
            if pt == tree {
                continue;
            }
            if self.entry(repo, pt)? != mine {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
