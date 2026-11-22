//! Plain-words facts about a file that is in conflict: what kind of conflict it is (both sides
//! changed it, one side deleted it, both added it), which two versions are in play, and for a
//! file changed on both sides, where each disagreement is and what each side wrote there.
//! The window turns this into sentences; nothing here changes the repository.

use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

use super::{GitCommand, GitError};

/// Largest file whose markers are read, in bytes.
const MAX_BYTES: u64 = 2 * 1024 * 1024;
/// Lines kept per side of one disagreement.
const MAX_LINES: usize = 40;
/// Disagreements listed for one file.
const MAX_REGIONS: usize = 50;

/// How the two sides disagree about the file. "Ours" is the branch you are on, "theirs" the one
/// being merged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum ConflictKind {
    /// Both sides changed the file.
    BothModified,
    /// Both sides created a file with this name, with different contents.
    BothAdded,
    /// You deleted the file; the other side changed it.
    DeletedByUs,
    /// The other side deleted the file; you changed it.
    DeletedByThem,
    /// Both sides deleted it (a rename that went two ways).
    BothDeleted,
    /// Only you added the file (the other side has a rename or folder in its place).
    AddedByUs,
    /// Only the other side added the file.
    AddedByThem,
    /// Git no longer lists the file as in conflict.
    Resolved,
}

/// One place in the file where the two sides wrote different things.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRegion {
    /// First and last line of the region in the file, markers included (1-based).
    pub start_line: u32,
    pub end_line: u32,
    /// What your side wrote, and what the other side wrote.
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
    /// What both started from, when git recorded it (the `diff3` conflict style).
    pub base: Option<Vec<String>>,
    /// Some lines were left out to keep the list short.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    pub path: String,
    pub kind: ConflictKind,
    /// The branch you are on, or `None` when HEAD is detached.
    pub ours_name: Option<String>,
    /// The branch being merged in; a short commit id when it has no branch; `None` outside a
    /// merge (rebase, cherry-pick, stash apply).
    pub theirs_name: Option<String>,
    /// The file is not text, so there are no regions to show.
    pub binary: bool,
    pub regions: Vec<ConflictRegion>,
    /// More disagreements exist than are listed.
    pub more_regions: bool,
    /// The whole file as it is on disk, with the markers, for the inline editor. None for a
    /// binary or very large file, or one that is missing.
    pub text: Option<String>,
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let out = GitCommand::new(root).args(args).run().ok()?;
    out.success().then(|| out.stdout_str())
}

/// Which of stages 1 (base), 2 (ours) and 3 (theirs) git holds for `path`.
fn stages(root: &Path, path: &str) -> Result<[bool; 3], GitError> {
    let out = GitCommand::new(root)
        .args(["ls-files", "-u", "-z", "--"])
        .arg(path)
        .run()?
        .into_result()?;
    let mut have = [false; 3];
    for entry in out.stdout.split(|b| *b == 0) {
        // "<mode> <sha> <stage>\t<path>"
        let text = String::from_utf8_lossy(entry);
        let Some((meta, _)) = text.split_once('\t') else {
            continue;
        };
        if let Some(stage) = meta.split(' ').nth(2).and_then(|s| s.parse::<usize>().ok())
            && (1..=3).contains(&stage)
        {
            have[stage - 1] = true;
        }
    }
    Ok(have)
}

fn kind_of([base, ours, theirs]: [bool; 3]) -> ConflictKind {
    match (base, ours, theirs) {
        (true, true, true) => ConflictKind::BothModified,
        (false, true, true) => ConflictKind::BothAdded,
        (true, false, true) => ConflictKind::DeletedByUs,
        (true, true, false) => ConflictKind::DeletedByThem,
        (true, false, false) => ConflictKind::BothDeleted,
        (false, true, false) => ConflictKind::AddedByUs,
        (false, false, true) => ConflictKind::AddedByThem,
        (false, false, false) => ConflictKind::Resolved,
    }
}

