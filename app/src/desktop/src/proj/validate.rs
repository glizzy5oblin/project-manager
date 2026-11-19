//! Semantic checks that serde cannot express. Collects every problem instead of stopping at the
//! first, so the user can fix the file in one pass.

use super::schema::{ASSET_PLACEHOLDERS, ProjFile, WorkflowMode};
use super::{CURRENT_SCHEMA_VERSION, Issue};

pub(crate) fn validate(proj: &ProjFile) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut issue = |key: &str, message: String| {
        issues.push(Issue {
            key: key.to_string(),
            message,
        })
    };

    if proj.schema_version != CURRENT_SCHEMA_VERSION {
        issue(
            "schema_version",
            format!(
                "expected {CURRENT_SCHEMA_VERSION} after migration, found {}",
                proj.schema_version
            ),
        );
    }

    // [project]
    if proj.project.name.trim().is_empty() {
        issue("project.name", "must not be empty".into());
    } else if proj.project.name.contains(['/', '\\']) {
        issue("project.name", "must not contain path separators".into());
    }
    if let Err(e) = semver::Version::parse(&proj.project.version) {
        issue(
            "project.version",
            format!(
                "\"{}\" is not a semantic version (x.y.z): {e}",
                proj.project.version
            ),
        );
    }

    // [workflow]
    let wf = &proj.workflow;
    for (side, s) in [("private", &wf.private), ("public", &wf.public)] {
        if let Err(msg) = check_ref_name(&s.branch) {
            issue(&format!("workflow.{side}.branch"), msg);
        }
        if s.remote.trim().is_empty() {
            issue(
                &format!("workflow.{side}.remote"),
                "must not be empty".into(),
            );
        }
        if let Some(p) = &s.path
            && p.trim().is_empty()
        {
            issue(
                &format!("workflow.{side}.path"),
                "must not be empty when given".into(),
            );
        }
    }
    let private_path = wf.private.path.as_deref().unwrap_or(".");
    match wf.mode {
        WorkflowMode::TwoRepo => match wf.public.path.as_deref() {
            None => issue(
                "workflow.public.path",
                "required in two-repo mode (folder of the public repo)".into(),
            ),
            Some(p) if same_path(p, private_path) => issue(
                "workflow.public.path",
                "must differ from workflow.private.path in two-repo mode".into(),
            ),
            Some(_) => {}
        },
        WorkflowMode::SingleRepo => {
            if let Some(p) = wf.public.path.as_deref()
                && !same_path(p, private_path)
            {
                issue(
                    "workflow.public.path",
                    "single-repo mode uses one repository; omit this or set it to the private path"
                        .into(),
                );
            }
            if wf.public.branch == wf.private.branch {
                issue(
                    "workflow.public.branch",
                    "must differ from workflow.private.branch in single-repo mode".into(),
                );
            }
        }
    }

    // [staging]
    let st = &proj.staging;
    let lists = [
        ("staging.include", &st.include, true),
        ("staging.exclude", &st.exclude, false),
        ("staging.overrides.include", &st.overrides.include, false),
        ("staging.overrides.exclude", &st.overrides.exclude, false),
    ];
    for (key, globs, allow_negation) in lists {
        for (i, g) in globs.iter().enumerate() {
            let key = format!("{key}[{i}]");
            let pattern = match g.strip_prefix('!') {
                Some(rest) if allow_negation => rest,
                Some(_) => {
                    issue(
                        &key,
                        "\"!\" negation is only allowed in staging.include".into(),
                    );
                    continue;
                }
                None => g.as_str(),
            };
            if let Err(msg) = check_relative(pattern) {
                issue(&key, msg);
            } else if let Err(e) = globset::Glob::new(pattern) {
                issue(&key, format!("invalid glob \"{pattern}\": {}", e.kind()));
            }
        }
    }

    // [release]
    let rel = &proj.release;
    for (i, f) in rel.version_files.iter().enumerate() {
        if let Err(msg) = check_relative(f) {
            issue(&format!("release.version_files[{i}]"), msg);
        }
    }
    if rel.asset_name.trim().is_empty() {
        issue("release.asset_name", "must not be empty".into());
    }
    for ph in placeholders(&rel.asset_name) {
        if !ASSET_PLACEHOLDERS.contains(&ph.as_str()) {
            issue(
                "release.asset_name",
                format!("unknown placeholder {{{ph}}}; allowed: {{name}}, {{version}}"),
            );
        }
    }
    if rel.asset_name.contains(['/', '\\']) {
        issue(
            "release.asset_name",
            "must be a file name, not a path".into(),
        );
    }
    let mut seen = Vec::new();
    for (i, t) in rel.targets.iter().enumerate() {
        let key = format!("release.targets[{i}].kind");
        if seen.contains(&t.kind) {
            issue(&key, "duplicate release target".into());
        }
        seen.push(t.kind);
    }

    issues
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| {
        let p = p.replace('\\', "/");
        let p = p.trim_end_matches('/');
        let p = p.strip_prefix("./").unwrap_or(p);
        if p.is_empty() {
            ".".to_string()
        } else {
            p.to_string()
        }
    };
    norm(a) == norm(b)
}

/// Paths inside the project: relative and not escaping it with `..`.
fn check_relative(p: &str) -> Result<(), String> {
    if p.trim().is_empty() {
        return Err("must not be empty".into());
    }
    let unix = p.replace('\\', "/");
    if unix.starts_with('/') || (unix.len() > 1 && unix.as_bytes()[1] == b':') {
        return Err(format!("\"{p}\" must be relative to the project"));
    }
    if unix.split('/').any(|c| c == "..") {
        return Err(format!("\"{p}\" must not leave the project with \"..\""));
    }
    Ok(())
}

/// The main rules of `git check-ref-format` for a branch name.
fn check_ref_name(name: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("\"{name}\" is not a valid branch name: {why}"));
    if name.is_empty() {
        return Err("must not be empty".into());
    }
    if name.starts_with('-') {
        return bad("starts with \"-\"");
    }
    if name.contains("..") || name.contains("@{") || name.contains("//") {
        return bad("contains \"..\", \"@{\" or \"//\"");
    }
    if name
        .chars()
        .any(|c| c.is_control() || " ~^:?*[\\".contains(c))
    {
        return bad("contains a space or one of ~ ^ : ? * [ \\");
    }
    if name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.ends_with(".lock")
    {
        return bad("starts or ends with \"/\", or ends with \".\" or \".lock\"");
    }
    if name.split('/').any(|c| c.starts_with('.')) || name == "@" {
        return bad("a component starts with \".\" or the name is \"@\"");
    }
    Ok(())
}

fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                out.push(after[..end].to_string());
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out
}
