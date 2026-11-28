//! The Tauri commands the UI calls, and the events it listens to. Rules are in [`crate::ipc`]:
//! commands name projects by registry id, and only the add, create and clone flows take a path
//! (one the user picked in a dialog).
//!
//! Events:
//! - `task` ([`TaskEvent`]): background task started, progressed, finished.
//! - `repo-changed` ([`RepoChanged`]): files changed in watched projects; re-read their state.
//! - `themes-changed` ([`ThemeList`]): a file in the themes folder changed.
//! - `op-log` ([`OpEntry`]): a write action ran a git command or took a backup.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use crate::actions::{
    self, Action, ActionOutcome, OpEntry, OpLog, Preview, RebaseRange, Repo, RunOptions, StashEntry,
};
use crate::backup::{BackupRecord, Backups, RestorePreview, Retention, unix_now};
use crate::git::state::{RepoState, read_state};
use crate::git::watch::{DEFAULT_DEBOUNCE, DEFAULT_MAX_WAIT, RepoWatcher};
use crate::git::{GitInstall, detect};
use crate::github::{Account, AuthStatus, DevicePrompt, GitHub, GitHubError};
use crate::history::diff::{self, Blame, CommitDetail, FileDiff, WorkSide, WorkStat, parse_commit};
use crate::history::graph::{self, Graph, GraphInfo, GraphOptions, GraphRow};
use crate::history::search::{LogQuery, SearchResult, search};
use crate::ipc::{AppError, ErrorKind, EventSink, TaskEvent, TaskId, TaskInfo, TaskManager};
use crate::proj::{WorkflowMode, private_root, public_root};
use crate::projects::{
    ProjectEntry, ProjectSummary, Registry, RepoSide, Session, Settings, clone_project,
    create_project, ensure_repo, load_proj, read_json, summarize, write_json,
};
use crate::theme::{self, ThemeList};

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RepoChanged {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CloneResult {
    #[ts(type = "string")]
    pub path: PathBuf,
    /// Set when the clone held a valid `.proj` file and was added to the hub.
    pub project: Option<ProjectEntry>,
    /// Why the clone was not added (no or invalid `.proj` file). The clone is kept either way.
    pub not_added: Option<AppError>,
}

pub struct AppState {
    registry: Arc<Mutex<Registry>>,
    tasks: TaskManager,
    watcher: Mutex<Option<RepoWatcher>>,
    /// Watched working-tree roots mapped to the project id they belong to. A two-repo project
    /// contributes two roots (its private and public repos), both mapping to the same id.
    watched: Arc<Mutex<BTreeMap<PathBuf, String>>>,
    github: Arc<GitHub>,
    data_dir: PathBuf,
    themes_dir: PathBuf,
    /// Keeps the themes folder watcher alive.
    _themes_watcher: Option<notify::RecommendedWatcher>,
    /// The last graph built for each project, by id.
    graphs: Arc<Mutex<BTreeMap<String, Arc<Graph>>>>,
    /// Source of graph generations; unique across projects and rebuilds.
    graph_generation: Arc<AtomicU64>,
    /// Latest search number per project; a running search stops when it is no longer the latest.
    searches: Arc<Mutex<BTreeMap<String, u64>>>,
    /// Every git command and backup a write action ran.
    oplog: OpLog,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Key for the per-project graph cache and search registry, separating a two-repo project's two
/// sides so the private and public graphs never collide. The private side keeps the bare id for
/// backward compatibility.
fn repo_key(id: &str, side: RepoSide) -> String {
    match side {
        RepoSide::Private => id.to_string(),
        RepoSide::Public => format!("{id}\u{1f}public"),
    }
}

struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: &TaskEvent) {
        if let Err(e) = self.0.emit("task", event) {
            log::warn!("could not send task event: {e}");
        }
    }
}

impl AppState {
    pub fn new(app: &AppHandle) -> Result<AppState, Box<dyn std::error::Error>> {
        let data_dir = app.path().app_data_dir()?;
        let themes_dir = app.path().app_config_dir()?.join("themes");
        if let Err(e) = std::fs::create_dir_all(&themes_dir) {
            log::warn!("could not create {}: {e}", themes_dir.display());
        }
        let registry = Arc::new(Mutex::new(Registry::load(data_dir.join("projects.json"))));
        let watched = Arc::new(Mutex::new(BTreeMap::<PathBuf, String>::new()));

        let handle = app.clone();
        let ids_for = watched.clone();
        let watcher = RepoWatcher::new(DEFAULT_DEBOUNCE, DEFAULT_MAX_WAIT, move |roots| {
            let ids: BTreeSet<String> = lock(&ids_for)
                .iter()
                .filter(|(p, _)| roots.contains(*p))
                .map(|(_, id)| id.clone())
                .collect();
            if !ids.is_empty() {
                let _ = handle.emit(
                    "repo-changed",
                    RepoChanged {
                        ids: ids.into_iter().collect(),
                    },
                );
            }
        })
        .map_err(|e| log::warn!("file watching is off: {e}"))
        .ok();

        Ok(AppState {
            registry,
            tasks: TaskManager::new(TauriSink(app.clone())),
            watcher: Mutex::new(watcher),
            watched,
            github: Arc::new(GitHub::production(&data_dir)),
            data_dir: data_dir.clone(),
            _themes_watcher: watch_themes(app.clone(), themes_dir.clone()),
            themes_dir,
            graphs: Default::default(),
            graph_generation: Arc::new(AtomicU64::new(1)),
            searches: Default::default(),
            oplog: {
                let handle = app.clone();
                OpLog::persistent(data_dir.join("oplog.jsonl"), move |e| {
                    let _ = handle.emit("op-log", e);
                })
            },
        })
    }

    fn session_file(&self) -> PathBuf {
        self.data_dir.join("session.json")
    }