/// The name of the branch being merged in, from `MERGE_HEAD`.
fn merging_name(root: &Path) -> Option<String> {
    let sha = git_text(root, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])?;
    let branches = git_text(
        root,
        &[
            "for-each-ref",
            "--points-at",
            &sha,
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .unwrap_or_default();
    Some(
        branches
            .lines()
            .find(|l| !l.is_empty() && !l.ends_with("/HEAD"))
            .map(str::to_string)
            .unwrap_or_else(|| sha.chars().take(8).collect()),
    )
}

/// The disagreements marked in `text`. Understands both conflict styles (with and without the
/// `|||||||` section that shows the common starting point).
pub fn parse_regions(text: &str) -> (Vec<ConflictRegion>, bool) {
    enum Part {
        Outside,
        Ours,
        Base,
        Theirs,
    }
    let mut regions = Vec::new();
    let mut more = false;
    let mut part = Part::Outside;
    let mut cur: Option<ConflictRegion> = None;
    let mut lost = false;
    let push = |list: &mut Vec<String>, line: &str, lost: &mut bool| {
        if list.len() < MAX_LINES {
            list.push(line.to_string());
        } else {
            *lost = true;
        }
    };
    for (i, line) in text.lines().enumerate() {
        let n = (i + 1) as u32;
        match part {
            Part::Outside => {
                if line.starts_with("<<<<<<<") && (line.len() == 7 || line.as_bytes()[7] == b' ') {
                    cur = Some(ConflictRegion {
                        start_line: n,
                        end_line: n,
                        ours: Vec::new(),
                        theirs: Vec::new(),
                        base: None,
                        truncated: false,
                    });
                    lost = false;
                    part = Part::Ours;
                }
            }
            _ => {
                let Some(r) = cur.as_mut() else { continue };
                if line.starts_with("|||||||") && matches!(part, Part::Ours) {
                    r.base = Some(Vec::new());
                    part = Part::Base;
                } else if line == "=======" && matches!(part, Part::Ours | Part::Base) {
                    part = Part::Theirs;
                } else if line.starts_with(">>>>>>>") && matches!(part, Part::Theirs) {
                    r.end_line = n;
                    r.truncated = lost;
                    if regions.len() < MAX_REGIONS {
                        regions.push(cur.take().expect("a region is open"));
                    } else {
                        more = true;
                        cur = None;
                    }
                    part = Part::Outside;
                } else {
                    match part {
                        Part::Ours => push(&mut r.ours, line, &mut lost),
                        Part::Theirs => push(&mut r.theirs, line, &mut lost),
                        Part::Base => {
                            if let Some(b) = r.base.as_mut() {
                                push(b, line, &mut lost)
                            }
                        }
                        Part::Outside => {}
                    }
                }
            }
        }
    }
    (regions, more)
}

/// Describe the conflict in `path` (relative to the repository root).
pub fn describe(root: &Path, path: &str) -> Result<ConflictInfo, GitError> {
    let kind = kind_of(stages(root, path)?);
    let ours_name = git_text(root, &["symbolic-ref", "-q", "--short", "HEAD"]);
    let theirs_name = merging_name(root);
    let mut info = ConflictInfo {
        path: path.to_string(),
        kind,
        ours_name,
        theirs_name,
        binary: false,
        regions: Vec::new(),
        more_regions: false,
        text: None,
    };
    if matches!(kind, ConflictKind::BothModified | ConflictKind::BothAdded) {
        let file = root.join(path);
        let small = std::fs::metadata(&file).is_ok_and(|m| m.len() <= MAX_BYTES);
        match std::fs::read(&file) {
            Ok(bytes) if small => {
                if bytes.contains(&0) {
                    info.binary = true;
                } else {
                    let text = String::from_utf8_lossy(&bytes);
                    let (regions, more) = parse_regions(&text);
                    info.regions = regions;
                    info.more_regions = more;
                    // Editing text that is not valid UTF-8 would damage it on save.
                    info.text = String::from_utf8(bytes).ok();
                }
            }
            Ok(_) => info.binary = true,
            Err(_) => {}
        }
    }
    Ok(info)
}

#[cfg(test)]
mod tests;
