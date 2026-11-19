//! The Tauri commands the UI calls, and the events it listens to. Rules are in [`crate::ipc`]:
//! commands name projects by registry id, and only the add, create and clone flows take a path
//! (one the user picked in a dialog).
//!
//! Events:
//! - `task` ([`TaskEvent`]): background task started, progressed, finished.
//! - `repo-changed` ([`RepoChanged`]): files changed in watched projects; re-read their state.
//! - `themes-changed` ([`ThemeList`]): a file in the themes folder changed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use ts_rs::TS;

use crate::backup::unix_now;
use crate::git::state::{RepoState, read_state};
use crate::git::watch::{DEFAULT_DEBOUNCE, DEFAULT_MAX_WAIT, RepoWatcher};
use crate::git::{GitInstall, detect};
use crate::github::{Account, AuthStatus, DevicePrompt, GitHub, GitHubError};
use crate::history::diff::{self, Blame, CommitDetail, FileDiff, parse_commit};
use crate::history::graph::{self, Graph, GraphInfo, GraphOptions, GraphRow};
use crate::history::search::{LogQuery, SearchResult, search};
use crate::ipc::{AppError, ErrorKind, EventSink, TaskEvent, TaskId, TaskInfo, TaskManager};
use crate::proj::WorkflowMode;
use crate::projects::{
    ProjectEntry, ProjectSummary, Registry, Session, Settings, clone_project, create_project,
    inspect, read_json, summarize, write_json,
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
    /// Project roots being watched, by id.
    watched: Arc<Mutex<BTreeMap<String, PathBuf>>>,
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
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
        let watched = Arc::new(Mutex::new(BTreeMap::<String, PathBuf>::new()));

        let handle = app.clone();
        let ids_for = watched.clone();
        let watcher = RepoWatcher::new(DEFAULT_DEBOUNCE, DEFAULT_MAX_WAIT, move |roots| {
            let ids: Vec<String> = lock(&ids_for)
                .iter()
                .filter(|(_, p)| roots.contains(*p))
                .map(|(id, _)| id.clone())
                .collect();
            if !ids.is_empty() {
                let _ = handle.emit("repo-changed", RepoChanged { ids });
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
            data_dir,
            _themes_watcher: watch_themes(app.clone(), themes_dir.clone()),
            themes_dir,
            graphs: Default::default(),
            graph_generation: Arc::new(AtomicU64::new(1)),
            searches: Default::default(),
        })
    }

    fn session_file(&self) -> PathBuf {
        self.data_dir.join("session.json")
    }

    fn settings_file(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }

    fn entry(&self, id: &str) -> Result<ProjectEntry, AppError> {
        lock(&self.registry).get(id).cloned()
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
pub async fn project_state(state: State<'_, AppState>, id: String) -> Result<RepoState, AppError> {
    let entry = state.entry(&id)?;
    blocking(move || {
        inspect(&entry.path)?;
        Ok(read_state(&entry.path, true)?)
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
    if let Some(root) = lock(&state.watched).remove(&id)
        && let Some(w) = lock(&state.watcher).as_mut()
    {
        let _ = w.unwatch(&root);
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
    let wanted: BTreeMap<String, PathBuf> = {
        let reg = lock(&state.registry);
        ids.iter()
            .filter_map(|id| reg.get(id).ok().map(|e| (id.clone(), e.path.clone())))
            .collect()
    };
    let mut watcher = lock(&state.watcher);
    let Some(w) = watcher.as_mut() else { return };
    let mut watched = lock(&state.watched);
    let keep: BTreeSet<&PathBuf> = wanted.values().collect();
    for root in watched.values() {
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
                watched.insert(id, root);
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
) -> Result<GraphInfo, AppError> {
    let entry = state.entry(&id)?;
    let graphs = state.graphs.clone();
    let generations = state.graph_generation.clone();
    blocking(move || {
        let proj = inspect(&entry.path)?;
        let with_sides = |mut info: GraphInfo| {
            info.workflow_refs = graph::workflow_refs(&proj.workflow, &info.refs);
            info
        };
        let key = graph::current_key(&entry.path, &options)?;
        if let Some(g) = lock(&graphs).get(&id)
            && g.key == key
        {
            return Ok(with_sides(g.info()));
        }
        let generation = generations.fetch_add(1, Ordering::Relaxed);
        let g = Arc::new(graph::build(&entry.path, &options, generation)?);
        let info = g.info();
        lock(&graphs).insert(id, g);
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
) -> Result<Vec<GraphRow>, AppError> {
    let g = state.graph(&id, generation)?;
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
) -> Result<Option<u32>, AppError> {
    let g = state.graph(&id, generation)?;
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
) -> Result<SearchResult, AppError> {
    let g = state.graph(&id, generation)?;
    let mine = state.next_search(&id);
    let searches = state.searches.clone();
    blocking(move || search(&g, &query, || lock(&searches).get(&id) != Some(&mine))).await
}

/// Cancel a running search of a project, if any.
#[tauri::command]
pub fn log_search_cancel(state: State<'_, AppState>, id: String) {
    state.next_search(&id);
}

/// A commit's metadata and changed files, compared with parent `parent` (default: the first).
#[tauri::command]
pub async fn commit_detail(
    state: State<'_, AppState>,
    id: String,
    commit: String,
    parent: Option<u32>,
) -> Result<CommitDetail, AppError> {
    let path = state.entry(&id)?.path;
    let commit = parse_commit(&commit)?;
    blocking(move || {
        inspect(&path)?;
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
) -> Result<FileDiff, AppError> {
    let root = state.entry(&id)?.path;
    let commit = parse_commit(&commit)?;
    blocking(move || {
        inspect(&root)?;
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

/// Who last changed each line of a file, as of a commit.
#[tauri::command]
pub async fn file_blame(
    state: State<'_, AppState>,
    id: String,
    commit: String,
    path: String,
) -> Result<Blame, AppError> {
    let root = state.entry(&id)?.path;
    let commit = parse_commit(&commit)?;
    blocking(move || {
        inspect(&root)?;
        let repo = crate::git::state::open(&root)?;
        diff::blame(&repo, commit, &path)
    })
    .await
}
