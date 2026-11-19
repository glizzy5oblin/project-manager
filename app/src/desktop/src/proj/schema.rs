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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    /// Files whose version string is bumped together with `project.version`.
    #[serde(default)]
    pub version_files: Vec<String>,
    /// Release zip name. Placeholders: `{name}`, `{version}`.
    #[serde(default = "default_asset_name")]
    pub asset_name: String,
    #[serde(default)]
    pub targets: Vec<ReleaseTarget>,
}

impl Default for Release {
    fn default() -> Self {
        Release {
            version_files: Vec::new(),
            asset_name: default_asset_name(),
            targets: Vec::new(),
        }
    }
}

pub(crate) const ASSET_PLACEHOLDERS: &[&str] = &["name", "version"];

fn default_asset_name() -> String {
    "{name}-{version}.zip".to_string()
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
