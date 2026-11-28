//! Private to public staging.
//!
//! Two workflow modes, the same rules:
//!
//! - **two-repo**: the private working tree is the source; the public repo on disk is the
//!   destination. Its working tree is updated and committed.
//! - **single-repo**: the private branch is the source (committed state only — uncommitted work
//!   is left alone) and the public branch is the destination. The destination is updated in a
//!   temporary worktree, so the user's working tree is never touched.
//!
//! A file is staged when it passes the allowlist: built-in defaults first (everything, except
//! `.md` files except the three standard exceptions), then the `.proj` `include`/`exclude`
//! globs, then the manual per-file `overrides`. Later rules win. Each staged file is then checked
//! by the leak scan ([`scan_disk_file`] / [`scan_tree_file`]); blockers stop the file from
//! staging unless the run allows it, warnings never do.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use globset::Glob;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use gix::ObjectId;

use crate::git::{GitCommand, GitError};
use crate::proj::{DEFAULT_MAX_FILE_BYTES, DEFAULT_WARN_FILE_BYTES, ProjFile, WorkflowMode};

/// One allowlist rule. Later rules win.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rule {
    Include(String),
    Exclude(String),
}

/// The ordered allowlist for a project, highest priority last.
pub(crate) fn allowlist_rules(proj: &ProjFile) -> Vec<Rule> {
    let mut rules = vec![
        // Everything by default; markdown files stay private unless allowlisted; the three
        // standard names are the exceptions (the starter `.proj` includes them).
        Rule::Include("**".to_string()),
        Rule::Exclude("**/*.md".to_string()),
        Rule::Include("README.md".to_string()),
        Rule::Include("CHANGELOG.md".to_string()),
        Rule::Include("LICENSE".to_string()),
    ];
    for entry in &proj.staging.include {
        match entry.strip_prefix('!') {
            Some(pattern) => rules.push(Rule::Exclude(pattern.to_string())),
            None => rules.push(Rule::Include(entry.clone())),
        }
    }
    for entry in &proj.staging.exclude {
        rules.push(Rule::Exclude(entry.clone()));
    }
    for entry in &proj.staging.overrides.include {
        rules.push(Rule::Include(entry.clone()));
    }
    for entry in &proj.staging.overrides.exclude {
        rules.push(Rule::Exclude(entry.clone()));
    }
    rules
}

/// The compiled allowlist for a project. The matchers are compiled once here so the
/// per-file check does no glob compilation.
#[derive(Debug, Clone, Default)]
pub struct Allowlist {
    rules: Vec<(globset::GlobMatcher, bool)>,
    /// `[staging.map]`, public path -> private path.
    targets: std::collections::BTreeMap<String, String>,
    /// The mapped private paths, which never stage under their own name.
    sources: BTreeSet<String>,
}

impl Allowlist {
    pub(crate) fn from_rules(rules: Vec<Rule>) -> Self {
        let mut out = Vec::new();
        for rule in rules {
            let (pattern, include) = match rule {
                Rule::Include(p) => (p, true),
                Rule::Exclude(p) => (p, false),
            };
            if let Ok(glob) = Glob::new(&pattern) {
                out.push((glob.compile_matcher(), include));
            }
        }
        Self {
            rules: out,
            ..Self::default()
        }
    }

    /// The allowlist for `proj`: its rules plus its `[staging.map]`. Entries that would leave
    /// the project are ignored (validation reports them).
    pub(crate) fn for_proj(proj: &ProjFile) -> Self {
        let mut list = Self::from_rules(allowlist_rules(proj));
        for (from, to) in &proj.staging.map {
            let safe = |p: &str| {
                !p.is_empty()
                    && !Path::new(p).is_absolute()
                    && !Path::new(p)
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
            };
            if safe(from) && safe(to) && from != to {
                list.targets.insert(to.clone(), from.clone());
                list.sources.insert(from.clone());
            }
        }
        list
    }

    /// Whether this private path is staged under its own name. A mapped file stages under its
    /// target name instead, and a private file whose name is a map target is never staged.
    pub fn staged(&self, path: &str) -> bool {
        if self.sources.contains(path) || self.targets.contains_key(path) {
            return false;
        }
        self.rule_match(path)
    }

    /// Whether this public path is managed by staging (staged under its own name, or a map target).
    pub fn public_staged(&self, path: &str) -> bool {
        self.targets.contains_key(path) || self.staged(path)
    }

    /// The private path a public path is staged from.
    pub fn source_of<'a>(&'a self, public: &'a str) -> &'a str {
        self.targets.get(public).map_or(public, String::as_str)
    }

    /// The map targets whose private file is in `private` (public path, private path).
    fn mapped<'a>(
        &'a self,
        private: &'a BTreeSet<String>,
    ) -> impl Iterator<Item = &'a String> + 'a {
        self.targets
            .iter()
            .filter(move |(_, from)| private.contains(*from))
            .map(|(to, _)| to)
    }

    fn rule_match(&self, path: &str) -> bool {
        let mut last = false;
        for (matcher, include) in &self.rules {
            if matcher.is_match(path) {
                last = *include;
            }
        }
        last
    }
}

/// Status of one file against the public side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum StageFileStatus {
    /// On the private side, not on the public side.
    Added,
    /// On the private side, different on the public side.
    Changed,
    /// On the public side, not on the private side — the public copy is removed.
    Removed,
    /// Same on both sides.
    Unchanged,
}

/// One leak-scan finding kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum LeakKind {
    ApiKey,
    EnvFile,
    PrivateKey,
    DecompiledFolder,
    DecompiledExtension,
    DecompiledSignature,
    DecompiledArchive,
    CustomRegex,
    Oversize,
    Warn,
    SymlinkEscape,
}

/// A finding for one file. The value never travels with the finding; only the rule and file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LeakIssue {
    pub path: String,
    pub kind: LeakKind,
    pub blocking: bool,
    pub detail: String,
}

/// One file in the preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StagingFile {
    pub path: String,
    pub status: StageFileStatus,
    pub bytes: u64,
    pub lfs: bool,
    pub issues: Vec<LeakIssue>,
}

/// The preview of a staging run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StagePreview {
    pub files: Vec<StagingFile>,
    /// Blocking findings the sync would refuse to pass without an allow.
    pub blockers: usize,
    /// Whether the file list is truncated (the safety cap stopped the scan).
    pub truncated: bool,
}

/// The outcome of a sync.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SyncOutcome {
    /// Files copied or deleted on the public side.
    pub synced: usize,
    /// Files already identical.
    pub skipped: usize,
    /// The commit message used.
    pub message: Option<String>,
    /// Whether the public side was committed.
    pub committed: bool,
    /// The public side's commit after the sync.
    pub head: Option<String>,
    /// Whether the public side was pushed.
    pub pushed: bool,
    /// Non-blocking notes.
    pub warnings: Vec<String>,
    /// Files refused because of blocking findings the run did not allow.
    pub blockers: Vec<String>,
    /// Files allowed by the run despite blocking findings.
    pub allowed: Vec<String>,
}

/// A verification report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VerifyReport {
    pub ok: bool,
    pub issues: Vec<String>,
}

/// One staging record, kept in `.git/pm/staging.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StagingRecord {
    pub id: String,
    /// Unix seconds.
    pub at: u64,
    pub message: String,
    /// The private branch the stage was made from.
    pub source: String,
    /// The private tip the stage was made from (`None` for an unborn branch).
    pub commit: Option<String>,
    pub files: Vec<String>,
    /// Files allowed by the run despite blocking findings.
    pub allowed: Vec<String>,
}

// =========================================================================
// Errors

#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error(transparent)]
    Proj(#[from] crate::proj::ProjError),
    #[error(transparent)]
    Git(#[from] GitError),
    #[error("could not read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the public side has uncommitted changes outside the allowlist: {0}")]
    PublicDirty(String),
    #[error("the public side would not commit: {0}")]
    PublicCommit(String),
    #[error("{0} does not exist on either the private or the public side")]
    NotFound(String),
}

impl StagingError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        StagingError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    fn err_cmd(command: String, code: Option<i32>, stderr: String) -> Self {
        StagingError::Git(GitError::Failed {
            command,
            code,
            stderr,
        })
    }
}

// =========================================================================
// Loading and paths

/// Load the `.proj` file for a project root.
pub fn load_proj(root: &Path) -> Result<ProjFile, StagingError> {
    let path = crate::proj::find_proj_file(root)?;
    let doc = crate::proj::ProjDocument::load(&path)?;
    Ok(doc.to_proj()?)
}

/// The compiled allowlist for a project.
pub fn allowlist(root: &Path) -> Result<Allowlist, StagingError> {
    let proj = load_proj(root)?;
    Ok(Allowlist::for_proj(&proj))
}

/// The absolute path of the `.proj` file's folder, marked as the project root.
pub fn proj_root(root: &Path) -> Result<PathBuf, StagingError> {
    let path = crate::proj::find_proj_file(root)?;
    Ok(path.parent().unwrap_or(root).to_path_buf())
}

/// The public side's folder — two-repo only.
pub fn public_root(root: &Path, proj: &ProjFile) -> Result<PathBuf, StagingError> {
    Ok(crate::proj::public_root(root, proj)?)
}