    fn settings_file(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    /// The app data directory (registry, session, settings, oplog, release zips).
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn entry(&self, id: &str) -> Result<ProjectEntry, AppError> {
        lock(&self.registry).get(id).cloned()
    }

    /// The working-tree root of one side of a project: the private repo, or, for a two-repo
    /// project, the separate public repo resolved from the `.proj` file. Both are resolved from
    /// the project root (the folder holding the `.proj`); the proj itself is loaded once per
    /// side. Asking for the public side of a single-repo project is an error (there it is a
    /// branch, not a separate repository).
    fn repo_path(&self, id: &str, side: RepoSide) -> Result<PathBuf, AppError> {
        let root = self.entry(id)?.path;
        let proj = load_proj(&root)?;
        match side {
            RepoSide::Private => Ok(private_root(&root, &proj)?),
            RepoSide::Public => {
                if proj.workflow.mode != WorkflowMode::TwoRepo {
                    return Err(AppError::invalid(
                        "this project has no separate public repository (single-repo mode)",
                    ));
                }
                Ok(public_root(&root, &proj)?)
            }
        }
    }

    /// The cached graph of a project, if it is still the generation the caller has.
    /// Start a new search for a project, which cancels the one before it. Returns its number.
    fn next_search(&self, id: &str) -> u64 {
        let mut s = lock(&self.searches);
        let n = s.entry(id.to_string()).or_default();
        *n += 1;
        *n
    }

    fn graph(&self, id: &str, generation: u64) -> Result<Arc<Graph>, AppError> {
        match lock(&self.graphs).get(id) {
            Some(g) if g.generation == generation => Ok(g.clone()),
            _ => Err(AppError::new(
                ErrorKind::Stale,
                "the history changed; reload the graph",
            )),
        }
    }
}

/// Emit `themes-changed` when the themes folder changes, after the burst of events an editor
/// save makes has settled.
fn watch_themes(app: AppHandle, dir: PathBuf) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok_and(|e| !matches!(e.kind, notify::EventKind::Access(_))) {
            let _ = tx.send(());
        }
    })
    .ok()?;
    watcher
        .watch(&dir, notify::RecursiveMode::NonRecursive)
        .map_err(|e| log::warn!("theme hot reload is off: {e}"))
        .ok()?;
    std::thread::Builder::new()
        .name("pm-themes".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
                let _ = app.emit("themes-changed", theme::load(&dir));
            }
        })
        .ok()?;
    Some(watcher)
}

/// Run blocking work (git, file system) off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal(format!("background work failed: {e}")))?
}

#[tauri::command]
pub async fn git_check() -> Result<GitInstall, AppError> {
    blocking(|| Ok(detect()?)).await
}

#[tauri::command]
pub fn tasks_running(state: State<'_, AppState>) -> Vec<TaskInfo> {
    state.tasks.running()
}

#[tauri::command]
pub fn task_cancel(state: State<'_, AppState>, id: TaskId) -> bool {
    state.tasks.cancel(id)
}

/// Every registered project's hub card, in hub order. Cards are built in parallel.
#[tauri::command]
pub async fn projects_list(state: State<'_, AppState>) -> Result<Vec<ProjectSummary>, AppError> {
    let entries = lock(&state.registry).list().to_vec();
    let registry = state.registry.clone();
    blocking(move || {
        let summaries: Vec<ProjectSummary> = std::thread::scope(|s| {
            let handles: Vec<_> = entries
                .iter()
                .map(|e| s.spawn(move || summarize(e)))
                .collect();
            handles
                .into_iter()
                .zip(&entries)
                .map(|(h, e)| {
                    h.join().unwrap_or_else(|_| {
                        // Only the registry details, with the error on the card.
                        let mut s = summarize(&ProjectEntry {
                            path: PathBuf::new(),
                            ..e.clone()
                        });
                        s.path = e.path.clone();
                        s.repo_error = Some(AppError::internal("reading the project failed"));
                        s
                    })
                })
                .collect()
        });
        let mut reg = lock(&registry);
        for s in &summaries {
            if s.proj_error.is_none() {
                reg.update_name(&s.id, &s.name)?;
            }
        }
        Ok(summaries)
    })
    .await
}

#[tauri::command]
pub async fn project_summary(
    state: State<'_, AppState>,
    id: String,
) -> Result<ProjectSummary, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || Ok(summarize(&entry))).await
}

/// Full repository state for a project tab.
#[tauri::command]
pub async fn project_state(
    state: State<'_, AppState>,
    id: String,
    repo_side: Option<RepoSide>,
) -> Result<RepoState, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        Ok(read_state(&root, true)?)
    })
    .await
}

/// Add an existing repository. With `create` set, a repository without a `.proj` file gets a
/// starter one for that workflow mode.
#[tauri::command]
pub async fn project_add(
    state: State<'_, AppState>,
    path: PathBuf,
    create: Option<WorkflowMode>,
) -> Result<ProjectEntry, AppError> {
    let registry = state.registry.clone();
    blocking(move || lock(&registry).add(&path, create, unix_now())).await
}

#[tauri::command]
pub async fn project_create(
    state: State<'_, AppState>,
    parent: PathBuf,
    name: String,
    mode: WorkflowMode,
) -> Result<ProjectEntry, AppError> {
    let registry = state.registry.clone();
    blocking(move || {
        let root = create_project(&parent, &name, mode)?;
        lock(&registry).add(&root, None, unix_now())
    })
    .await
}

