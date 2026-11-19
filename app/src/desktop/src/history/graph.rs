//! The branch graph of a repository: every commit reachable from the chosen refs, in
//! `git log --date-order` order, with lanes and edges from [`super::layout`].
//!
//! A [`Graph`] is built once and kept by the caller. The frontend asks for rows in chunks
//! ([`Graph::rows`]); commit text is decoded only for the rows asked for, so a 500k-commit history
//! costs one walk of the commit graph, not one decode per commit.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};

use gix::ObjectId;
use gix::bstr::ByteSlice;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::layout::{Layout, layout};
use crate::git::GitError;
use crate::git::state::open;
use crate::ipc::AppError;
use crate::proj::{Workflow, WorkflowMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    /// A detached HEAD (shown only when HEAD is not on a branch).
    Head,
    Branch,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RefLabel {
    /// Short name, e.g. `main`, `origin/main`, `v1.0`.
    pub name: String,
    /// Full name, e.g. `refs/heads/main`; `HEAD` for a detached HEAD.
    pub full: String,
    pub kind: RefKind,
    /// The branch HEAD points to, or the detached HEAD itself.
    pub is_head: bool,
}

/// Which commits the graph shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GraphOptions {
    /// Full ref names (or `HEAD`) whose history to show; `None` shows every branch, remote
    /// branch and tag, plus HEAD.
    pub refs: Option<Vec<String>>,
    /// Full ref name whose history stays bright; other commits are dimmed.
    pub highlight: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GraphInfo {
    /// Changes whenever the graph is rebuilt; row requests name it so stale rows are refused.
    #[ts(type = "number")]
    pub generation: u64,
    pub rows: u32,
    pub max_lanes: u16,
    /// Row of the commit HEAD points to, if it is in the graph.
    pub head_row: Option<u32>,
    /// Every ref the graph could show, for the branch filter.
    pub refs: Vec<RefLabel>,
    pub options: GraphOptions,
    /// The `.proj` file's private and public branches that exist here (full names, private
    /// first), for the "private and public only" filter. Empty when there are none.
    pub workflow_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub from: u16,
    pub to: u16,
    pub color: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    pub row: u32,
    pub id: String,
    pub lane: u16,
    pub color: u16,
    /// Segments from this row's centre down to the next row's centre.
    pub edges: Vec<GraphEdge>,
    pub refs: Vec<RefLabel>,
    pub parents: Vec<String>,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    /// Author date, seconds since the Unix epoch.
    #[ts(type = "number")]
    pub author_time: i64,
    /// Not reachable from the highlighted ref.
    pub dim: bool,
}

/// What the graph was built from; when it is unchanged, the graph is still current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphKey {
    pub options: GraphOptions,
    pub head: Option<ObjectId>,
    pub refs: Vec<(String, ObjectId)>,
}

pub struct Graph {
    pub generation: u64,
    pub key: GraphKey,
    path: PathBuf,
    ids: Vec<ObjectId>,
    index: HashMap<ObjectId, u32>,
    parent_start: Vec<u32>,
    parents: Vec<u32>,
    /// Committer time of each row.
    times: Vec<i64>,
    pub layout: Layout,
    labels: HashMap<u32, Vec<RefLabel>>,
    head_row: Option<u32>,
    dim: Option<Vec<bool>>,
    all_refs: Vec<RefLabel>,
}

/// The private branch and, in single-repo mode, the public branch, as full ref names, keeping
/// only those in `refs`. (In two-repo mode the public branch lives in the other repository.)
pub fn workflow_refs(workflow: &Workflow, refs: &[RefLabel]) -> Vec<String> {
    let mut sides = vec![&workflow.private];
    if workflow.mode == WorkflowMode::SingleRepo {
        sides.push(&workflow.public);
    }
    let mut out: Vec<String> = Vec::new();
    for side in sides {
        let full = format!("refs/heads/{}", side.branch);
        if refs.iter().any(|r| r.full == full) && !out.contains(&full) {
            out.push(full);
        }
    }
    out
}

fn read_err(path: &Path, e: impl std::fmt::Display) -> GitError {
    GitError::Read {
        path: path.to_path_buf(),
        message: crate::git::url::redact(&e.to_string()),
    }
}

/// A ref that points (after peeling) at a commit.
struct RefTarget {
    label: RefLabel,
    commit: ObjectId,
}