/// `git rev-parse --show-prefix` with the project root as cwd: the path from the git root to the
/// project root (empty string when they coincide).
fn path_prefix(root: &Path) -> Result<String, StagingError> {
    let out = GitCommand::new(root)
        .args(["rev-parse", "--show-prefix"])
        .output()?;
    if !out.success() {
        return Err(StagingError::err_cmd(
            "git rev-parse --show-prefix".to_string(),
            out.code,
            out.stderr.to_string(),
        ));
    }
    Ok(out.stdout_str().trim().to_string())
}

/// The repository's absolute git directory (works for worktrees and `.git` files too).
fn git_dir(root: &Path) -> Result<PathBuf, StagingError> {
    let out = GitCommand::new(root)
        .args(["rev-parse", "--absolute-git-dir"])
        .run()?;
    Ok(PathBuf::from(out.stdout_str()))
}

/// Strip the git-root prefix from every entry, so every path here is project-root relative.
fn strip_prefix(files: Vec<String>, prefix: &str) -> Vec<String> {
    files
        .into_iter()
        .filter_map(|f| {
            if prefix.is_empty() {
                Some(f)
            } else {
                f.strip_prefix(prefix).map(|rest| rest.to_string())
            }
        })
        .collect()
}

fn split_z(text: &str) -> Vec<String> {
    text.split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.replace('\\', "/"))
        .collect()
}

fn list_private_files(root: &Path, proj: &ProjFile) -> Result<Vec<String>, StagingError> {
    let private = crate::proj::private_root(root, proj)?;
    let prefix = path_prefix(&private)?;
    let files = match proj.workflow.mode {
        WorkflowMode::TwoRepo => {
            let out = GitCommand::new(&private)
                .args([
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "-z",
                ])
                .output()?;
            if !out.success() {
                return Err(StagingError::err_cmd(
                    "git ls-files --cached --others --exclude-standard -z".to_string(),
                    out.code,
                    out.stderr.to_string(),
                ));
            }
            split_z(&out.stdout_str())
        }
        WorkflowMode::SingleRepo => {
            let out = GitCommand::new(&private)
                .args([
                    "ls-tree",
                    "--name-only",
                    "-r",
                    "-z",
                    &proj.workflow.private.branch,
                    "--",
                ])
                .output()?;
            if !out.success() {
                return Err(StagingError::err_cmd(
                    format!(
                        "git ls-tree --name-only -r -z {} --",
                        proj.workflow.private.branch
                    ),
                    out.code,
                    out.stderr.to_string(),
                ));
            }
            split_z(&out.stdout_str())
        }
    };
    Ok(strip_prefix(files, &prefix))
}

fn list_public_files(root: &Path, proj: &ProjFile) -> Result<Vec<String>, StagingError> {
    match proj.workflow.mode {
        WorkflowMode::TwoRepo => {
            let public_root = public_root(root, proj)?;
            let prefix = path_prefix(&public_root)?;
            let out = GitCommand::new(&public_root)
                .args([
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "-z",
                ])
                .output()?;
            if !out.success() {
                return Err(StagingError::err_cmd(
                    "git ls-files --cached --others --exclude-standard -z".to_string(),
                    out.code,
                    out.stderr.to_string(),
                ));
            }
            Ok(strip_prefix(split_z(&out.stdout_str()), &prefix))
        }
        WorkflowMode::SingleRepo => {
            let private = crate::proj::private_root(root, proj)?;
            let prefix = path_prefix(&private)?;
            let out = GitCommand::new(&private)
                .args([
                    "ls-tree",
                    "--name-only",
                    "-r",
                    "-z",
                    &proj.workflow.public.branch,
                    "--",
                ])
                .output()?;
            if !out.success() {
                return Ok(Vec::new()); // public branch absent: nothing public yet
            }
            Ok(strip_prefix(split_z(&out.stdout_str()), &prefix))
        }
    }
}

/// Whether the repository uses Git LFS (gitignore'd LFS filter in `.git/config`).
pub fn uses_lfs(root: &Path) -> bool {
    let proj = match load_proj(root) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let private = crate::proj::private_root(root, &proj).unwrap_or_else(|_| root.to_path_buf());
    git_dir(&private)
        .ok()
        .map(|dir| {
            dir.join("lfs").is_dir()
                && std::fs::read_to_string(dir.join("config"))
                    .map(|c| c.contains("filter=lfs") || c.contains("filter \"lfs\""))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Whether a file is LFS-tracked (via git's attribute rules).
fn lfs_tracked(root: &Path, path: &str) -> bool {
    GitCommand::new(root)
        .args(["check-attr", "filter", "--", path])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("filter: lfs"))
        .unwrap_or(false)
}

// =========================================================================
// Fingerprints

/// Hash of a file's content, normalizing line endings so a checkout with different autocrlf
/// settings does not spuriously show a difference. Works on the private disk and the public
/// disk alike.
fn fingerprint_file(path: &Path) -> Result<String, StagingError> {
    let raw = read_file_through_link(path)?;
    let normalized = normalize_line_endings(&raw);
    Ok(format!("sha256:{:x}", Sha256::digest(&normalized)))
}

fn normalize_line_endings(bytes: &[u8]) -> Vec<u8> {
    // CRLF -> LF, then lone CR -> LF.
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                out.push(b'\n');
                i += 2;
            } else {
                out.push(b'\n');
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn read_file_through_link(path: &Path) -> Result<Vec<u8>, StagingError> {
    let meta = std::fs::metadata(path).map_err(|e| StagingError::io(path, e))?;
    if meta.is_symlink() {
        let target = std::fs::read_link(path).map_err(|e| StagingError::io(path, e))?;
        let resolved = if target.is_absolute() {
            target
        } else {
            path.parent().unwrap_or(Path::new(".")).join(target)
        };
        return std::fs::read(&resolved).map_err(|e| StagingError::io(path, e));
    }
    std::fs::read(path).map_err(|e| StagingError::io(path, e))
}

/// A private-side fingerprint for single-repo mode (the committed blob, exact).
fn private_fingerprint(root: &Path, proj: &ProjFile, path: &str) -> Result<String, StagingError> {
    let scope = format!("{}:{}", proj.workflow.private.branch, path);
    let out = GitCommand::new(root)
        .args(["rev-parse", "--verify", &scope])
        .output()?;
    if !out.success() {
        return Ok(String::new());
    }
    Ok(format!("sha:{}", out.stdout_str().trim()))
}

/// Public-side fingerprint for one path.
fn public_fingerprint(root: &Path, proj: &ProjFile, path: &str) -> Result<String, StagingError> {
    match proj.workflow.mode {
        WorkflowMode::TwoRepo => {
            let public_root = public_root(root, proj)?;
            let full = public_root.join(path);
            if !full.exists() {
                return Ok(String::new());
            }
            fingerprint_file(&full)
        }
        WorkflowMode::SingleRepo => {
            let scope = format!("{}:{}", proj.workflow.public.branch, path);
            let out = GitCommand::new(root)
                .args(["rev-parse", "--verify", &scope])
                .output()?;
            if !out.success() {
                return Ok(String::new());
            }
            Ok(format!("sha:{}", out.stdout_str().trim()))
        }
    }
}

// =========================================================================
// Leak scan

fn is_env(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == ".env" || name.starts_with(".env.") || name.ends_with(".env")
}

fn contains_private_key(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim().starts_with("-----BEGIN ") && line.contains("PRIVATE KEY-----"))
}

fn is_decompiled_signature(text: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"(?i)^\s*(//|#|;|<!--|/\*).*(decompiled|auto-generated|generated by a decompiler|generated by decompiler)",
        )
        .expect("static regex")
    });
    text.lines().any(|line| re.is_match(line))
}

fn is_api_key(text: &str) -> bool {
    static RE: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        [
            r"ghp_[A-Za-z0-9]{36,}",
            r"gho_[A-Za-z0-9]{36,}",
            r"ghu_[A-Za-z0-9]{36,}",
            r"ghs_[A-Za-z0-9]{36,}",
            r"ghr_[A-Za-z0-9]{36,}",
            r"github_pat_[A-Za-z0-9_]{20,}",
            r"AKIA[0-9A-Z]{16}",
            r"sk-[A-Za-z0-9_-]{20,}",
            r"xox[baprs]-[A-Za-z0-9-]{10,}",
            r#"(?i)(?:api[_-]?key|token|secret|password|passwd|private[_-]?key|authorization|bearer)['"\s:=]+['"]?[A-Za-z0-9_\-.=+/]{16,}"#,
        ]
        .iter()
        .filter_map(|p| regex::Regex::new(p).ok())
        .collect()
    })
    .iter()
    .any(|re| re.is_match(text))
}

/// The compiled allow-glob matchers for a project, compiled once per preview/scan pass.
fn exempt_matchers(proj: &ProjFile) -> Vec<globset::GlobMatcher> {
    proj.staging
        .leak
        .allow
        .iter()
        .filter_map(|g| Glob::new(g).ok())
        .map(|g| g.compile_matcher())
        .collect()
}

fn exempt(matchers: &[globset::GlobMatcher], path: &str) -> bool {
    matchers.iter().any(|m| m.is_match(path))
}