/// Start a clone task and return its id. The result ([`CloneResult`]) arrives in the task's
/// `finished` event.
#[tauri::command]
pub fn project_clone(
    state: State<'_, AppState>,
    url: String,
    parent: PathBuf,
    name: Option<String>,
) -> Result<TaskId, AppError> {
    let registry = state.registry.clone();
    let title = format!("Clone {}", crate::git::url::redact(url.trim()));
    state.tasks.spawn("clone", title, None, move |ctx| {
        let path = clone_project(&url, &parent, name.as_deref(), ctx)?;
        let added = lock(&registry).add(&path, None, unix_now());
        Ok(match added {
            Ok(entry) => CloneResult {
                path,
                project: Some(entry),
                not_added: None,
            },
            Err(e) => CloneResult {
                path,
                project: None,
                not_added: Some(e),
            },
        })
    })
}

/// Forget a project. Its files are left alone.
#[tauri::command]
pub fn project_remove(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    lock(&state.registry).remove(&id)?;
    let mut watched = lock(&state.watched);
    let gone: Vec<PathBuf> = watched
        .iter()
        .filter(|(_, pid)| **pid == id)
        .map(|(p, _)| p.clone())
        .collect();
    if let Some(w) = lock(&state.watcher).as_mut() {
        for root in &gone {
            let _ = w.unwatch(root);
        }
    }
    for root in gone {
        watched.remove(&root);
    }
    Ok(())
}

#[tauri::command]
pub fn project_reorder(
    state: State<'_, AppState>,
    id: String,
    index: usize,
) -> Result<(), AppError> {
    lock(&state.registry).reorder(&id, index)
}

/// Watch exactly these projects for changes (the open tabs). Unknown ids are ignored.
#[tauri::command]
pub fn projects_watch(state: State<'_, AppState>, ids: Vec<String>) {
    // Each project contributes its private root and, for a two-repo project, its public root too,
    // so edits on either side refresh the tab. Both roots map to the same project id.
    let roots: Vec<(String, PathBuf)> = {
        let reg = lock(&state.registry);
        ids.iter()
            .filter_map(|id| reg.get(id).ok().map(|e| (id.clone(), e.path.clone())))
            .collect()
    };
    let mut wanted: Vec<(String, PathBuf)> = Vec::with_capacity(roots.len());
    for (id, root) in roots {
        let Ok(proj) = load_proj(&root) else { continue };
        if let Ok(private) = private_root(&root, &proj) {
            wanted.push((id.clone(), private));
        }
        if proj.workflow.mode == WorkflowMode::TwoRepo
            && let Ok(public) = public_root(&root, &proj)
        {
            wanted.push((id, public));
        }
    }
    let mut watcher = lock(&state.watcher);
    let Some(w) = watcher.as_mut() else { return };
    let mut watched = lock(&state.watched);
    let keep: BTreeSet<&PathBuf> = wanted.iter().map(|(_, p)| p).collect();
    for root in watched.keys() {
        if !keep.contains(root) {
            let _ = w.unwatch(root);
        }
    }
    watched.clear();
    for (id, root) in wanted {
        match w.watch(&root) {
            Ok(()) => {
                // The watcher reports canonical roots.
                let root = std::fs::canonicalize(&root).unwrap_or(root);
                watched.insert(root, id);
            }
            Err(e) => log::warn!("not watching {}: {e}", root.display()),
        }
    }
}

#[tauri::command]
pub fn session_load(state: State<'_, AppState>) -> Session {
    let mut s: Session = read_json(&state.session_file());
    s.sanitize(&lock(&state.registry));
    s
}

#[tauri::command]
pub fn session_save(state: State<'_, AppState>, session: Session) -> Result<(), AppError> {
    write_json(&state.session_file(), &session)
}

#[tauri::command]
pub fn settings_load(state: State<'_, AppState>) -> Settings {
    read_json(&state.settings_file())
}

#[tauri::command]
pub fn settings_save(state: State<'_, AppState>, settings: Settings) -> Result<(), AppError> {
    write_json(&state.settings_file(), &settings)
}

#[tauri::command]
pub async fn themes_list(state: State<'_, AppState>) -> Result<ThemeList, AppError> {
    let dir = state.themes_dir.clone();
    blocking(move || Ok(theme::load(&dir))).await
}

/// Where user themes go, so the settings page can show it.
#[tauri::command]
pub fn themes_folder(state: State<'_, AppState>) -> String {
    state.themes_dir.display().to_string()
}

/// Rewrite the user theme `<id>.json` with `colors` (a full, merged map — only the colors that
/// differ from the built-in theme of its scheme are stored).
#[tauri::command]
pub async fn theme_save_user(
    state: State<'_, AppState>,
    id: String,
    colors: BTreeMap<String, String>,
) -> Result<(), AppError> {
    let dir = state.themes_dir.clone();
    blocking(move || theme::save_user(&dir, &id, &colors).map_err(AppError::invalid)).await
}

/// Save a new user theme named `name` in the themes folder (its scheme is the built-in theme's),
/// returning the new theme's id.
#[tauri::command]
pub async fn theme_save_as(
    state: State<'_, AppState>,
    name: String,
    scheme: theme::ColorScheme,
    colors: BTreeMap<String, String>,
) -> Result<String, AppError> {
    let dir = state.themes_dir.clone();
    blocking(move || theme::save_as(&dir, &name, scheme, &colors).map_err(AppError::invalid)).await
}

/// Write the theme `<id>` to `path` as a complete file (all colors).
#[tauri::command]
pub async fn theme_export(
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> Result<(), AppError> {
    let dir = state.themes_dir.clone();
    blocking(move || {
        let theme = theme::load(&dir)
            .themes
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| AppError::invalid(format!("no theme named \"{id}\"")))?;
        theme::export_theme(&theme, std::path::Path::new(&path)).map_err(AppError::invalid)
    })
    .await
}