/// Branches, remote branches and tags that point at commits, plus a detached HEAD. Backups
/// (`refs/pm/`), the stash and notes are not history the user works on and are left out.
fn read_refs(repo: &gix::Repository, path: &Path) -> Result<Vec<RefTarget>, GitError> {
    let head_name = repo.head_name().map_err(|e| read_err(path, e))?;
    let mut out = Vec::new();
    if head_name.is_none()
        && let Ok(id) = repo.head_id()
    {
        out.push(RefTarget {
            label: RefLabel {
                name: "HEAD".into(),
                full: "HEAD".into(),
                kind: RefKind::Head,
                is_head: true,
            },
            commit: id.detach(),
        });
    }
    let refs = repo.references().map_err(|e| read_err(path, e))?;
    let groups = [
        (RefKind::Branch, refs.local_branches()),
        (RefKind::Remote, refs.remote_branches()),
        (RefKind::Tag, refs.tags()),
    ];
    for (kind, iter) in groups {
        for r in iter.map_err(|e| read_err(path, e))? {
            let Ok(mut r) = r else { continue };
            let full = r.name().as_bstr().to_str_lossy().into_owned();
            let name = r.name().shorten().to_str_lossy().into_owned();
            if kind == RefKind::Remote && name.ends_with("/HEAD") {
                continue;
            }
            let Ok(id) = r.peel_to_id() else { continue };
            let id = id.detach();
            let is_commit = repo
                .find_header(id)
                .is_ok_and(|h| h.kind() == gix::object::Kind::Commit);
            if !is_commit {
                continue;
            }
            let is_head = kind == RefKind::Branch
                && head_name
                    .as_ref()
                    .is_some_and(|h| h.as_bstr() == full.as_bytes());
            out.push(RefTarget {
                label: RefLabel {
                    name,
                    full,
                    kind,
                    is_head,
                },
                commit: id,
            });
        }
    }
    Ok(out)
}

/// The refs and HEAD the graph depends on, read without walking any history.
pub fn current_key(path: &Path, options: &GraphOptions) -> Result<GraphKey, GitError> {
    let repo = open(path)?;
    let refs = read_refs(&repo, path)?;
    Ok(GraphKey {
        options: options.clone(),
        head: repo.head_id().ok().map(|i| i.detach()),
        refs: refs.into_iter().map(|r| (r.label.full, r.commit)).collect(),
    })
}

fn unknown_ref(path: &Path, name: &str) -> GitError {
    GitError::Read {
        path: path.to_path_buf(),
        message: format!("there is no branch or tag named {name:?}"),
    }
}