/// Path-level checks (folders, extensions, archives, `.env`), shared by both modes.
fn path_issues(proj: &ProjFile, path: &str) -> Vec<LeakIssue> {
    let leak = &proj.staging.leak;
    let mut issues = Vec::new();
    let lower = path.to_lowercase();

    let mut folders = BTreeSet::new();
    for f in &leak.folders {
        folders.insert(f.to_lowercase());
    }
    if lower.split('/').any(|seg| folders.contains(seg)) {
        issues.push(LeakIssue {
            path: path.to_string(),
            kind: LeakKind::DecompiledFolder,
            blocking: true,
            detail: "decompiled-code folder".to_string(),
        });
    }

    let ext = if lower.contains('.') {
        lower.rsplit('.').next().unwrap_or("").to_string()
    } else {
        String::new()
    };
    if !ext.is_empty() {
        let ext_set: BTreeSet<String> = leak.extensions.iter().map(|e| e.to_lowercase()).collect();
        if ext_set.contains(&ext) {
            issues.push(LeakIssue {
                path: path.to_string(),
                kind: LeakKind::DecompiledExtension,
                blocking: true,
                detail: "decompiled-code extension".to_string(),
            });
        }
        let archive_set: BTreeSet<String> =
            leak.archives.iter().map(|e| e.to_lowercase()).collect();
        if archive_set.contains(&ext) {
            issues.push(LeakIssue {
                path: path.to_string(),
                kind: LeakKind::DecompiledArchive,
                blocking: true,
                detail: "packed asset archive".to_string(),
            });
        }
    }

    if is_env(path) {
        issues.push(LeakIssue {
            path: path.to_string(),
            kind: LeakKind::EnvFile,
            blocking: true,
            detail: ".env file".to_string(),
        });
    }
    issues
}

fn size_issues(proj: &ProjFile, path: &str, size: u64, lfs: bool) -> Vec<LeakIssue> {
    let leak = &proj.staging.leak;
    let mut issues = Vec::new();
    let warn_limit = leak.warn_file_bytes.unwrap_or(DEFAULT_WARN_FILE_BYTES);
    let max_limit = leak.max_file_bytes.unwrap_or(DEFAULT_MAX_FILE_BYTES);
    if size > max_limit && !lfs {
        issues.push(LeakIssue {
            path: path.to_string(),
            kind: LeakKind::Oversize,
            blocking: true,
            detail: "file over the size limit".to_string(),
        });
    }
    if size > warn_limit && !lfs {
        issues.push(LeakIssue {
            path: path.to_string(),
            kind: LeakKind::Warn,
            blocking: false,
            detail: "file over the warning limit".to_string(),
        });
    }
    issues
}

fn content_issues(proj: &ProjFile, path: &str, text: &str) -> Vec<LeakIssue> {
    let leak = &proj.staging.leak;
    let mut issues = Vec::new();
    if is_api_key(text) {
        issues.push(LeakIssue {
            path: path.to_string(),
            kind: LeakKind::ApiKey,
            blocking: true,
            detail: "API key or token".to_string(),
        });
    }
    if contains_private_key(text) {
        issues.push(LeakIssue {
            path: path.to_string(),
            kind: LeakKind::PrivateKey,
            blocking: true,
            detail: "private-key block".to_string(),
        });
    }
    if is_decompiled_signature(text) {
        issues.push(LeakIssue {
            path: path.to_string(),
            kind: LeakKind::DecompiledSignature,
            blocking: true,
            detail: "decompiler header".to_string(),
        });
    }
    for re in &leak.custom_regexes {
        let matched = regex::Regex::new(re)
            .map(|re| re.is_match(text))
            .unwrap_or(false);
        if matched {
            issues.push(LeakIssue {
                path: path.to_string(),
                kind: LeakKind::CustomRegex,
                blocking: true,
                detail: "custom rule matched".to_string(),
            });
            break;
        }
    }
    issues
}

/// Leak scan for a two-repo file (on disk). Content checks are capped to keep the scan cheap.
fn scan_disk_file(
    proj: &ProjFile,
    root: &Path,
    path: &str,
    exempts: &[globset::GlobMatcher],
) -> Result<Vec<LeakIssue>, StagingError> {
    if exempt(exempts, path) {
        return Ok(Vec::new());
    }
    let mut issues = path_issues(proj, path);

    let full = root.join(path);
    let md = std::fs::metadata(&full).map_err(|e| StagingError::io(&full, e))?;
    let size = md.len();
    let lfs = lfs_tracked(root, path);

    issues.extend(size_issues(proj, path, size, lfs));

    if size <= 2 * 1024 * 1024 {
        let bytes = std::fs::read(&full).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes).to_string();
        issues.extend(content_issues(proj, path, &text));
    }

    Ok(issues)
}

/// Leak scan for a single-repo file (committed blob in the private branch).
fn scan_tree_file(
    root: &Path,
    proj: &ProjFile,
    path: &str,
    exempts: &[globset::GlobMatcher],
) -> Result<Vec<LeakIssue>, StagingError> {
    if exempt(exempts, path) {
        return Ok(Vec::new());
    }
    let mut issues = path_issues(proj, path);

    let branch = &proj.workflow.private.branch;
    let scope = format!("{branch}:{path}");
    let out = GitCommand::new(root)
        .args(["cat-file", "blob", &scope])
        .output()?;
    if !out.success() {
        return Ok(issues);
    }
    let bytes = out.stdout;
    let size = bytes.len() as u64;
    let lfs = lfs_tracked(root, path);

    issues.extend(size_issues(proj, path, size, lfs));

    if size <= 2 * 1024 * 1024 {
        let text = String::from_utf8_lossy(&bytes).to_string();
        issues.extend(content_issues(proj, path, &text));
    }

    Ok(issues)
}

// =========================================================================
// Preview

const MAX_PREVIEW_FILES: usize = 5000;

/// The staged files, the leak scan and the status of each against the public side.
pub fn preview(root: &Path) -> Result<StagePreview, StagingError> {
    let proj = load_proj(root)?;
    let allowlist = Allowlist::for_proj(&proj);

    // Every path here is a public path; `allowlist.source_of` names the private file behind it.
    let listed: BTreeSet<String> = list_private_files(root, &proj)?.into_iter().collect();
    let mut private: BTreeSet<String> = listed
        .iter()
        .filter(|f| allowlist.staged(f))
        .cloned()
        .collect();
    private.extend(allowlist.mapped(&listed).cloned());
    let public: BTreeSet<String> = list_public_files(root, &proj)?
        .into_iter()
        .filter(|f| allowlist.public_staged(f))
        .collect();

    let mut files = Vec::new();
    let mut blockers = 0usize;
    let mut truncated = false;

    let mut sorted: Vec<String> = private.iter().cloned().collect();
    sorted.sort();
    if sorted.len() > MAX_PREVIEW_FILES {
        sorted.truncate(MAX_PREVIEW_FILES);
        truncated = true;
    }

    let private = crate::proj::private_root(root, &proj)?;
    let canon_root = std::fs::canonicalize(&private).unwrap_or_else(|_| private.to_path_buf());
    let exempt_matchers = exempt_matchers(&proj);

    for f in &sorted {
        let mut issues = Vec::new();
        let src = allowlist.source_of(f);
        let full = private.join(src);

        // Symlink handling: refuse the file when the link escapes the project, and skip the
        // content read (a broken link must not fail the whole preview).
        let symlink = std::fs::symlink_metadata(&full)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if symlink {
            let escaped = std::fs::canonicalize(&full)
                .map(|target| !target.starts_with(&canon_root))
                .unwrap_or(true);
            if escaped {
                issues.push(LeakIssue {
                    path: f.clone(),
                    kind: LeakKind::SymlinkEscape,
                    blocking: true,
                    detail: "symlink points outside the project".to_string(),
                });
                files.push(StagingFile {
                    path: f.clone(),
                    status: StageFileStatus::Added,
                    bytes: 0,
                    lfs: false,
                    issues,
                });
                blockers += 1;
                continue;
            }
        }

        let private_fp = if proj.workflow.mode == WorkflowMode::TwoRepo {
            if full.exists() {
                fingerprint_file(&full)?
            } else {
                String::new()
            }
        } else {
            private_fingerprint(&private, &proj, src)?
        };
        let public_fp = public_fingerprint(root, &proj, f)?;
        let status = if !public.contains(f) {
            StageFileStatus::Added
        } else if public_fp == private_fp && !public_fp.is_empty() {
            StageFileStatus::Unchanged
        } else {
            StageFileStatus::Changed
        };

        let bytes = std::fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
        let lfs = lfs_tracked(&private, src);

        let mut scan = if proj.workflow.mode == WorkflowMode::TwoRepo {
            if full.exists() {
                scan_disk_file(&proj, &private, src, &exempt_matchers)?
            } else {
                Vec::new()
            }
        } else {
            scan_tree_file(&private, &proj, src, &exempt_matchers)?
        };
        scan.append(&mut issues);

        blockers += scan.iter().filter(|i| i.blocking).count();

        files.push(StagingFile {
            path: f.clone(),
            status,
            bytes,
            lfs,
            issues: scan,
        });
    }

    // Public files that are allowlisted but gone from the private side.
    for f in public.iter() {
        if !sorted.contains(f) {
            files.push(StagingFile {
                path: f.clone(),
                status: StageFileStatus::Removed,
                bytes: 0,
                lfs: false,
                issues: Vec::new(),
            });
        }
    }

    files.sort_by(|a, b| {
        let rank = |f: &StagingFile| match f.status {
            StageFileStatus::Added => 0,
            StageFileStatus::Changed => 1,
            StageFileStatus::Removed => 2,
            StageFileStatus::Unchanged => 3,
        };
        rank(a).cmp(&rank(b)).then(a.path.cmp(&b.path))
    });

    Ok(StagePreview {
        files,
        blockers,
        truncated,
    })
}

// =========================================================================
// Sync