/// Validate the theme file at `path` and copy it into the themes folder, returning the new id.
#[tauri::command]
pub async fn theme_import(state: State<'_, AppState>, path: String) -> Result<String, AppError> {
    let dir = state.themes_dir.clone();
    blocking(move || {
        theme::import_theme(&dir, std::path::Path::new(&path)).map_err(AppError::invalid)
    })
    .await
}

/// A started GitHub sign-in: show `prompt` to the user. The account ([`Account`]) arrives in the
/// task's `finished` event once they approve; cancelling the task stops waiting.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    #[ts(type = "number")]
    pub task_id: TaskId,
    pub prompt: DevicePrompt,
}

/// The GitHub account. `check` asks GitHub whether the sign-in still works (a network call);
/// `gh` also looks for the GitHub CLI.
#[tauri::command]
pub async fn github_status(
    state: State<'_, AppState>,
    check: bool,
    gh: bool,
) -> Result<AuthStatus, AppError> {
    let github = state.github.clone();
    blocking(move || Ok(github.status(check, gh))).await
}

#[tauri::command]
pub async fn github_sign_in(state: State<'_, AppState>) -> Result<SignIn, AppError> {
    let github = state.github.clone();
    let grant = {
        let github = github.clone();
        blocking(move || Ok(github.sign_in_start()?)).await?
    };
    let prompt = grant.prompt.clone();
    let task_id = state.tasks.spawn(
        "github-sign-in",
        "Sign in to GitHub",
        // Not a repository: a fixed name so only one sign-in runs at a time.
        Some(PathBuf::from("<github-sign-in>")),
        move |ctx| {
            ctx.message("Waiting for approval on GitHub");
            let mut wait = |d: Duration| -> Result<(), GitHubError> {
                let until = std::time::Instant::now() + d;
                while std::time::Instant::now() < until {
                    if ctx.is_cancelled() {
                        return Err(GitHubError::Cancelled);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                if ctx.is_cancelled() {
                    return Err(GitHubError::Cancelled);
                }
                Ok(())
            };
            Ok(github.sign_in_finish(&grant, &mut wait)?)
        },
    )?;
    Ok(SignIn { task_id, prompt })
}

/// Use the GitHub CLI's login.
#[tauri::command]
pub async fn github_use_gh(state: State<'_, AppState>) -> Result<Account, AppError> {
    let github = state.github.clone();
    blocking(move || Ok(github.use_gh()?)).await
}

#[tauri::command]
pub async fn github_sign_out(state: State<'_, AppState>) -> Result<(), AppError> {
    let github = state.github.clone();
    blocking(move || Ok(github.sign_out()?)).await
}

/// Rows a single `graph_rows` call returns at most.
const MAX_GRAPH_ROWS: u32 = 2000;

/// Build (or reuse) the branch graph of a project. The graph is rebuilt only when HEAD, a ref or
/// the options changed since the last call, so the UI can call this on every `repo-changed`.
#[tauri::command]
pub async fn graph_load(
    state: State<'_, AppState>,
    id: String,
    options: GraphOptions,
    repo_side: Option<RepoSide>,
) -> Result<GraphInfo, AppError> {
    let side = repo_side.unwrap_or_default();
    let cache_key = repo_key(&id, side);
    let entry_root = state.entry(&id)?.path.clone();
    let root = state.repo_path(&id, side)?;
    let graphs = state.graphs.clone();
    let generations = state.graph_generation.clone();
    blocking(move || {
        let proj = load_proj(&entry_root)?;
        let staged_upto = crate::staging::graph_upto(&entry_root);
        let with_sides = |mut info: GraphInfo| {
            info.workflow_refs = graph::workflow_refs(&proj.workflow, &info.refs);
            info
        };
        let key = graph::current_key(&root, &options, staged_upto)?;
        if let Some(g) = lock(&graphs).get(&cache_key)
            && g.key == key
        {
            return Ok(with_sides(g.info()));
        }
        let generation = generations.fetch_add(1, Ordering::Relaxed);
        let g = Arc::new(graph::build(&root, &options, generation, staged_upto)?);
        let info = g.info();
        lock(&graphs).insert(cache_key, g);
        Ok(with_sides(info))
    })
    .await
}

/// Rows `start..start + count` of a graph returned by `graph_load`. Fails with `stale` when the
/// graph has been rebuilt since.
#[tauri::command]
pub async fn graph_rows(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
    start: u32,
    count: u32,
    repo_side: Option<RepoSide>,
) -> Result<Vec<GraphRow>, AppError> {
    let g = state.graph(&repo_key(&id, repo_side.unwrap_or_default()), generation)?;
    blocking(move || Ok(g.rows(start, count.min(MAX_GRAPH_ROWS))?)).await
}

/// Row of a commit, given as a full id or an abbreviation of at least 4 hex digits. `None` when
/// the commit is not in the graph (or the abbreviation is ambiguous).
#[tauri::command]
pub async fn graph_find(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
    commit: String,
    repo_side: Option<RepoSide>,
) -> Result<Option<u32>, AppError> {
    let g = state.graph(&repo_key(&id, repo_side.unwrap_or_default()), generation)?;
    blocking(move || g.find(&commit)).await
}

/// Search the log of a graph returned by `graph_load`. Starting a search cancels the previous
/// one for the same project (it fails with `cancelled`).
#[tauri::command]
pub async fn log_search(
    state: State<'_, AppState>,
    id: String,
    generation: u64,
    query: LogQuery,
    repo_side: Option<RepoSide>,
) -> Result<SearchResult, AppError> {
    let key = repo_key(&id, repo_side.unwrap_or_default());
    let g = state.graph(&key, generation)?;
    let mine = state.next_search(&key);
    let searches = state.searches.clone();
    blocking(move || search(&g, &query, || lock(&searches).get(&key) != Some(&mine))).await
}

/// Cancel a running search of a project, if any.
#[tauri::command]
pub fn log_search_cancel(state: State<'_, AppState>, id: String, repo_side: Option<RepoSide>) {
    state.next_search(&repo_key(&id, repo_side.unwrap_or_default()));
}

/// A commit's metadata and changed files, compared with parent `parent` (default: the first).
#[tauri::command]
pub async fn commit_detail(
    state: State<'_, AppState>,
    id: String,
    commit: String,
    parent: Option<u32>,
    repo_side: Option<RepoSide>,
) -> Result<CommitDetail, AppError> {
    let path = state.repo_path(&id, repo_side.unwrap_or_default())?;
    let commit = parse_commit(&commit)?;
    blocking(move || {
        ensure_repo(&path)?;
        let repo = crate::git::state::open(&path)?;
        diff::commit_detail(&repo, commit, parent)
    })
    .await
}

/// One file's changes in a commit. `oldPath` is the name before a rename. Files over 1 MiB come
/// back with `tooLarge` set unless `force` is true (never over 16 MiB).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn file_diff(
    state: State<'_, AppState>,
    id: String,
    commit: String,
    parent: Option<u32>,
    path: String,
    old_path: Option<String>,
    context: Option<u32>,
    force: bool,
    repo_side: Option<RepoSide>,
) -> Result<FileDiff, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    let commit = parse_commit(&commit)?;
    blocking(move || {
        ensure_repo(&root)?;
        let repo = crate::git::state::open(&root)?;
        diff::file_diff(
            &repo,
            commit,
            parent,
            &path,
            old_path.as_deref(),
            context.unwrap_or(3),
            force,
        )
    })
    .await
}