/// Build the graph. Unknown ref names in `options` are an error.
pub fn build(path: &Path, options: &GraphOptions, generation: u64) -> Result<Graph, GitError> {
    let mut repo = open(path)?;
    repo.object_cache_size_if_unset(32 * 1024 * 1024);
    let refs = read_refs(&repo, path)?;
    let head = repo.head_id().ok().map(|i| i.detach());

    let find = |name: &str| -> Result<ObjectId, GitError> {
        if name == "HEAD" {
            return head.ok_or_else(|| unknown_ref(path, name));
        }
        refs.iter()
            .find(|r| r.label.full == name)
            .map(|r| r.commit)
            .ok_or_else(|| unknown_ref(path, name))
    };
    let tips: Vec<ObjectId> = match &options.refs {
        Some(names) => names.iter().map(|n| find(n)).collect::<Result<_, _>>()?,
        None => refs.iter().map(|r| r.commit).chain(head).collect(),
    };
    let highlight = options.highlight.as_deref().map(find).transpose()?;

    // One walk collects every commit with its parents and committer time, newest first. It uses
    // the commit-graph file when there is one, so most commits are never decoded here.
    let mut walk_ids = Vec::new();
    let mut walk_parents: Vec<gix::traverse::commit::ParentIds> = Vec::new();
    let mut walk_times = Vec::new();
    if !tips.is_empty() {
        let walk = repo
            .rev_walk(tips.iter().copied())
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
            ))
            .all()
            .map_err(|e| read_err(path, e))?;
        for info in walk {
            let info = info.map_err(|e| read_err(path, e))?;
            let time = info.commit_time();
            let info = info.detach();
            walk_ids.push(info.id);
            walk_parents.push(info.parent_ids);
            walk_times.push(time);
        }
    }
    let walk_index: HashMap<ObjectId, u32> = walk_ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i as u32))
        .collect();
    // Parents as walk indices; parents outside the walk (a shallow clone's boundary) are dropped.
    let wparents: Vec<Vec<u32>> = walk_parents
        .iter()
        .map(|ps| {
            let mut v: Vec<u32> = Vec::with_capacity(ps.len());
            for p in ps {
                if let Some(&i) = walk_index.get(p)
                    && !v.contains(&i)
                {
                    v.push(i);
                }
            }
            v
        })
        .collect();

    let order = date_order(&walk_times, &wparents);

    // Re-index by display row.
    let n = order.len();
    let mut row_of = vec![0u32; n];
    for (row, &w) in order.iter().enumerate() {
        row_of[w as usize] = row as u32;
    }
    let mut ids = Vec::with_capacity(n);
    let mut times = Vec::with_capacity(n);
    let mut parent_start = Vec::with_capacity(n + 1);
    let mut parents = Vec::new();
    parent_start.push(0);
    for &w in &order {
        let w = w as usize;
        ids.push(walk_ids[w]);
        times.push(walk_times[w]);
        parents.extend(wparents[w].iter().map(|&p| row_of[p as usize]));
        parent_start.push(parents.len() as u32);
    }
    let index: HashMap<ObjectId, u32> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i as u32))
        .collect();

    let lay = layout(n, |r| {
        parents[parent_start[r] as usize..parent_start[r + 1] as usize].to_vec()
    });

    let mut labels: HashMap<u32, Vec<RefLabel>> = HashMap::new();
    for r in &refs {
        if let Some(&row) = index.get(&r.commit) {
            labels.entry(row).or_default().push(r.label.clone());
        }
    }
    for v in labels.values_mut() {
        // HEAD's branch first, then branches, remotes, tags.
        v.sort_by_key(|l| (!l.is_head, l.kind as u8, l.name.clone()));
    }

    let mut graph = Graph {
        generation,
        key: GraphKey {
            options: options.clone(),
            head,
            refs: refs
                .iter()
                .map(|r| (r.label.full.clone(), r.commit))
                .collect(),
        },
        path: path.to_path_buf(),
        head_row: head.and_then(|h| index.get(&h).copied()),
        ids,
        index,
        parent_start,
        parents,
        times,
        layout: lay,
        labels,
        dim: None,
        all_refs: refs.into_iter().map(|r| r.label).collect(),
    };
    if let Some(tip) = highlight {
        graph.dim = Some(match graph.index.get(&tip) {
            Some(&row) => {
                let reach = graph.reachable(row);
                reach.into_iter().map(|r| !r).collect()
            }
            None => vec![true; graph.ids.len()],
        });
    }
    Ok(graph)
}

/// `git rev-list --date-order`: no parent before all its children, otherwise newest committer
/// time first; ties keep the order commits became ready, as git's priority queue does.
/// `times` and `parents` are indexed by walk position (newest first).
fn date_order(times: &[i64], parents: &[Vec<u32>]) -> Vec<u32> {
    let n = times.len();
    let mut children = vec![0u32; n];
    for ps in parents {
        for &p in ps {
            children[p as usize] += 1;
        }
    }
    let mut seq: u64 = 0;
    let mut heap: BinaryHeap<(i64, Reverse<u64>, u32)> = BinaryHeap::new();
    for (i, &c) in children.iter().enumerate() {
        if c == 0 {
            heap.push((times[i], Reverse(seq), i as u32));
            seq += 1;
        }
    }
    let mut out = Vec::with_capacity(n);
    while let Some((_, _, i)) = heap.pop() {
        out.push(i);
        for &p in &parents[i as usize] {
            let c = &mut children[p as usize];
            *c -= 1;
            if *c == 0 {
                heap.push((times[p as usize], Reverse(seq), p));
                seq += 1;
            }
        }
    }
    out
}

/// Decoded commit fields shared by graph rows, search and details.
pub struct CommitText {
    pub subject: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub author_time: i64,
    pub author_offset: i32,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_time: i64,
    pub committer_offset: i32,
}

pub fn commit_text(repo: &gix::Repository, id: ObjectId) -> Result<CommitText, String> {
    let commit = repo.find_commit(id).map_err(|e| e.to_string())?;
    let c = commit.decode().map_err(|e| e.to_string())?;
    let author = c.author().map_err(|e| e.to_string())?;
    let committer = c.committer().map_err(|e| e.to_string())?;
    let at = author.time().unwrap_or_default();
    let ct = committer.time().unwrap_or_default();
    Ok(CommitText {
        subject: c.message().summary().to_str_lossy().into_owned(),
        message: c.message.to_str_lossy().into_owned(),
        author_name: author.name.to_str_lossy().into_owned(),
        author_email: author.email.to_str_lossy().into_owned(),
        author_time: at.seconds,
        author_offset: at.offset,
        committer_name: committer.name.to_str_lossy().into_owned(),
        committer_email: committer.email.to_str_lossy().into_owned(),
        committer_time: ct.seconds,
        committer_offset: ct.offset,
    })
}