/// Sync the private side into the public side and record the run.
///
/// `allow` names files that may stage despite a blocking finding (one-time per-file exemptions
/// for this run). `message` overrides the generated commit message. `push` pushes the public side
/// to its origin after the commit.
pub fn sync(
    root: &Path,
    allow: &[String],
    message: Option<String>,
    push: bool,
) -> Result<SyncOutcome, StagingError> {
    let proj = load_proj(root)?;
    let preview = preview(root)?;

    let allow: BTreeSet<String> = allow.iter().cloned().collect();

    // Files with blocking findings that are not allowed: refuse the whole run.
    let refused: BTreeSet<String> = preview
        .files
        .iter()
        .filter(|f| {
            f.status != StageFileStatus::Removed
                && f.issues
                    .iter()
                    .any(|i| i.blocking && !allow.contains(&f.path))
        })
        .map(|f| f.path.clone())
        .collect();
    if !refused.is_empty() {
        return Ok(SyncOutcome {
            synced: 0,
            skipped: 0,
            message: None,
            committed: false,
            head: None,
            pushed: false,
            warnings: Vec::new(),
            blockers: refused.into_iter().collect(),
            allowed: Vec::new(),
        });
    }

    let changed: BTreeSet<String> = preview
        .files
        .iter()
        .filter(|f| f.status == StageFileStatus::Added || f.status == StageFileStatus::Changed)
        .map(|f| f.path.clone())
        .collect();
    let removable: BTreeSet<String> = preview
        .files
        .iter()
        .filter(|f| f.status == StageFileStatus::Removed)
        .map(|f| f.path.clone())
        .collect();
    let skipped = preview
        .files
        .iter()
        .filter(|f| f.status == StageFileStatus::Unchanged)
        .count();
    let allowed: Vec<String> = preview
        .files
        .iter()
        .filter(|f| allow.contains(&f.path))
        .map(|f| f.path.clone())
        .collect();

    let plan = SyncPlan {
        root,
        proj: &proj,
        changed: &changed,
        removable: &removable,
        skipped,
        message,
        push,
        allowed: &allowed,
    };
    match proj.workflow.mode {
        WorkflowMode::TwoRepo => sync_two_repo(&plan),
        WorkflowMode::SingleRepo => sync_single_repo(&plan),
    }
}

/// Everything a sync needs beyond the project file itself.
struct SyncPlan<'a> {
    root: &'a Path,
    proj: &'a ProjFile,
    changed: &'a BTreeSet<String>,
    removable: &'a BTreeSet<String>,
    skipped: usize,
    message: Option<String>,
    push: bool,
    allowed: &'a [String],
}

fn sync_two_repo(plan: &SyncPlan) -> Result<SyncOutcome, StagingError> {
    let root = plan.root;
    let proj = plan.proj;
    let changed = plan.changed;
    let removable = plan.removable;
    let skipped = plan.skipped;
    let message = plan.message.clone();
    let push = plan.push;
    let allowed = plan.allowed;
    let public_root = public_root(root, proj)?;
    let private = crate::proj::private_root(root, proj)?;

    // Refuse when the public side has uncommitted changes outside the allowlist.
    let mut dirty: BTreeSet<String> = BTreeSet::new();
    {
        let prefix = path_prefix(&public_root)?;
        let staged = GitCommand::new(&public_root)
            .args(["diff", "--name-only", "-z", "--cached"])
            .run()?;
        dirty.extend(strip_prefix(split_z(&staged.stdout_str()), &prefix));
        let unstaged = GitCommand::new(&public_root)
            .args(["diff", "--name-only", "-z"])
            .run()?;
        dirty.extend(strip_prefix(split_z(&unstaged.stdout_str()), &prefix));
        let others = GitCommand::new(&public_root)
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .run()?;
        dirty.extend(strip_prefix(split_z(&others.stdout_str()), &prefix));
    }
    let allowlist = Allowlist::for_proj(proj);
    let non_allowlisted: Vec<String> = dirty
        .into_iter()
        .filter(|f| !allowlist.public_staged(f))
        .collect();
    if !non_allowlisted.is_empty() {
        return Err(StagingError::PublicDirty(non_allowlisted.join(", ")));
    }

    let mut warnings = Vec::new();
    let mut synced = 0usize;

    for f in changed {
        let src = private.join(allowlist.source_of(f));
        let dst = public_root.join(f);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StagingError::io(parent, e))?;
        }
        copy_file(&src, &dst)?;
        synced += 1;
    }

    for f in removable {
        let dst = public_root.join(f);
        if dst.exists() {
            std::fs::remove_file(&dst).map_err(|e| StagingError::io(&dst, e))?;
            synced += 1;
        }
    }

    remove_empty_parents(&public_root, removable);

    let mut committed = false;
    let mut final_message = message.clone();

    if synced > 0 {
        let mut paths: Vec<String> = changed.iter().cloned().collect();
        paths.extend(removable.iter().cloned());
        git_add(&public_root, &paths)?;

        let msg = message.unwrap_or_else(|| {
            format!(
                "Stage {} file{}",
                synced,
                if synced == 1 { "" } else { "s" }
            )
        });
        final_message = Some(msg.clone());
        git_commit(&public_root, &msg)?;
        committed = true;
    }

    let head = if committed {
        git_head(&public_root)?
    } else {
        None
    };

    let mut pushed = false;
    if push && committed {
        let branch = proj.workflow.public.branch.clone();
        let remote = proj.workflow.public.remote.clone();
        let out = GitCommand::new(&public_root)
            .args(["push", &remote, &branch])
            .output()?;
        if out.success() {
            pushed = true;
        } else {
            let tail = out.stderr.trim().to_string();
            warnings.push(format!("push failed: {tail}"));
        }
    }

    let record_message = final_message
        .clone()
        .unwrap_or_else(|| "Staged files".to_string());
    let record = StagingRecord {
        id: record_id(),
        at: unix_now(),
        message: record_message,
        source: proj.workflow.private.branch.clone(),
        commit: private_tip(&private, proj),
        files: changed
            .iter()
            .cloned()
            .chain(removable.iter().cloned())
            .collect(),
        allowed: allowed.to_vec(),
    };
    append_staging_record(&private, &record)?;

    Ok(SyncOutcome {
        synced,
        skipped,
        message: final_message,
        committed,
        head,
        pushed,
        warnings,
        blockers: Vec::new(),
        allowed: allowed.to_vec(),
    })
}

fn remove_empty_parents(public_root: &Path, removable: &BTreeSet<String>) {
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    for f in removable {
        let mut dir = PathBuf::from(f).parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            if d.as_os_str().is_empty() {
                break;
            }
            dirs.insert(public_root.join(&d));
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    for dir in dirs {
        // Only succeeds when the directory is empty (or already gone); ignore otherwise.
        let _ = std::fs::remove_dir(&dir);
    }
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), StagingError> {
    if std::fs::symlink_metadata(src)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        let target = std::fs::read_link(src).map_err(|e| StagingError::io(src, e))?;
        #[cfg(unix)]
        {
            if dst.symlink_metadata().is_ok() {
                std::fs::remove_file(dst).map_err(|e| StagingError::io(dst, e))?;
            }
            std::os::unix::fs::symlink(&target, dst).map_err(|e| StagingError::io(dst, e))?;
        }
        #[cfg(not(unix))]
        {
            let resolved = std::fs::canonicalize(src).map_err(|e| StagingError::io(src, e))?;
            std::fs::copy(resolved, dst).map_err(|e| StagingError::io(dst, e))?;
        }
        return Ok(());
    }

    std::fs::copy(src, dst).map_err(|e| StagingError::io(dst, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(src)
            .map_err(|e| StagingError::io(src, e))?
            .mode();
        let mut perms = std::fs::metadata(dst)
            .map_err(|e| StagingError::io(dst, e))?
            .permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(dst, perms).map_err(|e| StagingError::io(dst, e))?;
    }
    Ok(())
}

fn git_add(root: &Path, paths: &[String]) -> Result<(), StagingError> {
    if paths.is_empty() {
        return Ok(());
    }
    for chunk in paths.chunks(50) {
        let mut args = vec!["add".to_string(), "-A".to_string(), "--".to_string()];
        args.extend(chunk.iter().cloned());
        GitCommand::new(root).args(args).run()?;
    }
    Ok(())
}

fn git_commit(root: &Path, message: &str) -> Result<(), StagingError> {
    let out = GitCommand::new(root)
        .args(["commit", "-m", message])
        .output()?;
    if !out.success() {
        return Err(StagingError::PublicCommit(out.stderr.trim().to_string()));
    }
    Ok(())
}

fn git_head(root: &Path) -> Result<Option<String>, StagingError> {
    let out = GitCommand::new(root).args(["rev-parse", "HEAD"]).output()?;
    if out.success() {
        let head = out.stdout_str().trim().to_string();
        if head.is_empty() {
            return Ok(None);
        }
        Ok(Some(head))
    } else {
        Ok(None)
    }
}

/// The private branch's tip. `root` is the private repository root.
fn private_tip(root: &Path, proj: &ProjFile) -> Option<String> {
    GitCommand::new(root)
        .args(["rev-parse", &proj.workflow.private.branch])
        .output()
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout_str().trim().to_string())
        .filter(|s| !s.is_empty() && s != "(null)")
}

