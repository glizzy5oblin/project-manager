use super::*;

const TWO_REPO: &str = r#"# Example project
schema_version = 1

[project]
name = "ExampleMod"   # display name
version = "0.0.1"

[workflow]
mode = "two-repo"

[workflow.private]
path = "."
branch = "main"
remote = "origin"

[workflow.public]
path = "../ExampleMod-public"
branch = "main"

[staging]
# what goes public
include = ["src/**", "README.md", "LICENSE", "!src/secret/**"]
exclude = ["secrets/**", "**/*.dump"]
overrides = { include = [], exclude = [] }

[release]
version_files = ["manifest.json"]
asset_name = "{name}-{version}.zip"

[[release.targets]]
kind = "github"
"#;

const SINGLE_REPO: &str = r#"schema_version = 1

[project]
name = "Solo"
version = "1.2.3-beta.1"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"
"#;

fn issues(text: &str) -> Vec<Issue> {
    match ProjFile::parse(text) {
        Err(ProjError::Invalid(issues)) => issues,
        other => panic!("expected validation issues, got {other:?}"),
    }
}

fn has_issue(text: &str, key: &str) -> bool {
    issues(text).iter().any(|i| i.key == key)
}

#[test]
fn parses_two_repo_mode() {
    let p = ProjFile::parse(TWO_REPO).unwrap();
    assert_eq!(p.workflow.mode, WorkflowMode::TwoRepo);
    assert_eq!(
        p.workflow.public.path.as_deref(),
        Some("../ExampleMod-public")
    );
    assert_eq!(
        p.workflow.public.remote, "origin",
        "remote defaults to origin"
    );
    assert_eq!(p.staging.include.len(), 4);
    assert_eq!(p.release.targets[0].kind, TargetKind::Github);
    assert_eq!(p.asset_file_name(), "ExampleMod-0.0.1.zip");
}

#[test]
fn parses_single_repo_mode_with_defaults() {
    let p = ProjFile::parse(SINGLE_REPO).unwrap();
    assert_eq!(p.workflow.mode, WorkflowMode::SingleRepo);
    assert_eq!(p.workflow.public.branch, "public");
    assert!(p.staging.include.is_empty());
    assert_eq!(p.release.asset_name, "{name}-{version}.zip");
    assert!(p.release.targets.is_empty());
}

#[test]
fn templates_are_valid_for_both_modes() {
    for mode in [WorkflowMode::TwoRepo, WorkflowMode::SingleRepo] {
        let text = ProjFile::template("My \"Quoted\" Mod", mode);
        let p = ProjFile::parse(&text).unwrap_or_else(|e| panic!("{mode:?}: {e}\n{text}"));
        assert_eq!(p.project.name, "My \"Quoted\" Mod");
        assert_eq!(p.workflow.mode, mode);
    }
}

#[test]
fn round_trip_without_edits_is_byte_identical() {
    for text in [TWO_REPO, SINGLE_REPO] {
        let doc = ProjDocument::parse(text).unwrap();
        assert_eq!(doc.to_string(), text);
    }
}

#[test]
fn edits_preserve_comments_and_layout() {
    let mut doc = ProjDocument::parse(TWO_REPO).unwrap();
    doc.set_version("0.1.0").unwrap();
    doc.set_string_array(&["staging", "exclude"], &["secrets/**"]);
    let out = doc.to_string();

    assert!(out.contains("# Example project"));
    assert!(out.contains("# what goes public"));
    assert!(out.contains("name = \"ExampleMod\"   # display name"));
    assert!(out.contains("version = \"0.1.0\""));
    assert!(out.contains("exclude = [\"secrets/**\"]"));
    // Only the two edited lines differ.
    let changed = TWO_REPO
        .lines()
        .zip(out.lines())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(changed, 2, "{out}");

    let p = doc.to_proj().unwrap();
    assert_eq!(p.project.version, "0.1.0");
    assert_eq!(p.staging.exclude, vec!["secrets/**"]);
}