/// One file's uncommitted change: `staged` (HEAD to the index) or `unstaged` (the index to
/// the working tree). Size limits as for `file_diff`.
#[tauri::command]
pub async fn work_diff(
    state: State<'_, AppState>,
    id: String,
    path: String,
    side: WorkSide,
    context: Option<u32>,
    force: bool,
    repo_side: Option<RepoSide>,
) -> Result<FileDiff, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        let repo = crate::git::state::open(&root)?;
        diff::work_diff(&repo, &path, side, context.unwrap_or(3), force)
    })
    .await
}

/// Show a project's folder in the system file manager, or with `path` (relative to the project)
/// that file selected in its folder. Only registered projects can be opened this way.
#[tauri::command]
pub async fn open_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    path: Option<String>,
    repo_side: Option<RepoSide>,
) -> Result<(), AppError> {
    use tauri_plugin_opener::OpenerExt;
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    let result = match path {
        None => app.opener().open_path(root.to_string_lossy(), None::<&str>),
        Some(p) => {
            crate::actions::check_path(&p)?;
            let target = root.join(&p);
            // A deleted file has nothing to select: show the folder it was in.
            let target = if target.exists() {
                target
            } else {
                target.parent().unwrap_or(&root).to_path_buf()
            };
            app.opener().reveal_item_in_dir(target)
        }
    };
    result.map_err(|e| AppError::invalid(format!("Could not open the folder: {e}")))
}

/// The entries of one folder of a project (`""` is the top), for the Files view.
#[tauri::command]
pub async fn files_list(
    state: State<'_, AppState>,
    id: String,
    dir: String,
    repo_side: Option<RepoSide>,
) -> Result<crate::files::FileList, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || crate::files::list_dir(&root, &dir)).await
}

/// A text file of a project, for the editor.
#[tauri::command]
pub async fn file_read(
    state: State<'_, AppState>,
    id: String,
    path: String,
    repo_side: Option<RepoSide>,
) -> Result<crate::files::FileText, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || crate::files::read_text(&root, &path)).await
}

/// Save a text file of a project. The file is the user's own edit, so no restore point is taken.
#[tauri::command]
pub async fn file_write(
    state: State<'_, AppState>,
    id: String,
    path: String,
    content: String,
    repo_side: Option<RepoSide>,
) -> Result<(), AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || crate::files::write_text(&root, &path, &content)).await
}

/// Make a new empty file or folder in a project, for the Files view.
#[tauri::command]
pub async fn file_create(
    state: State<'_, AppState>,
    id: String,
    path: String,
    dir: bool,
    repo_side: Option<RepoSide>,
) -> Result<(), AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || crate::files::create(&root, &path, dir)).await
}

/// What kind of conflict a file is in and where the two sides disagree (the Changes view).
#[tauri::command]
pub async fn conflict_info(
    state: State<'_, AppState>,
    id: String,
    path: String,
    repo_side: Option<RepoSide>,
) -> Result<crate::git::conflict::ConflictInfo, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        Ok(crate::git::conflict::describe(&root, &path)?)
    })
    .await
}

/// Lines added and removed in each of the given uncommitted files (the Changes list).
#[tauri::command]
pub async fn work_stats(
    state: State<'_, AppState>,
    id: String,
    files: Vec<(String, WorkSide)>,
    repo_side: Option<RepoSide>,
) -> Result<Vec<WorkStat>, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        let repo = crate::git::state::open(&root)?;
        Ok(diff::work_stats(&repo, &files))
    })
    .await
}

/// Who last changed each line of a file, as of a commit.
#[tauri::command]
pub async fn file_blame(
    state: State<'_, AppState>,
    id: String,
    commit: String,
    path: String,
    repo_side: Option<RepoSide>,
) -> Result<Blame, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    let commit = parse_commit(&commit)?;
    blocking(move || {
        ensure_repo(&root)?;
        let repo = crate::git::state::open(&root)?;
        diff::blame(&repo, commit, &path)
    })
    .await
}

/// What a write action would do. For a dangerous action the preview's `token` must be passed
/// to `action_run`; it stops working when the repository changes.
#[tauri::command]
pub async fn action_preview(
    state: State<'_, AppState>,
    id: String,
    action: Action,
    allow_untrusted: bool,
    repo_side: Option<RepoSide>,
) -> Result<Preview, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || actions::preview(&Repo::new(&root), &action, allow_untrusted)).await
}

