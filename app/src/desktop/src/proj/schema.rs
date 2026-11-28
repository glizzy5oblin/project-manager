//! Typed `.proj` schema. Unknown keys are rejected so typos (e.g. `exlude`) surface as errors
//! instead of being silently ignored.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjFile {
    pub schema_version: u32,
    pub project: Project,
    pub workflow: Workflow,
    #[serde(default)]
    pub staging: Staging,
    #[serde(default)]
    pub release: Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    /// Semantic version, `x.y.z`.
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowMode {
    /// A private repo plus a separate public repo.
    TwoRepo,
    /// One repo with a private branch and a public branch.
    SingleRepo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub mode: WorkflowMode,
    pub private: RepoSide,
    pub public: RepoSide,
}

/// One side (private or public) of the workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoSide {
    /// Repository folder, relative to the `.proj` file. Required for the public side in two-repo
    /// mode; must be omitted (or equal the private path) in single-repo mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub branch: String,
    #[serde(default = "default_remote")]
    pub remote: String,
}

fn default_remote() -> String {
    "origin".to_string()
}

/// Which private files are copied to the public side. Precedence (later wins): built-in defaults,
/// then `include`/`exclude`, then `overrides`. An `include` entry starting with `!` excludes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Staging {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub overrides: Overrides,
    /// Files published under another name: private path -> public path. The public path is
    /// managed from the mapped file alone, and the private file is staged only under the mapped
    /// name (e.g. `"publish/Makefile" = "Makefile"` keeps the private Makefile private).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub map: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub leak: Leak,
}

/// The leak scan: which files may not be staged as-is, and which paths are exempt.
///
/// Blockers require an explicit `allow` (a project-level exemption in `.proj`, or a one-time
/// per-file allow in the sync call); warnings do not block. Built-in defaults cover the agreed
/// rule families; every list is configurable per project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Leak {
    /// Decompiled-code folder names, any path segment matching one of these blocks (case-insensitive).
    #[serde(default = "default_leak_folders")]
    pub folders: Vec<String>,
    /// Decompiled-code file extensions (without a dot), e.g. `dll` for `lib.dll`.
    #[serde(default = "default_leak_extensions")]
    pub extensions: Vec<String>,
    /// Packed asset archives (extensions without a dot), e.g. `pak` for `assets.pak`.
    #[serde(default = "default_leak_archives")]
    pub archives: Vec<String>,
    /// Extra regexes matched against file contents. A match blocks.
    #[serde(default)]
    pub custom_regexes: Vec<String>,
    /// Globs exempting files from the whole scan (keys, env files, decompiled code, size).
    #[serde(default)]
    pub allow: Vec<String>,
    /// Files larger than this are blocked, unless they are LFS-tracked. Defaults to 100 MB.
    #[serde(default)]
    pub max_file_bytes: Option<u64>,
    /// Files larger than this are warned about (they still stage). Defaults to 50 MB.
    #[serde(default)]
    pub warn_file_bytes: Option<u64>,
}

impl Default for Leak {
    fn default() -> Self {
        Leak {
            folders: default_leak_folders(),
            extensions: default_leak_extensions(),
            archives: default_leak_archives(),
            custom_regexes: Vec::new(),
            allow: Vec::new(),
            max_file_bytes: None,
            warn_file_bytes: None,
        }
    }
}

pub const DEFAULT_MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
pub const DEFAULT_WARN_FILE_BYTES: u64 = 50 * 1024 * 1024;