#[test]
fn set_creates_missing_tables() {
    let mut doc = ProjDocument::parse(SINGLE_REPO).unwrap();
    doc.set_string_array(&["release", "version_files"], &["mod.json"]);
    doc.set_string_array(&["staging", "overrides", "exclude"], &["notes.txt"]);
    let p = doc.to_proj().unwrap();
    assert_eq!(p.release.version_files, vec!["mod.json"]);
    assert_eq!(p.staging.overrides.exclude, vec!["notes.txt"]);
}

#[test]
fn remove_deletes_keys() {
    let mut doc = ProjDocument::parse(TWO_REPO).unwrap();
    assert!(doc.remove(&["staging", "overrides"]));
    assert!(!doc.remove(&["staging", "overrides"]));
    assert!(!doc.remove(&["nope", "nothing"]));
    assert_eq!(doc.to_proj().unwrap().staging.overrides, Default::default());
}

#[test]
fn set_version_rejects_non_semver() {
    let mut doc = ProjDocument::parse(TWO_REPO).unwrap();
    assert!(matches!(doc.set_version("1.0"), Err(ProjError::Invalid(_))));
    assert!(doc.to_string().contains("version = \"0.0.1\""));
}

#[test]
fn save_refuses_invalid_documents_and_writes_valid_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.proj");
    std::fs::write(&path, TWO_REPO).unwrap();

    let mut doc = ProjDocument::load(&path).unwrap();
    doc.set_string(&["workflow", "mode"], "single-repo"); // public path now invalid
    assert!(doc.save(&path).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        TWO_REPO,
        "file untouched"
    );

    let mut doc = ProjDocument::load(&path).unwrap();
    doc.set_version("2.0.0").unwrap();
    doc.save(&path).unwrap();
    assert_eq!(ProjFile::load(&path).unwrap().project.version, "2.0.0");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temp files left behind");
}