fn record_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", now.as_secs(), now.subsec_nanos())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append one record to `.git/pm/staging.json`. `root` is the private repository root.
fn append_staging_record(root: &Path, record: &StagingRecord) -> Result<(), StagingError> {
    let git_dir = git_dir(root)?;
    let pm = git_dir.join("pm");
    std::fs::create_dir_all(&pm).map_err(|e| StagingError::io(&pm, e))?;
    let path = pm.join("staging.json");
    let mut records: Vec<StagingRecord> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    records.push(record.clone());
    if records.len() > 200 {
        records.drain(0..records.len() - 200);
    }
    let json = serde_json::to_string_pretty(&records).map_err(|e| StagingError::Io {
        path: path.clone(),
        source: std::io::Error::other(e),
    })?;
    std::fs::write(&path, json).map_err(|e| StagingError::io(&path, e))?;
    Ok(())
}

// =========================================================================
// Single-repo sync

/// A temporary worktree for the public branch.
fn temp_worktree(root: &Path, branch: &str) -> Result<PathBuf, StagingError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("pm-staging-{}-{stamp}", std::process::id()));
    let out = GitCommand::new(root)
        .args([
            "worktree",
            "add",
            "--detach",
            &dir.to_string_lossy(),
            branch,
        ])
        .output()?;
    if !out.success() {
        let msg = out.stderr.trim().to_string();
        return Err(StagingError::err_cmd(
            format!("git worktree add --detach {} {branch}", dir.display()),
            out.code,
            msg,
        ));
    }
    Ok(dir)
}

fn remove_worktree(root: &Path, dir: &Path) {
    let _ = GitCommand::new(root)
        .args(["worktree", "remove", "--force", &dir.to_string_lossy()])
        .output();
}

fn sync_single_repo(plan: &SyncPlan) -> Result<SyncOutcome, StagingError> {
    let root = plan.root;
    let proj = plan.proj;
    let changed = plan.changed;
    let removable = plan.removable;
    let skipped = plan.skipped;
    let message = plan.message.clone();
    let push = plan.push;
    let allowed = plan.allowed;
    let tmp = temp_worktree(root, &proj.workflow.public.branch)?;
    let result = sync_worktree(root, proj, changed, removable, message.as_deref(), &tmp);
    remove_worktree(root, &tmp);

    let (synced, committed) = result?;

    let public_head = {
        let out = GitCommand::new(root)
            .args(["rev-parse", "--verify", &proj.workflow.public.branch])
            .output()?;
        if out.success() {
            Some(out.stdout_str().trim().to_string())
        } else {
            None
        }
    };

    let mut warnings = Vec::new();
    let mut pushed = false;
    if push && committed {
        let branch = proj.workflow.public.branch.clone();
        let remote = proj.workflow.public.remote.clone();
        let out = GitCommand::new(root)
            .args(["push", &remote, &branch])
            .output()?;
        if out.success() {
            pushed = true;
        } else {
            let tail = out.stderr.trim().to_string();
            warnings.push(format!("push failed: {tail}"));
        }
    }

    let msg = message.unwrap_or_else(|| {
        format!(
            "Stage {} file{}",
            synced,
            if synced == 1 { "" } else { "s" }
        )
    });

    let record = StagingRecord {
        id: record_id(),
        at: unix_now(),
        message: msg.clone(),
        source: proj.workflow.private.branch.clone(),
        commit: private_tip(root, proj),
        files: changed
            .iter()
            .cloned()
            .chain(removable.iter().cloned())
            .collect(),
        allowed: allowed.to_vec(),
    };
    append_staging_record(root, &record)?;

    Ok(SyncOutcome {
        synced,
        skipped,
        message: if synced > 0 { Some(msg) } else { None },
        committed,
        head: public_head,
        pushed,
        warnings,
        blockers: Vec::new(),
        allowed: allowed.to_vec(),
    })
}

/// Write the changed private files into the worktree, commit, and point the public branch at
/// the new commit.
fn sync_worktree(
    root: &Path,
    proj: &ProjFile,
    changed: &BTreeSet<String>,
    removable: &BTreeSet<String>,
    message: Option<&str>,
    tmp: &Path,
) -> Result<(usize, bool), StagingError> {
    let branch = &proj.workflow.private.branch;
    let allowlist = Allowlist::for_proj(proj);
    let mut synced = 0usize;

    for f in changed {
        let worktree_file = tmp.join(f);
        let src = allowlist.source_of(f);
        if let Some(parent) = worktree_file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StagingError::io(parent, e))?;
        }

        // The git mode tells us whether this is a symlink or an executable.
        let mode_out = GitCommand::new(root)
            .args(["ls-tree", "-r", branch, "--", src])
            .output()?;
        let mode = String::from_utf8_lossy(&mode_out.stdout)
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string();

        let blob_out = GitCommand::new(root)
            .args(["cat-file", "blob", &format!("{branch}:{src}")])
            .output()?;
        if !blob_out.success() {
            return Err(StagingError::err_cmd(
                format!("git cat-file blob {branch}:{src}"),
                blob_out.code,
                blob_out.stderr.to_string(),
            ));
        }
        let bytes = blob_out.stdout;

        if worktree_file.symlink_metadata().is_ok() {
            std::fs::remove_file(&worktree_file)
                .map_err(|e| StagingError::io(&worktree_file, e))?;
        }

        if mode == "120000" {
            // A symlink in the git tree: the blob holds the target path.
            let target = String::from_utf8_lossy(&bytes).to_string();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &worktree_file)
                .map_err(|e| StagingError::io(&worktree_file, e))?;
            #[cfg(not(unix))]
            std::fs::write(&worktree_file, &bytes)
                .map_err(|e| StagingError::io(&worktree_file, e))?;
        } else {
            std::fs::write(&worktree_file, &bytes)
                .map_err(|e| StagingError::io(&worktree_file, e))?;
            if mode == "100755" {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&worktree_file)
                        .map_err(|e| StagingError::io(&worktree_file, e))?
                        .permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&worktree_file, perms)
                        .map_err(|e| StagingError::io(&worktree_file, e))?;
                }
            }
        }
        synced += 1;
    }

    for f in removable {
        let worktree_file = tmp.join(f);
        if worktree_file.symlink_metadata().is_ok() {
            std::fs::remove_file(&worktree_file)
                .map_err(|e| StagingError::io(&worktree_file, e))?;
            synced += 1;
        }
    }

    let mut paths: Vec<String> = changed.iter().cloned().collect();
    paths.extend(removable.iter().cloned());
    git_add(tmp, &paths)?;

    let diff = GitCommand::new(tmp)
        .args(["diff", "--cached", "--quiet"])
        .output()?;
    let changed_any = !diff.success();

    if changed_any {
        let msg = message.map(|m| m.to_string()).unwrap_or_else(|| {
            format!(
                "Stage {} file{}",
                synced,
                if synced == 1 { "" } else { "s" }
            )
        });
        git_commit(tmp, &msg)?;
        let head_out = GitCommand::new(tmp).args(["rev-parse", "HEAD"]).output()?;
        if head_out.success() {
            let new_head = head_out.stdout_str().trim().to_string();
            if !new_head.is_empty() {
                GitCommand::new(root)
                    .args([
                        "update-ref",
                        &format!("refs/heads/{}", proj.workflow.public.branch),
                        &new_head,
                    ])
                    .run()?;
            }
        }
    }

    remove_empty_parents(tmp, removable);

    Ok((synced, changed_any))
}

// =========================================================================
// Verify

/// Compare the public side against the allowlist and the private side.
pub fn verify(root: &Path) -> Result<VerifyReport, StagingError> {
    let proj = load_proj(root)?;
    let allowlist = Allowlist::for_proj(&proj);
    let private_repo = crate::proj::private_root(root, &proj)?;

    let listed: BTreeSet<String> = list_private_files(root, &proj)?.into_iter().collect();
    let public = list_public_files(root, &proj)?;

    let mut issues = Vec::new();

    for f in &public {
        if !allowlist.public_staged(f) {
            issues.push(format!("the public side has {f}, which is not allowlisted"));
        }
    }

    // The public paths the private side stages (mapped files under their target names).
    let mut staged: BTreeSet<String> = listed
        .iter()
        .filter(|f| allowlist.staged(f))
        .cloned()
        .collect();
    staged.extend(allowlist.mapped(&listed).cloned());

    let public_set: BTreeSet<&String> = public.iter().collect();
    for f in &staged {
        if !public_set.contains(f) {
            issues.push(format!("the public side is missing {f}"));
            continue;
        }
        let src = allowlist.source_of(f);
        let pf = if proj.workflow.mode == WorkflowMode::TwoRepo {
            let full = private_repo.join(src);
            if !full.exists() {
                String::new()
            } else {
                fingerprint_file(&full)?
            }
        } else {
            private_fingerprint(&private_repo, &proj, src)?
        };
        let gf = public_fingerprint(root, &proj, f)?;
        if !pf.is_empty() && !gf.is_empty() && pf != gf {
            issues.push(format!("{f} differs (private vs public)"));
        }
    }

    let ok = issues.is_empty();
    Ok(VerifyReport { ok, issues })
}

/// The commit the last sync covered: the private tip recorded in the newest record that names
/// one. `None` when there is no record, or no record names a commit (a fresh or unborn project).
///
/// `root` is the folder that holds the `.proj` file; the records live in the project's private
/// repository, not at the root itself.
pub fn graph_upto(root: &Path) -> Option<ObjectId> {
    let records = records(root).ok()?;
    let commit = records.iter().rev().find_map(|r| r.commit.as_ref())?;
    ObjectId::from_hex(commit.to_ascii_lowercase().as_bytes()).ok()
}