fn default_leak_folders() -> Vec<String> {
    [
        "decompiled",
        "dump",
        "ilspy",
        "ghidra",
        "cpp2il",
        "il2cppdumper",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_leak_extensions() -> Vec<String> {
    ["dll", "exe", "so", "il", "idb"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn default_leak_archives() -> Vec<String> {
    ["pak", "bundle", "unity3d", "assets", "vpk"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Manual per-file choices layered on top of the globs. Entries are exact paths or globs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// One recorded release. Added to `[[release.history]]` when a tag is created; nothing else
/// writes it. The `date` is the tag's author date (the release date).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ReleaseHistoryEntry {
    /// The tag name, e.g. `v1.2.3`.
    pub tag: String,
    /// The version in the tag (without the `v`).
    pub version: String,
    /// The tag's author date (`YYYY-MM-DD`).
    pub date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    /// Files whose version string is bumped together with `project.version`.
    #[serde(default)]
    pub version_files: Vec<String>,
    /// Release zip name. Placeholders: `{name}`, `{version}`.
    #[serde(default = "default_asset_name")]
    pub asset_name: String,
    /// Conventional-commit prefixes the release notes group by, in display order
    /// (e.g. `["feat", "fix", "docs"]`). The default is the usual short list; a commit that
    /// matches none of them lands in an "Other changes" group.
    #[serde(default = "default_changelog_groups")]
    pub changelog_groups: Vec<String>,
    #[serde(default)]
    pub targets: Vec<ReleaseTarget>,
    /// The recorded releases, oldest first. One entry per tag; added by `create_tag`.
    #[serde(default)]
    pub history: Vec<ReleaseHistoryEntry>,
}

impl Default for Release {
    fn default() -> Self {
        Release {
            version_files: Vec::new(),
            asset_name: default_asset_name(),
            changelog_groups: default_changelog_groups(),
            targets: Vec::new(),
            history: Vec::new(),
        }
    }
}

pub(crate) const ASSET_PLACEHOLDERS: &[&str] = &["name", "version"];

fn default_asset_name() -> String {
    "{name}-{version}.zip".to_string()
}

fn default_changelog_groups() -> Vec<String> {
    [
        "feat", "fix", "perf", "docs", "refactor", "test", "build", "ci", "chore",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTarget {
    pub kind: TargetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetKind {
    Github,
}

impl ProjFile {
    /// Release asset file name with placeholders filled in.
    pub fn asset_file_name(&self) -> String {
        self.release
            .asset_name
            .replace("{name}", &self.project.name)
            .replace("{version}", &self.project.version)
    }

    /// Commented starter file for a new project.
    pub fn template(name: &str, mode: WorkflowMode) -> String {
        let name = toml_edit::Value::from(name).to_string();
        let workflow = match mode {
            WorkflowMode::TwoRepo => {
                "mode = \"two-repo\"\n\n\
                 [workflow.private]\n\
                 path = \".\"\n\
                 branch = \"main\"\n\
                 remote = \"origin\"\n\n\
                 [workflow.public]\n\
                 # Folder of the separate public repo, relative to this file.\n\
                 path = \"../public\"\n\
                 branch = \"main\"\n\
                 remote = \"origin\"\n"
            }
            WorkflowMode::SingleRepo => {
                "mode = \"single-repo\"\n\n\
                 [workflow.private]\n\
                 branch = \"main\"\n\
                 remote = \"origin\"\n\n\
                 [workflow.public]\n\
                 # Branch that receives only the staged files.\n\
                 branch = \"public\"\n\
                 remote = \"origin\"\n"
            }
        };
        format!(
            "# Project Manager project file. Reference: docs/proj-format.md\n\
             schema_version = 1\n\n\
             [project]\n\
             name = {name}\n\
             version = \"0.0.1\"\n\n\
             [workflow]\n\
             # \"two-repo\": separate private and public repos. \"single-repo\": private and public branches.\n\
             {workflow}\n\
             [staging]\n\
             # Globs of private files copied to public. A leading \"!\" excludes.\n\
             include = [\"README.md\", \"LICENSE\"]\n\
             exclude = []\n\n\
             [release]\n\
             version_files = []\n\
             asset_name = \"{{name}}-{{version}}.zip\"\n\n\
             [[release.targets]]\n\
             kind = \"github\"\n"
        )
    }
}