#[test]
fn syntax_errors_report_line_and_column() {
    let text = "schema_version = 1\n[project]\nname = \"x\nversion = \"1.0.0\"\n";
    match ProjFile::parse(text) {
        Err(ProjError::Parse { line, column, .. }) => {
            assert_eq!(line, Some(3));
            assert!(column.is_some());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn wrong_types_are_parse_errors_with_location() {
    let text = TWO_REPO.replace("version = \"0.0.1\"", "version = 1");
    match ProjFile::parse(&text) {
        Err(ProjError::Parse {
            line: Some(6),
            message,
            ..
        }) => assert!(message.contains("string"), "{message}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_fields_are_rejected_by_name() {
    let text = TWO_REPO.replace("exclude = [\"secrets", "exlude = [\"secrets");
    let err = ProjFile::parse(&text).unwrap_err().to_string();
    assert!(err.contains("exlude"), "{err}");

    let text = format!("{SINGLE_REPO}\n[extras]\nfoo = 1\n");
    let err = ProjFile::parse(&text).unwrap_err().to_string();
    assert!(err.contains("extras"), "{err}");
}

#[test]
fn unknown_enum_values_are_rejected() {
    let text = SINGLE_REPO.replace("single-repo", "three-repo");
    let err = ProjFile::parse(&text).unwrap_err().to_string();
    assert!(err.contains("three-repo"), "{err}");
}

#[test]
fn missing_required_sections_are_reported() {
    let err = ProjFile::parse("schema_version = 1\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("project"), "{err}");
}

#[test]
fn schema_version_checks() {
    let no_version = SINGLE_REPO.replace("schema_version = 1\n", "");
    assert!(
        ProjFile::parse(&no_version)
            .unwrap_err()
            .to_string()
            .contains("schema_version")
    );

    let zero = SINGLE_REPO.replace("schema_version = 1", "schema_version = 0");
    assert!(matches!(
        ProjFile::parse(&zero),
        Err(ProjError::Parse { .. })
    ));

    let future = SINGLE_REPO.replace("schema_version = 1", "schema_version = 99");
    assert!(matches!(
        ProjFile::parse(&future),
        Err(ProjError::UnsupportedSchema {
            found: 99,
            supported: 1
        })
    ));

    let mut doc = ProjDocument::parse(SINGLE_REPO).unwrap();
    assert!(
        !migrate(&mut doc).unwrap(),
        "current version needs no migration"
    );
}

#[test]
fn two_repo_requires_distinct_public_path() {
    let no_path = TWO_REPO.replace("path = \"../ExampleMod-public\"\n", "");
    assert!(has_issue(&no_path, "workflow.public.path"));
    let same = TWO_REPO.replace("../ExampleMod-public", "./");
    assert!(has_issue(&same, "workflow.public.path"));
}

#[test]
fn single_repo_rules() {
    let same_branch = SINGLE_REPO.replace("branch = \"public\"", "branch = \"main\"");
    assert!(has_issue(&same_branch, "workflow.public.branch"));
    let other_path = SINGLE_REPO.replace(
        "branch = \"public\"",
        "branch = \"public\"\npath = \"../elsewhere\"",
    );
    assert!(has_issue(&other_path, "workflow.public.path"));
    let same_path = SINGLE_REPO.replace("branch = \"public\"", "branch = \"public\"\npath = \".\"");
    ProjFile::parse(&same_path).unwrap();
}

#[test]
fn field_level_validation_collects_every_issue() {
    let text = TWO_REPO
        .replace("name = \"ExampleMod\"", "name = \"\"")
        .replace("version = \"0.0.1\"", "version = \"v1\"")
        .replace(
            "branch = \"main\"\nremote",
            "branch = \"bad branch\"\nremote",
        )
        .replace("\"secrets/**\"", "\"!secrets/**\"")
        .replace("\"**/*.dump\"", "\"../outside/**\"")
        .replace("\"manifest.json\"", "\"/abs/manifest.json\"")
        .replace("{name}-{version}.zip", "{name}-{date}.zip");
    let keys: Vec<String> = issues(&text).into_iter().map(|i| i.key).collect();
    for expected in [
        "project.name",
        "project.version",
        "workflow.private.branch",
        "staging.exclude[0]",
        "staging.exclude[1]",
        "release.version_files[0]",
        "release.asset_name",
    ] {
        assert!(
            keys.iter().any(|k| k == expected),
            "missing {expected} in {keys:?}"
        );
    }
}

#[test]
fn invalid_globs_are_reported() {
    let text = TWO_REPO.replace("\"src/**\"", "\"src/[unclosed\"");
    assert!(has_issue(&text, "staging.include[0]"));
}

#[test]
fn changelog_groups_default_to_the_conventional_list() {
    for text in [TWO_REPO, SINGLE_REPO] {
        let p = ProjFile::parse(text).unwrap();
        assert_eq!(
            p.release.changelog_groups,
            vec![
                "feat", "fix", "perf", "docs", "refactor", "test", "build", "ci", "chore"
            ],
            "default group list differs for {text:?}"
        );
    }
}

#[test]
fn remote_names_are_validated() {
    // The remote is passed to `git` as a bare argument (e.g. `git push <remote> <ref>`),
    // so a leading `-` or whitespace must be refused: either could be parsed as an option
    // and break the call or, worse, be interpreted as something else.
    let private_dash = TWO_REPO.replace("remote = \"origin\"", "remote = \"-upload-pack=x\"");
    let private_space = TWO_REPO.replace("remote = \"origin\"", "remote = \"upstream main\"");
    assert!(has_issue(&private_dash, "workflow.private.remote"));
    assert!(has_issue(&private_space, "workflow.private.remote"));

    // A clean name is accepted.
    assert!(ProjFile::parse(TWO_REPO).is_ok());

    // The public side is checked the same way (the default "origin" is fine).
    let public_dash = TWO_REPO.replace(
        "[workflow.public]\npath",
        "[workflow.public]\nremote = \"-bad\"\npath",
    );
    let keys: Vec<String> = issues(&public_dash).into_iter().map(|i| i.key).collect();
    assert!(
        keys.contains(&"workflow.public.remote".to_string()),
        "{keys:?}"
    );
}

#[test]
fn changelog_groups_are_validated() {
    let text = TWO_REPO.replace(
        "[release]\nversion_files",
        "[release]\nchangelog_groups = [\"feat\", \"FEAT\", \"Bad Prefix!\", \"\", \"perf\"]\nversion_files",
    );
    let keys: Vec<String> = issues(&text).into_iter().map(|i| i.key).collect();
    assert!(
        keys.contains(&"release.changelog_groups[1]".to_string()),
        "{keys:?}"
    );
    assert!(
        keys.contains(&"release.changelog_groups[2]".to_string()),
        "{keys:?}"
    );
    assert!(
        keys.contains(&"release.changelog_groups[3]".to_string()),
        "{keys:?}"
    );
    assert!(
        !keys.contains(&"release.changelog_groups[0]".to_string()),
        "{keys:?}"
    );
    assert!(
        !keys.contains(&"release.changelog_groups[4]".to_string()),
        "{keys:?}"
    );
}

#[test]
fn duplicate_targets_are_reported() {
    let text = format!("{TWO_REPO}\n[[release.targets]]\nkind = \"github\"\n");
    assert!(has_issue(&text, "release.targets[1].kind"));
}

#[test]
fn branch_name_rules() {
    for bad in [
        "-x", "a..b", "a b", "a~b", "a/", "a.lock", ".hidden", "a//b", "@",
    ] {
        let text = SINGLE_REPO.replace("branch = \"public\"", &format!("branch = \"{bad}\""));
        assert!(has_issue(&text, "workflow.public.branch"), "{bad} accepted");
    }
    for good in ["release/v1", "feature-x", "public_2"] {
        let text = SINGLE_REPO.replace("branch = \"public\"", &format!("branch = \"{good}\""));
        ProjFile::parse(&text).unwrap_or_else(|e| panic!("{good}: {e}"));
    }
}

#[test]
fn find_proj_file_cases() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        find_proj_file(dir.path()),
        Err(ProjError::NotFound(_))
    ));
    std::fs::write(dir.path().join("a.proj"), SINGLE_REPO).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
    assert_eq!(
        find_proj_file(dir.path()).unwrap(),
        dir.path().join("a.proj")
    );
    std::fs::write(dir.path().join("b.proj"), SINGLE_REPO).unwrap();
    assert!(matches!(
        find_proj_file(dir.path()),
        Err(ProjError::Ambiguous { .. })
    ));
}

#[test]
fn public_root_resolves_by_mode() {
    let root = Path::new("/projects/example");

    let two = ProjFile::parse(TWO_REPO).unwrap();
    assert_eq!(
        public_root(root, &two).unwrap(),
        root.join("../ExampleMod-public")
    );

    // Single-repo has no separate public repo: the public side is `root` itself.
    let single = ProjFile::parse(SINGLE_REPO).unwrap();
    assert_eq!(public_root(root, &single).unwrap(), root.to_path_buf());
}

/// `private_root` mirrors `public_root`: the legacy layout (proj in the private repo,
/// `path = "."`) resolves to `root` itself, and the new layout (proj at the project root,
/// `path = "./private"`) resolves into the child `private/` repo.
#[test]
fn private_root_resolves_both_layouts() {
    let root = Path::new("/projects/example");

    // Legacy layout: the proj lives in the private repo itself.
    let legacy = ProjFile::parse(TWO_REPO).unwrap();
    assert_eq!(private_root(root, &legacy).unwrap(), root.to_path_buf());

    // New layout: the proj lives at the project root, the private repo is beside public/.
    let new_text = TWO_REPO.replace("path = \".\"", "path = \"./private\"");
    let new = ProjFile::parse(&new_text).unwrap();
    assert_eq!(private_root(root, &new).unwrap(), root.join("private"));

    // Single-repo has no separate private repo: it is the root itself.
    let single = ProjFile::parse(SINGLE_REPO).unwrap();
    assert_eq!(private_root(root, &single).unwrap(), root.to_path_buf());

    // A two-repo side with no path resolves to the root (the default `"."`), never an error.
    let no_path =
        ProjFile::parse(&new_text.replace("path = \"./private\"", "# path omitted")).unwrap();
    assert_eq!(private_root(root, &no_path).unwrap(), root.to_path_buf());
}

#[test]
fn line_col_counts_characters() {
    assert_eq!(line_col("ab\ncé\nx", 0), (1, 1));
    assert_eq!(line_col("ab\ncé\nx", 3), (2, 1));
    assert_eq!(line_col("ab\ncé\nx", 7), (3, 1));
}