/// The staging records kept in `.git/pm/staging.json` (oldest first). `root` is the folder that
/// holds the `.proj` file; the records live in the project's private repository.
pub fn records(root: &Path) -> Result<Vec<StagingRecord>, StagingError> {
    let proj = load_proj(root)?;
    let private = crate::proj::private_root(root, &proj)?;
    let git_dir = git_dir(&private)?;
    let path = git_dir.join("pm").join("staging.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| StagingError::io(&path, e))?;
    serde_json::from_str(&text).map_err(|e| StagingError::Io {
        path: path.clone(),
        source: std::io::Error::other(e),
    })
}

/// Is the private side dirty (uncommitted work present)? Two-repo uses the working tree;
/// single-repo deliberately does not (the branch is the source).
pub fn private_is_dirty(root: &Path) -> Result<bool, StagingError> {
    let proj = load_proj(root)?;
    let private = crate::proj::private_root(root, &proj)?;
    let out = GitCommand::new(&private)
        .args(["status", "--porcelain", "--"])
        .output()?;
    Ok(out.success() && !out.stdout_str().trim().is_empty())
}

// =========================================================================
// Manual per-file overrides (saved to .proj)

/// Set or clear a per-file override in `.proj`. `kind` is "include", "exclude" or "clear".
pub fn set_override(root: &Path, path: &str, kind: &str) -> Result<(), StagingError> {
    let proj_path = crate::proj::find_proj_file(root)?;
    let mut doc = crate::proj::ProjDocument::load(&proj_path)?;

    let current: ProjFile = doc.to_proj()?;
    let mut include: Vec<String> = current.staging.overrides.include.clone();
    let mut exclude: Vec<String> = current.staging.overrides.exclude.clone();

    include.retain(|p| p != path);
    exclude.retain(|p| p != path);

    match kind {
        "include" => include.push(path.to_string()),
        "exclude" => exclude.push(path.to_string()),
        "clear" => {}
        other => {
            return Err(StagingError::Proj(crate::proj::ProjError::Invalid(vec![
                crate::proj::Issue {
                    key: "staging.overrides".to_string(),
                    message: format!("unknown override kind {other:?}"),
                },
            ])));
        }
    }

    include.sort();
    exclude.sort();

    doc.set_string_array(&["staging", "overrides", "include"], &include);
    doc.set_string_array(&["staging", "overrides", "exclude"], &exclude);
    doc.save(&proj_path)?;
    Ok(())
}

// =========================================================================
// Diffs

use crate::history::diff::{FileDiff, FileStatus, LARGE_FILE, MAX_FILE, hunks};

/// The private side of one file: its bytes, plus the mode git records.
fn private_side(
    root: &Path,
    proj: &ProjFile,
    path: &str,
) -> Result<(Option<Vec<u8>>, Option<String>), StagingError> {
    let private = crate::proj::private_root(root, proj)?;
    match proj.workflow.mode {
        WorkflowMode::TwoRepo => {
            let full = private.join(path);
            if !full.symlink_metadata().is_ok() {
                return Ok((None, None));
            }
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::MetadataExt;
                let md =
                    std::fs::symlink_metadata(&full).map_err(|e| StagingError::io(&full, e))?;
                if md.file_type().is_symlink() {
                    Some("120000".to_string())
                } else {
                    Some(format!("{:06o}", md.mode()))
                }
            };
            #[cfg(not(unix))]
            let mode = None;
            let bytes = read_file_through_link(&full)?;
            Ok((Some(bytes), mode))
        }
        WorkflowMode::SingleRepo => {
            let scope = format!("{}:{path}", proj.workflow.private.branch);
            let mode_out = GitCommand::new(&private)
                .args(["ls-tree", "-r", &proj.workflow.private.branch, "--", path])
                .output()?;
            let mode = String::from_utf8_lossy(&mode_out.stdout)
                .split_whitespace()
                .next()
                .filter(|m| !m.is_empty())
                .map(str::to_string);
            if mode.is_none() {
                return Ok((None, None));
            }
            let blob = GitCommand::new(&private)
                .args(["cat-file", "blob", &scope])
                .output()?;
            if !blob.success() {
                return Ok((None, None));
            }
            Ok((Some(blob.stdout), mode))
        }
    }
}

/// The public side of one file: its bytes, plus the mode git records.
fn public_side(
    root: &Path,
    proj: &ProjFile,
    path: &str,
) -> Result<(Option<Vec<u8>>, Option<String>), StagingError> {
    match proj.workflow.mode {
        WorkflowMode::TwoRepo => {
            let full = public_root(root, proj)?.join(path);
            if !full.symlink_metadata().is_ok() {
                return Ok((None, None));
            }
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::MetadataExt;
                let md =
                    std::fs::symlink_metadata(&full).map_err(|e| StagingError::io(&full, e))?;
                if md.file_type().is_symlink() {
                    Some("120000".to_string())
                } else {
                    Some(format!("{:06o}", md.mode()))
                }
            };
            #[cfg(not(unix))]
            let mode = None;
            let bytes = read_file_through_link(&full)?;
            Ok((Some(bytes), mode))
        }
        WorkflowMode::SingleRepo => {
            let scope = format!("{}:{path}", proj.workflow.public.branch);
            let mode_out = GitCommand::new(root)
                .args(["ls-tree", "-r", &proj.workflow.public.branch, "--", path])
                .output()?;
            let mode = String::from_utf8_lossy(&mode_out.stdout)
                .split_whitespace()
                .next()
                .filter(|m| !m.is_empty())
                .map(str::to_string);
            if mode.is_none() {
                return Ok((None, None));
            }
            let blob = GitCommand::new(root)
                .args(["cat-file", "blob", &scope])
                .output()?;
            if !blob.success() {
                return Ok((None, None));
            }
            Ok((Some(blob.stdout), mode))
        }
    }
}

/// The diff of one file between the private and public sides, in the same shape as a commit
/// file diff so the frontend renders it like one. `old` is the public side, `new` the private.
pub fn diff(root: &Path, path: &str, force: bool) -> Result<FileDiff, StagingError> {
    let proj = load_proj(root)?;
    let allowlist = Allowlist::for_proj(&proj);
    let (private, private_mode) = private_side(root, &proj, allowlist.source_of(path))?;
    let (public, public_mode) = public_side(root, &proj, path)?;

    if private.is_none() && public.is_none() {
        return Err(StagingError::NotFound(path.to_string()));
    }

    let status = match (&private, &public) {
        (Some(_), None) => FileStatus::Added,
        (None, Some(_)) => FileStatus::Deleted,
        _ => FileStatus::Modified,
    };

    let old_size = public.as_ref().map(|b| b.len() as u64);
    let new_size = private.as_ref().map(|b| b.len() as u64);
    let biggest = old_size.unwrap_or(0).max(new_size.unwrap_or(0));

    let mut out = FileDiff {
        path: path.to_string(),
        old_path: None,
        status,
        binary: false,
        too_large: false,
        old_size,
        new_size,
        old_mode: public_mode,
        new_mode: private_mode,
        hunks: Vec::new(),
    };

    if biggest > MAX_FILE || (biggest > LARGE_FILE && !force) {
        out.too_large = true;
        return Ok(out);
    }

    let old = public.as_deref().unwrap_or(&[]);
    let new = private.as_deref().unwrap_or(&[]);
    if is_binary(old) || is_binary(new) {
        out.binary = true;
        return Ok(out);
    }

    // Like a commit diff: an empty side stays out of the hunks, but context marks the
    // boundary.
    let old_text = String::from_utf8_lossy(old);
    let new_text = String::from_utf8_lossy(new);
    out.hunks = hunks(&old_text, &new_text, 3);
    Ok(out)
}

fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(8000)].contains(&0)
}

// =========================================================================
// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureRepo;

    const TWO_REPO_PROJ: &str = r#"schema_version = 1

[project]
name = "Example"
version = "1.0.0"

[workflow]
mode = "two-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "main"
path = "../public"
"#;

    const SINGLE_REPO_PROJ: &str = r#"schema_version = 1