/// The stashes of a project, newest first. Each is named by its commit id.
#[tauri::command]
pub async fn stash_list(
    state: State<'_, AppState>,
    id: String,
    repo_side: Option<RepoSide>,
) -> Result<Vec<StashEntry>, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        actions::list_stashes(&Repo::new(&root))
    })
    .await
}

/// The commits of the current branch from `from` (a revision) to HEAD, oldest first, for the
/// interactive rebase editor. Refused for the same reasons the rebase itself would be.
#[tauri::command]
pub async fn rebase_range(
    state: State<'_, AppState>,
    id: String,
    from: String,
    repo_side: Option<RepoSide>,
) -> Result<RebaseRange, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        actions::rebase_range(&Repo::new(&root), &from)
    })
    .await
}

/// Start a write action as a task and return its id; the [`ActionOutcome`] arrives in the
/// task's `finished` event. One task per repository at a time.
#[tauri::command]
pub fn action_run(
    state: State<'_, AppState>,
    id: String,
    action: Action,
    token: Option<String>,
    allow_untrusted: bool,
    repo_side: Option<RepoSide>,
) -> Result<TaskId, AppError> {
    let entry = state.entry(&id)?;
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    let log = state.oplog.clone();
    let settings_file = state.settings_file();
    let kind = format!("action-{}", action.name());
    let title = format!("{} in {}", action.name(), entry.name);
    state
        .tasks
        .spawn(&kind, title, Some(root.clone()), move |ctx| {
            let backups = read_json::<Settings>(&settings_file).backups;
            let opts = RunOptions {
                allow_untrusted,
                backups,
                ..Default::default()
            };
            let repo = Repo::new(&root);
            let outcome: ActionOutcome = actions::run(
                &repo,
                &action,
                token.as_deref(),
                &opts,
                &log,
                &ctx.cancel,
                |p| ctx.progress(p),
            )?;
            if outcome.backup.is_some() {
                prune_backups(&root);
            }
            Ok(outcome)
        })
}

/// Apply the default retention after a new backup. A failure only costs disk space.
fn prune_backups(root: &std::path::Path) {
    match Backups::open(root).and_then(|b| b.prune(Retention::default(), unix_now())) {
        Ok(gone) if !gone.is_empty() => log::info!("pruned backups: {}", gone.join(", ")),
        Ok(_) => {}
        Err(e) => log::warn!("could not prune backups in {}: {e}", root.display()),
    }
}

/// The restore points of a project, newest first.
#[tauri::command]
pub async fn backups_list(
    state: State<'_, AppState>,
    id: String,
    repo_side: Option<RepoSide>,
) -> Result<Vec<BackupRecord>, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        Ok(Backups::open(&root).and_then(|b| b.list())?)
    })
    .await
}

/// What restoring a restore point would change.
#[tauri::command]
pub async fn backup_preview(
    state: State<'_, AppState>,
    id: String,
    backup: String,
    repo_side: Option<RepoSide>,
) -> Result<RestorePreview, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        Ok(Backups::open(&root).and_then(|b| b.preview(&backup))?)
    })
    .await
}

/// Return a project to a restore point, as a task. A new restore point of the current state is
/// taken first, so the restore can be undone too.
#[tauri::command]
pub fn backup_restore(
    state: State<'_, AppState>,
    id: String,
    backup: String,
    repo_side: Option<RepoSide>,
) -> Result<TaskId, AppError> {
    let entry = state.entry(&id)?;
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    let log = state.oplog.clone();
    let title = format!("restore in {}", entry.name);
    state
        .tasks
        .spawn("action-restore", title, Some(root.clone()), move |_ctx| {
            let label = Repo::new(&root).label();
            let started = std::time::Instant::now();
            let result = Backups::open(&root).and_then(|b| b.restore(&backup, &Default::default()));
            match result {
                Ok(out) => {
                    log.backup(
                        &label,
                        "restore",
                        Ok(&out.pre_restore.id),
                        started.elapsed(),
                    );
                    prune_backups(&root);
                    Ok(ActionOutcome {
                        title: format!("Restored {backup}"),
                        backup: Some(out.pre_restore),
                        conflicts: Vec::new(),
                    })
                }
                Err(e) => {
                    log.backup(&label, "restore", Err(e.to_string()), started.elapsed());
                    Err(AppError::from(e))
                }
            }
        })
}

/// Bring a restore point's refs back from its bundle (after the refs were deleted).
#[tauri::command]
pub async fn backups_import_bundle(
    state: State<'_, AppState>,
    id: String,
    backup: String,
    repo_side: Option<RepoSide>,
) -> Result<(), AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        Ok(Backups::open(&root).and_then(|b| b.import_bundle(&backup))?)
    })
    .await
}

/// Delete a restore point: its refs, bundle file and index entry. The repository is untouched.
#[tauri::command]
pub async fn backups_delete(
    state: State<'_, AppState>,
    id: String,
    backup: String,
    repo_side: Option<RepoSide>,
) -> Result<(), AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        Ok(Backups::open(&root).and_then(|b| b.delete(&backup))?)
    })
    .await
}

/// The restore points the default retention would delete: too many or too old. Nothing is deleted.
#[tauri::command]
pub async fn backups_cleanup_preview(
    state: State<'_, AppState>,
    id: String,
    repo_side: Option<RepoSide>,
) -> Result<Vec<BackupRecord>, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        Ok(Backups::open(&root)
            .and_then(|b| b.cleanup_preview(Retention::default(), unix_now()))?)
    })
    .await
}