impl Graph {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Open the repository again. Cheap (config and refs are read lazily), and keeps [`Graph`]
    /// free of gix's single-threaded handles so it can be shared between commands.
    pub fn repo(&self) -> Result<gix::Repository, GitError> {
        let mut repo = open(&self.path)?;
        repo.object_cache_size_if_unset(4 * 1024 * 1024);
        Ok(repo)
    }

    pub fn id(&self, row: usize) -> ObjectId {
        self.ids[row]
    }

    pub fn row_of(&self, id: &ObjectId) -> Option<u32> {
        self.index.get(id).copied()
    }

    pub fn parents_of(&self, row: usize) -> &[u32] {
        &self.parents[self.parent_start[row] as usize..self.parent_start[row + 1] as usize]
    }

    /// Row of a commit given as a full id or an abbreviation of at least 4 hex digits; `None`
    /// when no commit (or more than one) in the graph matches.
    pub fn find(&self, commit: &str) -> Result<Option<u32>, AppError> {
        let hex = commit.trim().to_ascii_lowercase();
        if !(4..=40).contains(&hex.len()) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AppError::invalid(
                "a commit id is 4 to 40 hexadecimal digits",
            ));
        }
        if hex.len() == 40 {
            let id =
                ObjectId::from_hex(hex.as_bytes()).map_err(|e| AppError::invalid(e.to_string()))?;
            return Ok(self.row_of(&id));
        }
        let prefix =
            gix::hash::Prefix::from_hex(&hex).map_err(|e| AppError::invalid(e.to_string()))?;
        let mut found = None;
        for (row, id) in self.ids.iter().enumerate() {
            if prefix.cmp_oid(id) == std::cmp::Ordering::Equal {
                if found.is_some() {
                    return Ok(None);
                }
                found = Some(row as u32);
            }
        }
        Ok(found)
    }

    pub fn commit_time(&self, row: usize) -> i64 {
        self.times[row]
    }

    pub fn labels(&self, row: u32) -> &[RefLabel] {
        self.labels.get(&row).map(Vec::as_slice).unwrap_or_default()
    }

    /// Rows reachable from `row` (itself included).
    pub fn reachable(&self, row: u32) -> Vec<bool> {
        let mut seen = vec![false; self.ids.len()];
        let mut stack = vec![row];
        while let Some(r) = stack.pop() {
            if std::mem::replace(&mut seen[r as usize], true) {
                continue;
            }
            stack.extend(self.parents_of(r as usize).iter().copied());
        }
        seen
    }

    pub fn info(&self) -> GraphInfo {
        GraphInfo {
            generation: self.generation,
            rows: self.ids.len() as u32,
            max_lanes: self.layout.max_lanes,
            head_row: self.head_row,
            refs: self.all_refs.clone(),
            options: self.key.options.clone(),
            workflow_refs: Vec::new(),
        }
    }

    /// Rows `start..start + count` (clamped to the graph).
    pub fn rows(&self, start: u32, count: u32) -> Result<Vec<GraphRow>, GitError> {
        let repo = self.repo()?;
        let start = (start as usize).min(self.ids.len());
        let end = start.saturating_add(count as usize).min(self.ids.len());
        Ok((start..end)
            .map(|r| {
                let id = self.ids[r];
                let text = commit_text(&repo, id);
                let (subject, author_name, author_email, author_time) = match text {
                    Ok(t) => (t.subject, t.author_name, t.author_email, t.author_time),
                    Err(e) => (
                        format!("(could not read commit: {e})"),
                        String::new(),
                        String::new(),
                        0,
                    ),
                };
                GraphRow {
                    row: r as u32,
                    id: id.to_string(),
                    lane: self.layout.lane[r],
                    color: self.layout.color[r],
                    edges: self
                        .layout
                        .edges_of(r)
                        .iter()
                        .map(|e| GraphEdge {
                            from: e.from,
                            to: e.to,
                            color: e.color,
                        })
                        .collect(),
                    refs: self.labels(r as u32).to_vec(),
                    parents: self
                        .parents_of(r)
                        .iter()
                        .map(|&p| self.ids[p as usize].to_string())
                        .collect(),
                    subject,
                    author_name,
                    author_email,
                    author_time,
                    dim: self.dim.as_ref().is_some_and(|d| d[r]),
                }
            })
            .collect())
    }
}