[project]
name = "Example"
version = "1.0.0"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"
"#;

    fn with_proj(r: &FixtureRepo, text: &str) {
        r.write("example.proj", text);
        r.commit_all("add proj");
    }

    /// Private fixture repo plus its public sibling repo, as the .proj describes.
    fn two_repo_fixture() -> FixtureRepo {
        let private = FixtureRepo::new();
        let sibling = private.path().parent().unwrap().join("public");
        let out = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&sibling)
            .output()
            .unwrap();
        assert!(out.status.success(), "git init failed: {out:?}");
        with_proj(&private, TWO_REPO_PROJ);
        private
    }

    fn public_dir(private: &FixtureRepo) -> PathBuf {
        private.path().parent().unwrap().join("public")
    }

    fn public_git(public: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-C", public.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?} failed: {:?}", args, out);
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    // ---- allowlist (pure) ----

    #[test]
    fn allowlist_precedence_later_rule_wins() {
        let proj = ProjFile::parse(SINGLE_REPO_PROJ).unwrap();
        let allowlist = Allowlist::for_proj(&proj);
        assert!(
            allowlist.staged("src/main.rs"),
            "default: everything staged"
        );
        assert!(!allowlist.staged("docs/notes.md"), "default: .md excluded");
        assert!(allowlist.staged("README.md"), "default: README.md included");
        assert!(
            allowlist.staged("CHANGELOG.md"),
            "default: CHANGELOG.md included"
        );
        assert!(allowlist.staged("LICENSE"), "default: LICENSE included");
        assert!(!allowlist.staged("notes.md"), "default: other .md excluded");
    }

    #[test]
    fn allowlist_include_exclude_and_overrides() {
        let proj = ProjFile::parse(
            r#"schema_version = 1

[project]
name = "Example"
version = "1.0.0"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"

[staging]
include = ["src/**", "!src/vendor/**"]
exclude = ["**/scratch/**"]

[staging.overrides]
include = ["docs/extra.md"]
exclude = ["src/final.rs"]
"#,
        )
        .unwrap();
        let allowlist = Allowlist::for_proj(&proj);
        assert!(allowlist.staged("src/main.rs"), "include src/**");
        assert!(
            !allowlist.staged("src/vendor/x.rs"),
            "negation excludes vendor"
        );
        assert!(!allowlist.staged("src/scratch/x.rs"), "exclude glob wins");
        assert!(
            allowlist.staged("docs/extra.md"),
            "override include beats .md default"
        );
        assert!(!allowlist.staged("src/final.rs"), "override exclude wins");
        assert!(
            !allowlist.staged("docs/notes.md"),
            "plain .md still excluded"
        );
    }

    // ---- leak scan ----

    #[test]
    fn leak_scan_blocks_the_agreed_families() {
        let r = FixtureRepo::new();
        with_proj(&r, SINGLE_REPO_PROJ);
        let proj = load_proj(r.path()).unwrap();

        let cases: [(&str, &str, LeakKind); 6] = [
            (
                "secrets/key.txt",
                "token = ghp_abc123def456ghi789jkl012mno345pqr",
                LeakKind::ApiKey,
            ),
            (".env", "API_KEY=x\n", LeakKind::EnvFile),
            (
                "certs/server.txt",
                "-----BEGIN PRIVATE KEY-----\nxyz\n-----END PRIVATE KEY-----\n",
                LeakKind::PrivateKey,
            ),
            ("decompiled/lib.txt", "hello\n", LeakKind::DecompiledFolder),
            ("dlls/plugin.dll", "binary\n", LeakKind::DecompiledExtension),
            ("assets/pack.pak", "binary\n", LeakKind::DecompiledArchive),
        ];
        for (path, contents, kind) in cases {
            r.commit_file(path, contents, "add file");
            let issues = scan_tree_file(r.path(), &proj, path, &exempt_matchers(&proj)).unwrap();
            assert!(
                issues.iter().any(|i| i.kind == kind && i.blocking),
                "{path}: expected {kind:?} in {issues:?}"
            );
        }
    }

    #[test]
    fn leak_scan_detects_custom_regexes_and_size() {
        let r = FixtureRepo::new();
        with_proj(&r, SINGLE_REPO_PROJ);
        r.commit_file("custom.txt", "abc: DEADBEEF0123456789\n", "add custom");

        let mut proj = load_proj(r.path()).unwrap();
        proj.staging.leak.custom_regexes = vec!["DEADBEEF0123456789".to_string()];
        proj.staging.leak.warn_file_bytes = Some(8);
        proj.staging.leak.max_file_bytes = Some(4);
        let issues =
            scan_tree_file(r.path(), &proj, "custom.txt", &exempt_matchers(&proj)).unwrap();
        assert!(
            issues.iter().any(|i| i.kind == LeakKind::CustomRegex),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.kind == LeakKind::Warn && !i.blocking),
            "{issues:?}"
        );
        assert!(
            issues
                .iter()
                .any(|i| i.kind == LeakKind::Oversize && i.blocking),
            "{issues:?}"
        );
    }

    #[test]
    fn leak_scan_exempts_allow_globs() {
        let r = FixtureRepo::new();
        with_proj(&r, SINGLE_REPO_PROJ);
        let mut proj = load_proj(r.path()).unwrap();
        proj.staging.leak.allow = vec!["docs/**".to_string()];
        r.commit_file(
            "docs/api.txt",
            "token = ghp_abc123def456ghi789jkl012mno345pqr\n",
            "add doc",
        );

        let issues =
            scan_tree_file(r.path(), &proj, "docs/api.txt", &exempt_matchers(&proj)).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
    }

    // ---- preview ----

    #[test]
    fn preview_reports_status_and_issues() {
        let r = FixtureRepo::new();
        with_proj(&r, SINGLE_REPO_PROJ);
        r.commit_file("src/main.rs", "fn main() {}\n", "add main");
        r.branch("public"); // public branch == main tree, so this is a no-op state
        r.commit_file("src/notes.txt", "private notes\n", "add notes");

        let preview = preview(r.path()).unwrap();
        let main_file = preview
            .files
            .iter()
            .find(|f| f.path == "src/main.rs")
            .unwrap();
        assert_eq!(
            main_file.status,
            StageFileStatus::Unchanged,
            "{main_file:?}"
        );
        let notes = preview
            .files
            .iter()
            .find(|f| f.path == "src/notes.txt")
            .unwrap();
        assert_eq!(notes.status, StageFileStatus::Added, "{notes:?}");
        // .md files are excluded by the default allowlist.
        assert!(!preview.files.iter().any(|f| f.path == "docs/notes.md"));
    }

    #[test]
    fn two_repo_preview_reports_added_changed_removed() {
        let private = two_repo_fixture();
        private.commit_file("src/main.rs", "fn main() {}\n", "add main");

        let public = public_dir(&private);
        std::fs::create_dir_all(public.join("src")).unwrap();
        std::fs::write(public.join("src/main.rs"), "fn main() { /* changed */ }\n").unwrap();
        std::fs::write(public.join("public-only.txt"), "public side\n").unwrap();
        public_git(&public, &["add", "-A"]);
        public_git(
            &public,
            &[
                "-c",
                "core.autocrlf=false",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "user.name=t",
                "commit",
                "-q",
                "-m",
                "public",
            ],
        );

        let preview = preview(private.path()).unwrap();
        let changed = preview
            .files
            .iter()
            .find(|f| f.path == "src/main.rs")
            .unwrap();
        assert_eq!(changed.status, StageFileStatus::Changed, "{changed:?}");
        let removed = preview
            .files
            .iter()
            .find(|f| f.path == "public-only.txt")
            .unwrap();
        assert_eq!(removed.status, StageFileStatus::Removed, "{removed:?}");
    }

    // ---- sync: two-repo ----

    #[test]
    fn sync_two_repo_commits_and_records_and_returns_warnings() {
        let private = two_repo_fixture();
        private.commit_file("src/main.rs", "fn main() {}\n", "add main");
        private.commit_file(
            "src/notes.txt",
            "several lines\nof private content\n",
            "add notes",
        );
        private.commit_file(
            "secret.txt",
            "token = ghp_abc123def456ghi789jkl012mno345pqr\n",
            "add secret",
        );

        let out = sync(private.path(), &["secret.txt".to_string()], None, false).unwrap();
        assert!(out.committed, "{out:?}");
        assert_eq!(out.synced, 4, "{out:?}");
        assert!(out.blockers.is_empty(), "{out:?}");
        assert_eq!(out.allowed, vec!["secret.txt".to_string()]);

        let public = public_dir(&private);
        assert!(public.join("src/main.rs").exists(), "public has main.rs");
        assert!(public.join("secret.txt").exists(), "public has secret.txt");

        let records = records(private.path()).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        let mut files: Vec<String> = records[0].files.to_vec();
        files.sort();
        assert_eq!(
            files,
            vec![
                "example.proj".to_string(),
                "secret.txt".to_string(),
                "src/main.rs".to_string(),
                "src/notes.txt".to_string()
            ]
        );
        assert_eq!(records[0].allowed, vec!["secret.txt".to_string()]);
        assert!(records[0].commit.is_some(), "{:?}", records[0]);
    }

    #[test]
    fn sync_refuses_on_blocking_leaks() {
        let private = two_repo_fixture();
        private.commit_file(
            "secret.txt",
            "token = ghp_abc123def456ghi789jkl012mno345pqr\n",
            "add secret",
        );

        let out = sync(private.path(), &[], None, false).unwrap();
        assert!(out.blockers.contains(&"secret.txt".to_string()), "{out:?}");
        assert_eq!(out.synced, 0, "{out:?}");

        let out = sync(private.path(), &["secret.txt".to_string()], None, false).unwrap();
        assert_eq!(out.synced, 2, "{out:?}"); // example.proj + secret.txt
        assert!(out.blockers.is_empty(), "{out:?}");
        let public = public_dir(&private);
        assert!(public.join("secret.txt").exists());
    }

    // ---- sync: single-repo ----

    #[test]
    fn sync_single_repo_updates_the_public_branch() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        private.checkout_new("public");
        private.checkout("main");
        private.commit_file("src/main.rs", "fn main() {}\n", "add main");
        private.commit_file("docs/notes.md", "# Notes\n", "add notes");

        let out = sync(private.path(), &[], None, false).unwrap();
        assert!(out.committed, "{out:?}");

        let files = private.git(&["ls-tree", "--name-only", "-r", "public"]);
        assert!(files.contains("src/main.rs"), "{files}");
        assert!(files.contains("example.proj"), "{files}");
        // .md files are excluded by the default allowlist.
        assert!(!files.contains("docs/notes.md"), "{files}");

        // The user's working tree is untouched.
        assert!(private.path().join("src/main.rs").exists());
    }

    #[test]
    fn sync_single_repo_leaves_uncommitted_private_work_alone() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        private.checkout_new("public");
        private.checkout("main");
        private.commit_file("src/committed.rs", "fn committed() {}\n", "add committed");
        private.write("src/uncommitted.rs", "fn uncommitted() {}\n");

        let out = sync(private.path(), &[], None, false).unwrap();
        assert!(out.committed, "{out:?}");

        let files = private.git(&["ls-tree", "--name-only", "-r", "public"]);
        assert!(files.contains("src/committed.rs"), "{files}");
        assert!(!files.contains("src/uncommitted.rs"), "{files}");
    }

    #[test]
    fn verify_is_clean_after_a_sync() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        private.checkout_new("public");
        private.checkout("main");
        private.commit_file("src/main.rs", "fn main() {}\n", "add main");

        let out = sync(private.path(), &[], None, false).unwrap();
        assert!(out.committed, "{out:?}");

        let report = verify(private.path()).unwrap();
        assert!(report.ok, "{report:?}");
        assert!(report.issues.is_empty(), "{report:?}");
    }

    // ---- [staging.map] ----

    const MAP_STAGING: &str = r#"