/// Apply the default retention now: delete every restore point the preview would delete.
/// Returns the deleted ids.
#[tauri::command]
pub async fn backups_cleanup(
    state: State<'_, AppState>,
    id: String,
    repo_side: Option<RepoSide>,
) -> Result<Vec<String>, AppError> {
    let root = state.repo_path(&id, repo_side.unwrap_or_default())?;
    blocking(move || {
        ensure_repo(&root)?;
        Ok(Backups::open(&root).and_then(|b| b.cleanup(Retention::default(), unix_now()))?)
    })
    .await
}

/// The staging preview of a project.
#[tauri::command]
pub async fn staging_preview(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::staging::StagePreview, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::staging::preview(&entry.path)?)
    })
    .await
}

/// Sync the private side into the public side as a task. `allow` names files that may stage despite
/// a blocking finding; `message` overrides the generated commit message; `push` pushes the public
/// side after the commit.
#[tauri::command]
pub fn staging_sync(
    state: State<'_, AppState>,
    id: String,
    allow: Vec<String>,
    message: Option<String>,
    push: Option<bool>,
) -> Result<TaskId, AppError> {
    let entry = state.entry(&id)?;
    let title = format!("sync {}", entry.name);
    state.tasks.spawn(
        "staging-sync",
        title,
        Some(entry.path.clone()),
        move |_ctx| {
            let result = crate::staging::sync(&entry.path, &allow, message, push.unwrap_or(false));
            let out = result?;
            if !out.blockers.is_empty() {
                return Ok(ActionOutcome {
                    title: format!("Sync refused: {}", out.blockers.join(", ")),
                    backup: None,
                    conflicts: Vec::new(),
                });
            }
            Ok(ActionOutcome {
                title: format!(
                    "Synced {} file{}",
                    out.synced,
                    if out.synced == 1 { "" } else { "s" }
                ),
                backup: None,
                conflicts: Vec::new(),
            })
        },
    )
}

/// Verify the public side against the allowlist.
#[tauri::command]
pub async fn staging_verify(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::staging::VerifyReport, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::staging::verify(&entry.path)?)
    })
    .await
}

/// The staging records, oldest first.
#[tauri::command]
pub async fn staging_records(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<crate::staging::StagingRecord>, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::staging::records(&entry.path)?)
    })
    .await
}

/// The diff of one file between the private and public sides (or "not in this project" when
/// the path is unknown). The public side is `old`, the private side `new`.
#[tauri::command]
pub async fn staging_diff(
    state: State<'_, AppState>,
    id: String,
    path: String,
    force: bool,
) -> Result<crate::history::diff::FileDiff, AppError> {
    let entry = state.entry(&id)?;
    if crate::history::search::clean_path(&path).is_err() {
        return Err(AppError::invalid(
            "the file path must be a plain repository-relative path",
        ));
    }
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::staging::diff(&entry.path, &path, force)?)
    })
    .await
}

/// Set or clear a per-file override in `.proj`. `kind` is "include", "exclude" or "clear".
#[tauri::command]
pub async fn staging_set_override(
    state: State<'_, AppState>,
    id: String,
    path: String,
    kind: String,
) -> Result<(), AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::staging::set_override(&entry.path, &path, &kind)?)
    })
    .await
}

/// The release bump preview: what moves, and to what. Writes nothing.
#[tauri::command]
pub async fn release_preview(
    state: State<'_, AppState>,
    id: String,
    bump: crate::release::Bump,
    exact: Option<String>,
) -> Result<crate::release::ReleasePreview, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::preview_bump(
            &entry.path,
            bump,
            exact.as_deref(),
        )?)
    })
    .await
}

/// Apply a release bump: rewrite `.proj` and the configured version files.
#[tauri::command]
pub async fn release_apply(
    state: State<'_, AppState>,
    id: String,
    bump: crate::release::Bump,
    exact: Option<String>,
) -> Result<crate::release::ReleasePreview, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::apply_bump(
            &entry.path,
            bump,
            exact.as_deref(),
        )?)
    })
    .await
}

/// The release tag preview: what would be created, and whether the checks pass. Reads only.
#[tauri::command]
pub async fn release_tag_preview(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::release::TagPreview, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::tag_preview(&entry.path)?)
    })
    .await
}

/// Create the annotated release tag (and optionally push it).
#[tauri::command]
pub async fn release_tag_create(
    state: State<'_, AppState>,
    id: String,
    push: bool,
) -> Result<crate::release::TagResult, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::create_tag(&entry.path, push)?)
    })
    .await
}

/// Build the release zip: the staged public files, with a manifest and checksum.
#[tauri::command]
pub async fn release_zip(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::release::ZipOutcome, AppError> {
    let entry = state.entry(&id)?;
    let data_dir = state.data_dir();
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::build_release_zip(&entry.path, &data_dir)?)
    })
    .await
}

#[tauri::command]
pub async fn release_changelog(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::release::Changelog, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::generate_changelog(&entry.path)?)
    })
    .await
}

/// The recorded releases (`.proj`'s `[[release.history]]` table, oldest first).
#[tauri::command]
pub async fn release_history_list(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<crate::proj::ReleaseHistoryEntry>, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        let proj = crate::proj::ProjFile::load(&crate::proj::find_proj_file(&entry.path)?)?;
        Ok(proj.release.history)
    })
    .await
}

/// Record the current version's release in the history table. Idempotent: a tag that is
/// already recorded is left alone (no change, `false`).
#[tauri::command]
pub async fn release_history_record(
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        load_proj(&entry.path)?;
        let proj = crate::proj::ProjFile::load(&crate::proj::find_proj_file(&entry.path)?)?;
        let tag = format!("v{}", proj.project.version);
        Ok(crate::release::record_release_history(&entry.path, &tag)?)
    })
    .await
}

/// The ordered checklist of the current version's release (read-only).
#[tauri::command]
pub async fn release_checklist(
    state: State<'_, AppState>,
    id: String,
) -> Result<crate::release::checklist::ReleaseChecklist, AppError> {
    let entry = state.entry(&id)?;
    let github = state.github.clone();
    let data_dir = state.data_dir();
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::checklist::checklist(
            &entry.path,
            &data_dir,
            &github,
        ))
    })
    .await
}

/// The public repo's releases (GitHub's order).
#[tauri::command]
pub async fn release_github_list(
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<crate::release::github::GitHubRelease>, AppError> {
    let entry = state.entry(&id)?;
    let github = state.github.clone();
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::github::list_releases(&github, &entry.path)?)
    })
    .await
}

/// Create a GitHub release. The tag is `v<version>` unless one is given.
#[tauri::command]
pub async fn release_github_create(
    state: State<'_, AppState>,
    id: String,
    body: String,
    name: Option<String>,
    tag: Option<String>,
    draft: bool,
    prerelease: bool,
) -> Result<crate::release::github::GitHubRelease, AppError> {
    let entry = state.entry(&id)?;
    let github = state.github.clone();
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::github::create_release(
            &github,
            &entry.path,
            tag.as_deref(),
            name.as_deref(),
            &body,
            draft,
            prerelease,
        )?)
    })
    .await
}

/// Edit a release. Only the given fields are sent.
#[tauri::command]
pub async fn release_github_update(
    state: State<'_, AppState>,
    id: String,
    release_id: u64,
    name: Option<String>,
    body: Option<String>,
    draft: Option<bool>,
    prerelease: Option<bool>,
) -> Result<crate::release::github::GitHubRelease, AppError> {
    let entry = state.entry(&id)?;
    let github = state.github.clone();
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::github::update_release(
            &github,
            &entry.path,
            release_id,
            name.as_deref(),
            body.as_deref(),
            draft,
            prerelease,
        )?)
    })
    .await
}

/// Delete a release.
#[tauri::command]
pub async fn release_github_delete(
    state: State<'_, AppState>,
    id: String,
    release_id: u64,
) -> Result<(), AppError> {
    let entry = state.entry(&id)?;
    let github = state.github.clone();
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::github::delete_release(
            &github,
            &entry.path,
            release_id,
        )?)
    })
    .await
}

/// Upload the release zip as an asset on the release.
#[tauri::command]
pub async fn release_github_upload(
    state: State<'_, AppState>,
    id: String,
    release_id: u64,
) -> Result<crate::release::github::ReleaseAsset, AppError> {
    let entry = state.entry(&id)?;
    let github = state.github.clone();
    let data_dir = state.data_dir();
    blocking(move || {
        load_proj(&entry.path)?;
        Ok(crate::release::github::upload_asset(
            &github,
            &entry.path,
            &data_dir,
            release_id,
        )?)
    })
    .await
}

/// Replace the zip asset on the release (delete the old one, then upload the current zip).
#[tauri::command]
pub async fn release_github_replace_asset(
    state: State<'_, AppState>,
    id: String,
    release_id: u64,
) -> Result<crate::release::github::ReleaseAsset, AppError> {
    let entry = state.entry(&id)?;
    let github = state.github.clone();
    let data_dir = state.data_dir();
    blocking(move || {
        load_proj(&entry.path)?;
        let releases = crate::release::github::list_releases(&github, &entry.path)?;
        let release = releases
            .into_iter()
            .find(|r| r.id == release_id)
            .ok_or_else(|| {
                AppError::not_found(format!("release {release_id} not found in the public repo"))
            })?;
        Ok(crate::release::github::replace_asset(
            &github,
            &entry.path,
            &data_dir,
            &release,
        )?)
    })
    .await
}

/// The operation log, oldest first.
#[tauri::command]
pub fn op_log(state: State<'_, AppState>) -> Vec<OpEntry> {
    state.oplog.entries()
}

#[tauri::command]
pub fn op_log_clear(state: State<'_, AppState>) {
    state.oplog.clear();
}

/// A copyable, secret-free diagnostics report (version, platform, project count, log file, tail).
/// Never fails: a missing or unreadable log file yields an empty tail.
#[tauri::command]
pub fn get_diagnostics(state: State<'_, AppState>) -> crate::diagnostics::Diagnostics {
    let projects = lock(&state.registry).list().len();
    crate::diagnostics::build(&state.data_dir(), projects)
}

/// Export the app-level state (registry, session, settings, operation log) as a zip
/// at `path` (one the user picked in a dialog).
#[tauri::command]
pub fn app_data_export(state: State<'_, AppState>, path: String) -> Result<(), AppError> {
    let data_dir = state.data_dir();
    if path.is_empty() {
        return Err(AppError::invalid("no file path given"));
    }
    crate::app_data::export_state(&data_dir, std::path::Path::new(&path)).map(|_| ())
}

/// Restore the app-level state from a zip written by `app_data_export`. Returns the
/// files restored, in archive order.
#[tauri::command]
pub fn app_data_restore(state: State<'_, AppState>, path: String) -> Result<Vec<String>, AppError> {
    let data_dir = state.data_dir();
    if path.is_empty() {
        return Err(AppError::invalid("no file path given"));
    }
    crate::app_data::restore_state(&data_dir, std::path::Path::new(&path))
}

/// Write the diagnostics report to `path` (one the user picked in a dialog).
#[tauri::command]
pub fn diagnostics_save(state: State<'_, AppState>, path: String) -> Result<(), AppError> {
    let data_dir = state.data_dir();
    let projects = lock(&state.registry).list().len();
    if path.is_empty() {
        return Err(AppError::invalid("no file path given"));
    }
    crate::diagnostics::save(&data_dir, std::path::Path::new(&path), projects)
        .map_err(AppError::invalid)
}