[staging]
include = ["!**", "src/**"]

[staging.map]
"publish/Makefile" = "Makefile"
"#;

    #[test]
    fn map_stages_a_file_under_its_public_name_and_keeps_the_private_one() {
        let private = two_repo_fixture();
        with_proj(&private, &format!("{TWO_REPO_PROJ}{MAP_STAGING}"));
        private.commit_file("src/main.rs", "fn main() {}\n", "add main");
        private.commit_file("Makefile", "private:\n", "private makefile");
        private.commit_file("publish/Makefile", "public:\n", "public makefile");

        let p = preview(private.path()).unwrap();
        let paths: Vec<&str> = p.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["Makefile", "src/main.rs"], "{p:?}");

        let out = sync(private.path(), &[], None, false).unwrap();
        assert!(out.committed, "{out:?}");
        let public = public_dir(&private);
        assert_eq!(
            std::fs::read_to_string(public.join("Makefile")).unwrap(),
            "public:\n"
        );
        assert!(!public.join("publish").exists());

        // Now in step: unchanged, verify clean, and the diff reads the mapped file.
        let p = preview(private.path()).unwrap();
        assert!(
            p.files
                .iter()
                .all(|f| f.status == StageFileStatus::Unchanged),
            "{p:?}"
        );
        assert!(verify(private.path()).unwrap().ok);
        private.write("publish/Makefile", "public:\n\techo hi\n");
        let p = preview(private.path()).unwrap();
        let mk = p.files.iter().find(|f| f.path == "Makefile").unwrap();
        assert_eq!(mk.status, StageFileStatus::Changed);
        let d = diff(private.path(), "Makefile", false).unwrap();
        assert_eq!(d.new_size, Some("public:\n\techo hi\n".len() as u64));
        assert!(Allowlist::for_proj(&load_proj(private.path()).unwrap()).public_staged("Makefile"));
    }

    #[test]
    fn map_works_in_single_repo_mode() {
        let private = FixtureRepo::new();
        with_proj(&private, &format!("{SINGLE_REPO_PROJ}{MAP_STAGING}"));
        private.checkout_new("public");
        private.checkout("main");
        private.commit_file("publish/Makefile", "public:\n", "public makefile");

        let out = sync(private.path(), &[], None, false).unwrap();
        assert!(out.committed, "{out:?}");
        assert_eq!(private.git(&["show", "public:Makefile"]).trim(), "public:");
        // The public branch was forked after example.proj was committed; only that is extra.
        let report = verify(private.path()).unwrap();
        assert_eq!(
            report.issues,
            vec!["the public side has example.proj, which is not allowlisted".to_string()]
        );
    }

    // ---- overrides & symlinks ----

    #[test]
    fn set_override_round_trip_and_clear() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        private.commit_file("docs/notes.md", "# Notes\n", "add");

        set_override(private.path(), "docs/notes.md", "include").unwrap();
        let proj = load_proj(private.path()).unwrap();
        assert_eq!(
            proj.staging.overrides.include,
            vec!["docs/notes.md".to_string()]
        );
        let preview = preview(private.path()).unwrap();
        assert!(
            preview
                .files
                .iter()
                .any(|f| f.path == "docs/notes.md" && f.status == StageFileStatus::Added),
            "{preview:?}"
        );

        set_override(private.path(), "docs/notes.md", "clear").unwrap();
        let proj = load_proj(private.path()).unwrap();
        assert!(proj.staging.overrides.include.is_empty(), "{proj:?}");
    }

    #[test]
    fn symlink_escape_blocks_until_allowed() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        private.checkout_new("public");
        private.checkout("main");
        private.commit_file("src/real.txt", "content\n", "add real");
        std::os::unix::fs::symlink("/etc/hostname", private.path().join("link-out.txt")).unwrap();
        private.git(&["add", "link-out.txt"]);
        private.git(&["commit", "-q", "--allow-empty", "-m", "add link"]);

        let preview = preview(private.path()).unwrap();
        let link = preview
            .files
            .iter()
            .find(|f| f.path == "link-out.txt")
            .unwrap();
        assert!(
            link.issues
                .iter()
                .any(|i| i.kind == LeakKind::SymlinkEscape && i.blocking),
            "{link:?}"
        );

        let out = sync(private.path(), &[], None, false).unwrap();
        assert!(
            out.blockers.contains(&"link-out.txt".to_string()),
            "{out:?}"
        );

        let out = sync(private.path(), &["link-out.txt".to_string()], None, false).unwrap();
        assert_eq!(out.synced, 2, "{out:?}");
    }

    #[test]
    fn diff_for_two_repo_changed_file() {
        let private = two_repo_fixture();
        private.commit_file(
            "src/main.rs",
            "fn main() {\n    // private\n}\n",
            "add main",
        );
        let public = public_dir(&private);
        std::fs::create_dir_all(public.join("src")).unwrap();
        let public_src = "fn main() {\n    // public\n}\n";
        std::fs::write(public.join("src/main.rs"), public_src).unwrap();

        let out = diff(private.path(), "src/main.rs", false).unwrap();
        assert_eq!(out.status, FileStatus::Modified);
        assert_eq!(out.old_size, Some(public_src.len() as u64));
        assert_eq!(
            out.new_size,
            Some("fn main() {\n    // private\n}\n".len() as u64)
        );
        assert!(!out.binary);
        assert!(!out.too_large);
        let deleted: Vec<&str> = out
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| l.kind == crate::history::diff::LineKind::Deleted)
            .map(|l| l.text.as_str())
            .collect();
        let added: Vec<&str> = out
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| l.kind == crate::history::diff::LineKind::Added)
            .map(|l| l.text.as_str())
            .collect();
        assert!(
            deleted.iter().any(|t| t.contains("public")),
            "{:?}",
            out.hunks
        );
        assert!(
            added.iter().any(|t| t.contains("private")),
            "{:?}",
            out.hunks
        );
    }

    #[test]
    fn diff_for_single_repo_added_and_removed() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        // src/old.txt is on the public branch only: it was committed before the
        // public branch was cut, and removed from the private branch afterwards.
        private.commit_file("src/old.txt", "old file\n", "add old");
        private.branch("public");
        private.commit_file("src/notes.txt", "private notes\n", "add notes");

        let added = diff(private.path(), "src/notes.txt", false).unwrap();
        assert_eq!(added.status, FileStatus::Added);
        assert_eq!(added.old_size, None);
        assert!(added.new_size.unwrap() > 0);

        // src/old.txt is unchanged on both sides: an empty hunk list, not an error.
        let same = diff(private.path(), "src/old.txt", false).unwrap();
        assert_eq!(same.status, FileStatus::Modified);
        assert!(same.hunks.is_empty());

        private.remove("src/old.txt");
        private.commit_all("drop old");
        let removed = diff(private.path(), "src/old.txt", false).unwrap();
        assert_eq!(removed.status, FileStatus::Deleted);
        assert_eq!(removed.new_size, None);
    }

    #[test]
    fn diff_for_a_missing_file_is_not_found() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        private.commit_file("src/main.rs", "fn main() {}\n", "add main");
        private.branch("public");
        let err = diff(private.path(), "src/gone.txt", false).unwrap_err();
        assert!(matches!(err, StagingError::NotFound(_)), "{err:?}");
    }

    #[test]
    fn graph_upto_returns_the_last_record_commit() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        private.commit_file("src/main.rs", "fn main() {}\n", "add main");
        private.branch("public");

        assert!(graph_upto(private.path()).is_none(), "no record yet");

        sync(private.path(), &[], None, false).unwrap();

        let records = super::records(private.path()).unwrap();
        assert_eq!(records.len(), 1, "{records:?}");
        let tip = private.rev("main");
        assert_eq!(
            graph_upto(private.path()).unwrap().to_string(),
            tip,
            "the record names the private branch tip"
        );
    }

    #[test]
    fn private_is_dirty_reflects_the_working_tree() {
        let private = FixtureRepo::new();
        with_proj(&private, SINGLE_REPO_PROJ);
        private.commit_file("src/main.rs", "fn main() {}\n", "add main");
        assert!(!private_is_dirty(private.path()).unwrap());
        private.write("src/uncommitted.rs", "fn uncommitted() {}\n");
        assert!(private_is_dirty(private.path()).unwrap());
    }
}
