use std::collections::{BTreeSet, HashMap};

use super::graph::{GraphOptions, RefKind, build, current_key};
use super::layout::{Layout, layout};
use crate::testing::FixtureRepo;
use crate::testing::shapes::*;

// ---- layout (pure) ----

/// Follow every segment down from `row`'s node; the commits reached must be exactly its parents.
/// Also checks that every row's node sits on a lane and segments only touch lanes in range.
fn check_connectivity(lay: &Layout, parents: &[Vec<u32>]) {
    let n = parents.len();
    assert_eq!(lay.lane.len(), n);
    assert_eq!(lay.edge_start.len(), if n == 0 { 1 } else { n + 1 });
    for r in 0..n {
        assert!(lay.lane[r] < lay.max_lanes);
        for e in lay.edges_of(r) {
            assert!(
                e.from < lay.max_lanes && e.to < lay.max_lanes,
                "row {r}: {e:?}"
            );
            assert!(r + 1 < n, "the last row has segments");
        }
        let mut reached = BTreeSet::new();
        // (row, lane) of a line's current point.
        let mut stack: Vec<(usize, u16)> = lay
            .edges_of(r)
            .iter()
            .filter(|e| e.from == lay.lane[r])
            .map(|e| (r + 1, e.to))
            .collect();
        let mut seen = BTreeSet::new();
        while let Some((row, lane)) = stack.pop() {
            if !seen.insert((row, lane)) {
                continue;
            }
            if lay.lane[row] == lane {
                reached.insert(row as u32);
                continue;
            }
            for e in lay.edges_of(row).iter().filter(|e| e.from == lane) {
                stack.push((row + 1, e.to));
            }
        }
        let want: BTreeSet<u32> = parents[r].iter().copied().collect();
        assert_eq!(
            reached, want,
            "row {r}: lines reach {reached:?}, parents are {want:?}"
        );
    }
}

fn run_layout(parents: &[Vec<u32>]) -> Layout {
    let lay = layout(parents.len(), |r| parents[r].clone());
    check_connectivity(&lay, parents);
    lay
}

#[test]
fn layout_empty_and_single() {
    let lay = run_layout(&[]);
    assert_eq!(lay.max_lanes, 0);
    let lay = run_layout(&[vec![]]);
    assert_eq!(lay.lane, vec![0]);
    assert_eq!(lay.max_lanes, 1);
}

#[test]
fn layout_linear_stays_in_one_lane() {
    let parents: Vec<Vec<u32>> = (0..5u32)
        .map(|r| if r < 4 { vec![r + 1] } else { vec![] })
        .collect();
    let lay = run_layout(&parents);
    assert_eq!(lay.lane, vec![0; 5]);
    assert_eq!(lay.max_lanes, 1);
    assert!(lay.color.iter().all(|&c| c == lay.color[0]));
}

#[test]
fn layout_branch_and_merge() {
    // 0: merge(1, 2); 1: main(3); 2: feature(3); 3: base
    let parents = vec![vec![1, 2], vec![3], vec![3], vec![]];
    let lay = run_layout(&parents);
    assert_eq!(lay.lane, vec![0, 0, 1, 0]);
    assert_eq!(lay.max_lanes, 2);
    // The feature lane has its own colour.
    assert_ne!(lay.color[2], lay.color[1]);
}

#[test]
fn layout_two_tips_share_a_base() {
    // 0: tip a(2); 1: tip b(2); 2: base
    let parents = vec![vec![2], vec![2], vec![]];
    let lay = run_layout(&parents);
    assert_eq!(lay.lane, vec![0, 1, 0]);
}

#[test]
fn layout_freed_lanes_are_reused() {
    // Two short side branches one after the other both use lane 1.
    // 0: m(1,2) 1: x(3) 2: s1(3) 3: m(4,5) 4: y(6) 5: s2(6) 6: root
    let parents = vec![
        vec![1, 2],
        vec![3],
        vec![3],
        vec![4, 5],
        vec![6],
        vec![6],
        vec![],
    ];
    let lay = run_layout(&parents);
    assert_eq!(lay.lane[2], 1);
    assert_eq!(lay.lane[5], 1);
    assert_eq!(lay.max_lanes, 2);
}

#[test]
fn layout_octopus_and_shared_merge_parent() {
    // 0: octopus(1, 2, 3); 1, 2, 3 -> 4. Then 4 merges a parent another lane already waits for.
    let parents = vec![
        vec![1, 2, 3],
        vec![4],
        vec![4],
        vec![4],
        vec![5, 6],
        vec![6],
        vec![],
    ];
    run_layout(&parents);
    // A merge whose second parent is already waited for by another lane.
    let parents = vec![vec![2], vec![2, 3], vec![3], vec![]];
    run_layout(&parents);
}

#[test]
fn layout_random_dags() {
    let mut rng: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |m: u32| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng % m as u64) as u32
    };
    for case in 0..300 {
        let n = 1 + next(80) as usize;
        let parents: Vec<Vec<u32>> = (0..n as u32)
            .map(|r| {
                let below = n as u32 - r - 1;
                if below == 0 || (case % 7 != 0 && next(12) == 0) {
                    return vec![];
                }
                let mut ps = Vec::new();
                for _ in 0..1 + (next(10) / 7) + (next(30) / 29) {
                    let span = below.min(1 + next(8));
                    let p = r + 1 + next(span);
                    if !ps.contains(&p) {
                        ps.push(p);
                    }
                }
                ps
            })
            .collect();
        run_layout(&parents);
    }
}

// ---- graph against git ----

/// `git log --date-order` of the given revisions as (commit, parents) lines.
fn git_order(r: &FixtureRepo, revs: &[&str]) -> Vec<(String, Vec<String>)> {
    let mut args = vec!["log", "--date-order", "--format=%H %P"];
    args.extend_from_slice(revs);
    r.git(&args)
        .lines()
        .map(|l| {
            let mut it = l.split_whitespace().map(str::to_string);
            (it.next().unwrap(), it.collect())
        })
        .collect()
}

fn graph_order(g: &super::graph::Graph) -> Vec<(String, Vec<String>)> {
    (0..g.len())
        .map(|r| {
            (
                g.id(r).to_string(),
                g.parents_of(r)
                    .iter()
                    .map(|&p| g.id(p as usize).to_string())
                    .collect(),
            )
        })
        .collect()
}

const ALL: &[&str] = &["--branches", "--remotes", "--tags", "HEAD"];

fn assert_matches_git(r: &FixtureRepo) {
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    assert_eq!(graph_order(&g), git_order(r, ALL));
    let parents: Vec<Vec<u32>> = (0..g.len()).map(|i| g.parents_of(i).to_vec()).collect();
    check_connectivity(&g.layout, &parents);
}

#[test]
fn graph_matches_git_on_fixture_shapes() {
    assert_matches_git(&linear(4));
    assert_matches_git(&many_branches(6));
    assert_matches_git(&merges());
    assert_matches_git(&octopus());
    assert_matches_git(&conflict());
    let (clone, _remote) = ahead_behind();
    assert_matches_git(&clone);
}

#[test]
fn graph_matches_git_on_branchy_history() {
    for seed in 1..=4 {
        assert_matches_git(&branchy_history(400, 2 + seed as usize * 2, seed, false));
    }
}

#[test]
fn graph_is_topological_with_same_second_commits() {
    // Git breaks ties between equal commit times by the order commits were queued, which
    // depends on its walk internals; we only promise a valid order and the same commit set.
    for seed in 1..=3 {
        let r = branchy_history(300, 6, seed, true);
        let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
        let want: BTreeSet<String> = git_order(&r, ALL).into_iter().map(|c| c.0).collect();
        let got: BTreeSet<String> = graph_order(&g).into_iter().map(|c| c.0).collect();
        assert_eq!(got, want);
        for row in 0..g.len() {
            assert!(g.parents_of(row).iter().all(|&p| p as usize > row));
        }
        let parents: Vec<Vec<u32>> = (0..g.len()).map(|i| g.parents_of(i).to_vec()).collect();
        check_connectivity(&g.layout, &parents);
    }
}

#[test]
fn graph_of_empty_repo() {
    let r = FixtureRepo::new();
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    assert!(g.is_empty());
    let info = g.info();
    assert_eq!(info.rows, 0);
    assert_eq!(info.head_row, None);
    assert!(g.rows(0, 10).unwrap().is_empty());
}

#[test]
fn graph_filters_by_ref() {
    let r = merges();
    let opts = GraphOptions {
        refs: Some(vec!["refs/heads/feature/a".into()]),
        highlight: None,
    };
    let g = build(r.path(), &opts, 1).unwrap();
    assert_eq!(graph_order(&g), git_order(&r, &["feature/a"]));

    let opts = GraphOptions {
        refs: Some(vec![
            "refs/heads/feature/a".into(),
            "refs/heads/feature/b".into(),
        ]),
        highlight: None,
    };
    let g = build(r.path(), &opts, 1).unwrap();
    assert_eq!(graph_order(&g), git_order(&r, &["feature/a", "feature/b"]));

    let opts = GraphOptions {
        refs: Some(vec!["refs/heads/nope".into()]),
        highlight: None,
    };
    let err = build(r.path(), &opts, 1)
        .err()
        .expect("unknown ref is an error");
    assert!(err.to_string().contains("refs/heads/nope"), "{err}");
}

#[test]
fn graph_highlight_dims_other_commits() {
    let r = merges();
    let opts = GraphOptions {
        refs: None,
        highlight: Some("refs/heads/feature/b".into()),
    };
    let g = build(r.path(), &opts, 1).unwrap();
    let bright: BTreeSet<String> = r
        .git(&["rev-list", "feature/b"])
        .lines()
        .map(str::to_string)
        .collect();
    for row in g.rows(0, g.len() as u32).unwrap() {
        assert_eq!(row.dim, !bright.contains(&row.id), "{}", row.subject);
    }
}

#[test]
fn graph_rows_carry_labels_and_text() {
    let r = many_branches(3);
    let g = build(r.path(), &GraphOptions::default(), 7).unwrap();
    let info = g.info();
    assert_eq!(info.generation, 7);
    let total: usize = r.git(&["rev-list", "--count", "--all"]).parse().unwrap();
    assert_eq!(info.rows as usize, total);
    let head = r.head();
    let rows = g.rows(0, info.rows).unwrap();
    let head_row = &rows[info.head_row.unwrap() as usize];
    assert_eq!(head_row.id, head);
    assert_eq!(head_row.refs[0].name, "main");
    assert!(head_row.refs[0].is_head);
    assert_eq!(head_row.subject, "main 2");
    assert_eq!(head_row.author_name, "Fixture Author");
    assert_eq!(head_row.author_email, "author@example.invalid");

    let by_name: HashMap<&str, &super::graph::RefLabel> = rows
        .iter()
        .flat_map(|row| row.refs.iter())
        .map(|l| (l.name.as_str(), l))
        .collect();
    assert_eq!(by_name["v0.1.0"].kind, RefKind::Tag);
    assert_eq!(by_name["v0.1.0"].full, "refs/tags/v0.1.0");
    assert_eq!(by_name["feature/1"].kind, RefKind::Branch);
    assert!(!by_name["feature/1"].is_head);
    // The annotated tag labels the commit it points to.
    let tagged = rows
        .iter()
        .find(|row| row.refs.iter().any(|l| l.name == "v0.1.0"))
        .unwrap();
    assert_eq!(tagged.id, r.rev("v0.1.0^{commit}"));

    // Paging: a window in the middle, and past the end.
    let mid = g.rows(2, 3).unwrap();
    assert_eq!(mid.len(), 3);
    assert_eq!(mid[0].row, 2);
    assert_eq!(mid[0].id, rows[2].id);
    assert!(g.rows(info.rows, 5).unwrap().is_empty());
    assert_eq!(g.rows(info.rows - 1, 5).unwrap().len(), 1);
}

#[test]
fn workflow_refs_name_existing_private_and_public_branches() {
    use super::graph::{RefLabel, workflow_refs};
    use crate::proj::{RepoSide, Workflow, WorkflowMode};
    let label = |full: &str| RefLabel {
        name: full.rsplit('/').next().unwrap().to_string(),
        full: full.to_string(),
        kind: RefKind::Branch,
        is_head: false,
    };
    let side = |branch: &str| RepoSide {
        path: None,
        branch: branch.to_string(),
        remote: "origin".to_string(),
    };
    let refs = [
        label("refs/heads/dev"),
        label("refs/heads/main"),
        label("refs/remotes/origin/main"),
    ];
    let mut w = Workflow {
        mode: WorkflowMode::SingleRepo,
        private: side("dev"),
        public: side("main"),
    };
    assert_eq!(
        workflow_refs(&w, &refs),
        ["refs/heads/dev", "refs/heads/main"]
    );
    // The public branch of a two-repo project is in the other repository.
    w.mode = WorkflowMode::TwoRepo;
    assert_eq!(workflow_refs(&w, &refs), ["refs/heads/dev"]);
    // Missing branches are left out; the same branch twice appears once.
    w.mode = WorkflowMode::SingleRepo;
    w.private = side("gone");
    assert_eq!(workflow_refs(&w, &refs), ["refs/heads/main"]);
    w.private = side("main");
    assert_eq!(workflow_refs(&w, &refs), ["refs/heads/main"]);
}

#[test]
fn graph_labels_remotes_and_detached_head() {
    let (clone, _remote) = ahead_behind();
    clone.git(&["checkout", "-q", "--detach", "HEAD~1"]);
    let g = build(clone.path(), &GraphOptions::default(), 1).unwrap();
    let info = g.info();
    let names: Vec<(&str, RefKind)> = info
        .refs
        .iter()
        .map(|l| (l.name.as_str(), l.kind))
        .collect();
    assert!(
        names.contains(&("origin/main", RefKind::Remote)),
        "{names:?}"
    );
    assert!(names.contains(&("HEAD", RefKind::Head)), "{names:?}");
    let head_row = info.head_row.unwrap();
    assert_eq!(g.id(head_row as usize).to_string(), clone.rev("HEAD"));
    assert!(
        g.labels(head_row)
            .iter()
            .any(|l| l.kind == RefKind::Head && l.is_head)
    );
    // `main` is not checked out, so it is not marked as HEAD's branch.
    assert!(
        info.refs
            .iter()
            .all(|l| l.kind == RefKind::Head || !l.is_head)
    );
}

#[test]
fn graph_leaves_out_backups_and_stash() {
    let r = linear(2);
    r.git(&["update-ref", "refs/pm/backups/x", "HEAD~1"]);
    r.write("file.txt", "dirty\n");
    r.git(&["stash", "-q"]);
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    assert_eq!(g.len(), 2, "the stash commits are not in the graph");
    assert!(g.info().refs.iter().all(|l| !l.full.starts_with("refs/pm")));
}

#[test]
fn graph_key_changes_with_refs() {
    let r = linear(2);
    let opts = GraphOptions::default();
    let g = build(r.path(), &opts, 1).unwrap();
    assert_eq!(current_key(r.path(), &opts).unwrap(), g.key);
    r.commit_file("file.txt", "more\n", "more");
    assert_ne!(current_key(r.path(), &opts).unwrap(), g.key);
}

#[test]
fn graph_key_changes_when_head_moves_to_a_branch_at_the_same_commit() {
    let r = linear(2);
    r.git(&["branch", "other"]);
    let opts = GraphOptions::default();
    let g = build(r.path(), &opts, 1).unwrap();
    r.git(&["switch", "-q", "other"]);
    let key = current_key(r.path(), &opts).unwrap();
    assert_eq!(key.head, g.key.head, "same commit");
    assert_ne!(key, g.key, "the graph must rebuild so the HEAD label moves");
    let g2 = build(r.path(), &opts, 2).unwrap();
    // Both branches sit on the newest commit, row 0.
    let head_branch = |g: &super::graph::Graph| {
        g.labels(0)
            .iter()
            .find(|l| l.is_head)
            .map(|l| l.name.clone())
    };
    assert_eq!(head_branch(&g).as_deref(), Some("main"));
    assert_eq!(head_branch(&g2).as_deref(), Some("other"));

    r.git(&["switch", "-q", "--detach"]);
    assert_ne!(current_key(r.path(), &opts).unwrap(), key, "detached");
}

#[test]
fn graph_find_by_full_and_short_id() {
    let r = merges();
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    let head = r.head();
    let row = g.find(&head).unwrap().unwrap();
    assert_eq!(g.id(row as usize).to_string(), head);
    assert_eq!(g.find(&head[..10].to_uppercase()).unwrap(), Some(row));
    assert_eq!(
        g.find("0000000000000000000000000000000000000000").unwrap(),
        None
    );
    assert!(g.find("abc").is_err(), "too short");
    assert!(g.find("HEAD~1").is_err(), "not hex: no revision syntax");
    assert!(g.find(&"a".repeat(41)).is_err());
}

// ---- search against git ----

use super::search::{LogQuery, clean_path, search};

fn search_rows(r: &FixtureRepo, q: &LogQuery) -> Vec<String> {
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    let res = search(&g, q, || false).unwrap();
    res.rows
        .iter()
        .map(|&row| g.id(row as usize).to_string())
        .collect()
}

fn git_rows(r: &FixtureRepo, args: &[&str], paths: &[&str]) -> Vec<String> {
    let mut a = vec!["log", "--date-order", "--format=%H"];
    a.extend_from_slice(args);
    if args.iter().all(|x| !x.starts_with("refs/")) {
        a.extend_from_slice(ALL);
    }
    if !paths.is_empty() {
        a.push("--");
        a.extend_from_slice(paths);
    }
    r.git(&a).lines().map(str::to_string).collect()
}

/// A repo with several authors and messages in mixed case.
fn authors_repo() -> FixtureRepo {
    let r = many_branches(4);
    let people = [
        "Ana López <ana@example.invalid>",
        "Bob Stone <bob@EXAMPLE.invalid>",
        "Chen Wei <chen@example.invalid>",
    ];
    for (i, who) in people.iter().cycle().take(7).enumerate() {
        r.commit_file(
            "people.txt",
            &format!("{i}\n"),
            &format!("Fix BUG #{i} in Parser\n\nBody line {i}"),
        );
        r.git(&["commit", "-q", "--amend", "--no-edit", "--author", who]);
    }
    r
}

#[test]
fn search_message_matches_git_grep() {
    let r = authors_repo();
    for text in [
        "bug",
        "PARSER",
        "fix bug #3",
        "body line 2",
        "work 1",
        "nothing like this",
    ] {
        let q = LogQuery {
            message: Some(text.into()),
            ..Default::default()
        };
        let grep = format!("--grep={text}");
        assert_eq!(
            search_rows(&r, &q),
            git_rows(&r, &["-i", "-F", &grep], &[]),
            "{text}"
        );
    }
}

#[test]
fn search_author_matches_git_author() {
    let r = authors_repo();
    for text in [
        "ana",
        "LÓPEZ",
        "bob@example",
        "<chen@",
        "fixture author",
        "zed",
    ] {
        let q = LogQuery {
            author: Some(text.into()),
            ..Default::default()
        };
        let a = format!("--author={text}");
        if text == "LÓPEZ" {
            // git's -i folds ASCII only; we fold Unicode too, which finds more, never less.
            assert_eq!(search_rows(&r, &q).len(), 3);
            continue;
        }
        assert_eq!(
            search_rows(&r, &q),
            git_rows(&r, &["-i", "-F", &a], &[]),
            "{text}"
        );
    }
    // Message and author together: both must match.
    let q = LogQuery {
        message: Some("bug".into()),
        author: Some("bob".into()),
        ..Default::default()
    };
    let got = search_rows(&r, &q);
    assert_eq!(got.len(), 2);
    assert_eq!(
        got,
        git_rows(&r, &["-i", "-F", "--grep=bug", "--author=bob"], &[])
    );
}

#[test]
fn search_path_matches_git_full_history() {
    let r = branchy_history(300, 5, 3, false);
    for path in [
        "dir1",
        "dir1/file2.txt",
        "dir2/file0.txt",
        "missing.txt",
        "dir0/file9.txt",
    ] {
        let q = LogQuery {
            path: Some(path.into()),
            ..Default::default()
        };
        let spec = format!(":(literal){path}");
        assert_eq!(
            search_rows(&r, &q),
            git_rows(&r, &["--full-history"], &[&spec]),
            "{path}"
        );
    }
    // Spelling variants mean the same path.
    let q = |p: &str| LogQuery {
        path: Some(p.into()),
        ..Default::default()
    };
    assert_eq!(search_rows(&r, &q("./dir1/")), search_rows(&r, &q("dir1")));
    assert_eq!(
        search_rows(&r, &q("dir1\\file2.txt")),
        search_rows(&r, &q("dir1/file2.txt"))
    );

    // Merges that brought a change in are listed, as with --full-history.
    let m = merges();
    for path in ["a.txt", "b.txt", "release.txt", "main.txt"] {
        assert_eq!(
            search_rows(&m, &q(path)),
            git_rows(&m, &["--full-history"], &[path]),
            "{path}"
        );
    }
}

#[test]
fn search_dates_and_branch_match_git() {
    let r = branchy_history(200, 4, 2, false);
    // Fixture commit i is at EPOCH + 60 i.
    let since = 1_577_836_800 + 60 * 50;
    let until = 1_577_836_800 + 60 * 120;
    let q = LogQuery {
        since: Some(since),
        until: Some(until),
        ..Default::default()
    };
    let s = format!("--since=@{since}");
    let u = format!("--until=@{until}");
    let got = search_rows(&r, &q);
    assert_eq!(got.len(), 71, "both ends are inclusive");
    assert_eq!(got, git_rows(&r, &[&s, &u], &[]));

    let q = LogQuery {
        branch: Some("refs/heads/b2".into()),
        path: Some("dir0".into()),
        ..Default::default()
    };
    assert_eq!(
        search_rows(&r, &q),
        git_rows(&r, &["--full-history", "refs/heads/b2"], &["dir0"])
    );
    let q = LogQuery {
        branch: Some("refs/heads/none".into()),
        ..Default::default()
    };
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    assert!(search(&g, &q, || false).is_err());
}

#[test]
fn search_rejects_bad_input_and_can_be_cancelled() {
    let r = linear(3);
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    for bad in ["", "/", "a/../b", "./", "a//b", "a/./b"] {
        assert!(clean_path(bad).is_err(), "{bad:?}");
    }
    let q = LogQuery {
        message: Some("two\nlines".into()),
        ..Default::default()
    };
    assert!(search(&g, &q, || false).is_err());
    let q = LogQuery {
        message: Some("x".repeat(1001)),
        ..Default::default()
    };
    assert!(search(&g, &q, || false).is_err());
    // A regex-looking pattern is literal text.
    let q = LogQuery {
        message: Some("(a+)+$".into()),
        ..Default::default()
    };
    assert!(search(&g, &q, || false).unwrap().rows.is_empty());
    let err = search(&g, &LogQuery::default(), || true).unwrap_err();
    assert_eq!(err.kind, crate::ipc::ErrorKind::Cancelled);
    // No filters: every row.
    assert_eq!(
        search(&g, &LogQuery::default(), || false).unwrap().rows,
        vec![0, 1, 2]
    );
}

// ---- commit detail, diff and blame against git ----

use super::diff::{
    FileStatus, Hunk, LineKind, blame, commit_detail, file_diff, hunks, parse_commit,
};

/// A commit touching every kind of change: edit, add, delete, rename with an edit, binary,
/// executable bit, symlink, file without a final newline.
fn changes_repo() -> (FixtureRepo, String) {
    let r = FixtureRepo::new();
    let body: String = (1..=40)
        .map(|i| format!("line {i} of the story\n"))
        .collect();
    r.write("story.txt", &body);
    r.write("gone.txt", "bye\n");
    r.write(
        "old/name.txt",
        (1..=30)
            .map(|i| format!("stable text {i}\n"))
            .collect::<String>(),
    );
    r.write("image.bin", [0u8, 1, 2, 3, 0, 255]);
    r.write("run.sh", "echo hi\n");
    r.write("tail.txt", "a\nb\nc");
    r.commit_all("base");

    let edited = body
        .replace("line 5 of the story", "line 5 of the new story")
        .replace("line 30 of the story\n", "")
        + "an added last line\n";
    r.write("story.txt", &edited);
    r.remove("gone.txt");
    r.write("fresh.txt", "brand new\nfile\n");
    r.remove("old/name.txt");
    r.write(
        "new/name.txt",
        (1..=30)
            .map(|i| format!("stable text {i}\n"))
            .collect::<String>()
            + "plus one\n",
    );
    r.write("image.bin", [0u8, 9, 9, 9]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = r.path().join("run.sh");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    r.git(&["update-index", "--chmod=+x", "run.sh"]);
    r.write("tail.txt", "a\nB\nc");
    #[cfg(unix)]
    std::os::unix::fs::symlink("story.txt", r.path().join("link")).unwrap();
    let c = r.commit_all("all kinds of change");
    (r, c)
}

fn open_repo(r: &FixtureRepo) -> gix::Repository {
    crate::git::state::open(r.path()).unwrap()
}

#[test]
fn commit_detail_matches_git_numstat_and_name_status() {
    let (r, c) = changes_repo();
    let d = commit_detail(&open_repo(&r), parse_commit(&c).unwrap(), None).unwrap();
    assert_eq!(d.subject, "all kinds of change");
    assert_eq!(d.parents, vec![r.rev("HEAD~1")]);
    assert_eq!(d.compared_parent, Some(0));
    assert_eq!(d.author.name, "Fixture Author");
    assert_eq!(d.committer.email, "committer@example.invalid");
    assert!(!d.files_truncated && !d.stats_skipped);

    // git: "<add>\t<del>\t<path>" (or "-\t-" for binary), renames as "old => new" with -M.
    let numstat = r.git(&["diff", "--numstat", "-M", "HEAD~1", "HEAD"]);
    let name_status = r.git(&["diff", "--name-status", "-M", "HEAD~1", "HEAD"]);
    let mut want_status = Vec::new();
    for line in name_status.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let st = match &f[0][..1] {
            "A" => FileStatus::Added,
            "D" => FileStatus::Deleted,
            "M" => FileStatus::Modified,
            "R" => FileStatus::Renamed,
            "T" => FileStatus::TypeChanged,
            other => panic!("unexpected status {other}"),
        };
        let (path, old) = if f.len() == 3 {
            (f[2], Some(f[1].to_string()))
        } else {
            (f[1], None)
        };
        want_status.push((path.to_string(), old, st));
    }
    want_status.sort();
    let mut got_status: Vec<_> = d
        .files
        .iter()
        .map(|f| (f.path.clone(), f.old_path.clone(), f.status))
        .collect();
    got_status.sort();
    assert_eq!(got_status, want_status);

    let renamed = d
        .files
        .iter()
        .find(|f| f.status == FileStatus::Renamed)
        .unwrap();
    assert!(renamed.similarity.unwrap() >= 90, "{renamed:?}");
    for line in numstat.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let file = if f[2].contains(" => ") {
            renamed
        } else {
            d.files.iter().find(|x| x.path == f[2]).unwrap()
        };
        if f[0] == "-" {
            assert!(file.binary, "{f:?}");
            assert_eq!(file.additions, None);
        } else {
            assert_eq!(file.additions, Some(f[0].parse().unwrap()), "{f:?}");
            assert_eq!(file.deletions, Some(f[1].parse().unwrap()), "{f:?}");
        }
    }
}

#[test]
fn commit_detail_of_root_and_merge_commits() {
    let r = merges();
    let repo = open_repo(&r);
    let root = r.git(&["rev-list", "--max-parents=0", "HEAD"]);
    let d = commit_detail(&repo, parse_commit(&root).unwrap(), None).unwrap();
    assert_eq!(d.compared_parent, None);
    assert!(d.files.iter().all(|f| f.status == FileStatus::Added));
    assert_eq!(
        d.files.len(),
        r.git(&["show", "--format=", "--name-only", &root])
            .lines()
            .count()
    );

    let merge = r.rev("main");
    let first = commit_detail(&repo, parse_commit(&merge).unwrap(), None).unwrap();
    let second = commit_detail(&repo, parse_commit(&merge).unwrap(), Some(1)).unwrap();
    let names =
        |d: &super::diff::CommitDetail| d.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
    let git_names = |p: &str| {
        r.git(&["diff", "--name-only", &format!("{merge}^{p}"), &merge])
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&first), git_names("1"));
    assert_eq!(names(&second), git_names("2"));
    assert!(commit_detail(&repo, parse_commit(&merge).unwrap(), Some(2)).is_err());
    assert!(parse_commit("HEAD").is_err());
    assert!(parse_commit(&"0".repeat(40)).is_ok());
    assert!(commit_detail(&repo, parse_commit(&"0".repeat(40)).unwrap(), None).is_err());
}

/// Our hunks in `git diff` text form, for comparison.
fn unified(hs: &[Hunk]) -> String {
    let range = |start: u32, n: u32| {
        if n == 1 {
            format!("{start}")
        } else {
            format!("{start},{n}")
        }
    };
    let mut s = String::new();
    for h in hs {
        s += &format!(
            "@@ -{} +{} @@\n",
            range(h.old_start, h.old_lines),
            range(h.new_start, h.new_lines)
        );
        for l in &h.lines {
            let c = match l.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Deleted => '-',
            };
            s += &format!("{c}{}\n", l.text);
            if l.no_newline {
                s += "\\ No newline at end of file\n";
            }
        }
    }
    s
}

/// `git diff` hunks only (from the first `@@`), with the function-context text after `@@` cut.
fn git_hunks(r: &FixtureRepo, args: &[&str]) -> String {
    let mut a = vec!["diff", "--no-color", "--no-ext-diff"];
    a.extend_from_slice(args);
    let out = r.git(&a);
    let mut s = String::new();
    let mut started = false;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            started = true;
            let end = rest.find("@@").unwrap() + 4;
            s += &line[..end];
            s.push('\n');
        } else if started {
            s += line;
            s.push('\n');
        }
    }
    s
}

#[test]
fn file_diffs_match_git_diff() {
    let (r, c) = changes_repo();
    let repo = open_repo(&r);
    let id = parse_commit(&c).unwrap();
    for path in ["story.txt", "fresh.txt", "gone.txt", "tail.txt"] {
        let d = file_diff(&repo, id, None, path, None, 3, false).unwrap();
        assert_eq!(
            unified(&d.hunks),
            git_hunks(&r, &["HEAD~1", "HEAD", "--", path]),
            "{path}"
        );
    }
    for ctx in [0u32, 1, 10] {
        let d = file_diff(&repo, id, None, "story.txt", None, ctx, false).unwrap();
        let u = format!("-U{ctx}");
        assert_eq!(
            unified(&d.hunks),
            git_hunks(&r, &[&u, "HEAD~1", "HEAD", "--", "story.txt"])
        );
    }
    let d = file_diff(
        &repo,
        id,
        None,
        "new/name.txt",
        Some("old/name.txt"),
        3,
        false,
    )
    .unwrap();
    assert_eq!(d.status, FileStatus::Renamed);
    assert_eq!(
        unified(&d.hunks),
        git_hunks(
            &r,
            &["-M", "HEAD~1", "HEAD", "--", "old/name.txt", "new/name.txt"]
        )
    );

    let d = file_diff(&repo, id, None, "image.bin", None, 3, false).unwrap();
    assert!(d.binary && d.hunks.is_empty());
    assert_eq!((d.old_size, d.new_size), (Some(6), Some(4)));

    let d = file_diff(&repo, id, None, "run.sh", None, 3, false).unwrap();
    assert_eq!(d.status, FileStatus::Modified);
    assert_eq!(
        (d.old_mode.as_deref(), d.new_mode.as_deref()),
        (Some("100644"), Some("100755"))
    );
    assert!(d.hunks.is_empty());

    #[cfg(unix)]
    {
        let d = file_diff(&repo, id, None, "link", None, 3, false).unwrap();
        assert_eq!(d.status, FileStatus::Added);
        assert_eq!(d.new_mode.as_deref(), Some("120000"));
        assert_eq!(d.hunks[0].lines[0].text, "story.txt");
    }

    assert!(file_diff(&repo, id, None, "nope.txt", None, 3, false).is_err());
    assert!(file_diff(&repo, id, None, "../etc/passwd", None, 3, false).is_err());
}

#[test]
fn work_stats_count_lines_like_git_numstat() {
    use super::diff::{WorkSide, work_stats};
    let r = FixtureRepo::new();
    let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    r.write("story.txt", &body);
    r.write("gone.txt", "a\nb\nc\n");
    r.write("bin.dat", "x\0y");
    r.commit_all("base");
    r.write("story.txt", body.replace("line 3\n", "three\nextra\n"));
    r.remove("gone.txt");
    r.write("bin.dat", "x\0z");
    r.write("new.txt", "fresh\nfile\n");
    let repo = open_repo(&r);
    let files = [
        ("story.txt".to_string(), WorkSide::Unstaged),
        ("gone.txt".to_string(), WorkSide::Unstaged),
        ("bin.dat".to_string(), WorkSide::Unstaged),
        ("new.txt".to_string(), WorkSide::Unstaged),
        ("nope.txt".to_string(), WorkSide::Staged),
    ];
    let got: Vec<_> = work_stats(&repo, &files)
        .into_iter()
        .map(|s| (s.path, s.additions, s.deletions))
        .collect();
    assert_eq!(
        got,
        vec![
            ("story.txt".to_string(), Some(2), Some(1)),
            ("gone.txt".to_string(), Some(0), Some(3)),
            ("bin.dat".to_string(), None, None),
            ("new.txt".to_string(), Some(2), Some(0)),
            ("nope.txt".to_string(), None, None),
        ]
    );
}

#[test]
fn work_diffs_match_git_diff() {
    use super::diff::{WorkSide, work_diff};
    let r = FixtureRepo::new();
    let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    r.write("story.txt", &body);
    r.write("gone.txt", "bye\n");
    r.write("staged-gone.txt", "bye too\n");
    r.write("crlf.txt", "one\ntwo\nthree\n");
    r.commit_all("base");

    // A staged edit, then a further unstaged edit on top of it.
    r.write("story.txt", body.replace("line 3\n", "line three\n"));
    r.git(&["add", "story.txt"]);
    r.write(
        "story.txt",
        body.replace("line 3\n", "line three\n")
            .replace("line 15\n", "line fifteen\n"),
    );
    r.remove("gone.txt");
    r.git(&["rm", "-q", "staged-gone.txt"]);
    r.write("new.txt", "fresh\nfile\n");
    r.write("added.txt", "staged new\n");
    r.git(&["add", "added.txt"]);
    let repo = open_repo(&r);

    let check = |path: &str, side: WorkSide, git: &[&str]| {
        let d = work_diff(&repo, path, side, 3, false).unwrap();
        assert_eq!(unified(&d.hunks), git_hunks(&r, git), "{path} {side:?}");
        d
    };
    let d = check(
        "story.txt",
        WorkSide::Staged,
        &["--cached", "--", "story.txt"],
    );
    assert_eq!(d.status, FileStatus::Modified);
    check("story.txt", WorkSide::Unstaged, &["--", "story.txt"]);
    let d = check("gone.txt", WorkSide::Unstaged, &["--", "gone.txt"]);
    assert_eq!(d.status, FileStatus::Deleted);
    let d = check(
        "staged-gone.txt",
        WorkSide::Staged,
        &["--cached", "--", "staged-gone.txt"],
    );
    assert_eq!(d.status, FileStatus::Deleted);
    let d = check(
        "added.txt",
        WorkSide::Staged,
        &["--cached", "--", "added.txt"],
    );
    assert_eq!(d.status, FileStatus::Added);
    // Untracked: compared with nothing, as `git diff --no-index /dev/null` would.
    let d = work_diff(&repo, "new.txt", WorkSide::Unstaged, 3, false).unwrap();
    assert_eq!(d.status, FileStatus::Added);
    assert_eq!(unified(&d.hunks), "@@ -0,0 +1,2 @@\n+fresh\n+file\n");
    assert_eq!((d.old_size, d.new_size), (None, Some(11)));

    // CRLF on disk against LF in the index shows only the real change.
    r.write("crlf.txt", "one\r\nTWO\r\nthree\r\n");
    let d = work_diff(&repo, "crlf.txt", WorkSide::Unstaged, 3, false).unwrap();
    assert_eq!(
        unified(&d.hunks),
        "@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n"
    );

    // No change on that side is an empty diff; a file on neither side is an error.
    let d = work_diff(&repo, "gone.txt", WorkSide::Staged, 3, false).unwrap();
    assert!(d.hunks.is_empty());
    assert!(work_diff(&repo, "nope.txt", WorkSide::Unstaged, 3, false).is_err());
    for bad in ["../outside.txt", ".git/config", "a//b", "/etc/passwd", ""] {
        assert!(
            work_diff(&repo, bad, WorkSide::Unstaged, 3, false).is_err(),
            "{bad:?}"
        );
    }

    // Large files need `force`; binary files have no hunks.
    r.write("big.txt", "x\n".repeat(600 * 1024));
    let d = work_diff(&repo, "big.txt", WorkSide::Unstaged, 3, false).unwrap();
    assert!(d.too_large && d.hunks.is_empty());
    assert_eq!(d.new_size, Some(1200 * 1024));
    let d = work_diff(&repo, "big.txt", WorkSide::Unstaged, 3, true).unwrap();
    assert!(!d.too_large && !d.hunks.is_empty());
    r.write("blob.bin", [0u8, 1, 2]);
    let d = work_diff(&repo, "blob.bin", WorkSide::Unstaged, 3, false).unwrap();
    assert!(d.binary && d.hunks.is_empty());

    // A path through a symbolic link is not followed out of the working tree.
    #[cfg(unix)]
    {
        let outside = r.sibling("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink(&outside, r.path().join("escape")).unwrap();
        let e = work_diff(&repo, "escape/secret.txt", WorkSide::Unstaged, 3, false).unwrap_err();
        assert!(e.message.contains("symbolic link"), "{}", e.message);
        // The link itself shows its target, like git.
        let d = work_diff(&repo, "escape", WorkSide::Unstaged, 3, false).unwrap();
        assert_eq!(d.new_mode.as_deref(), Some("120000"));
    }
}

#[test]
fn work_diff_of_a_conflicted_file_shows_the_markers() {
    use super::diff::{WorkSide, work_diff};
    let r = conflict();
    start_conflict(&r);
    let d = work_diff(&open_repo(&r), "conflict.txt", WorkSide::Unstaged, 3, false).unwrap();
    let added: Vec<&str> = d.hunks[0]
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.text.as_str())
        .collect();
    assert!(added.iter().any(|l| l.starts_with("<<<<<<<")), "{added:?}");
    assert!(added.contains(&"topic version"), "{added:?}");
}

#[test]
fn word_emphasis_never_covers_indentation() {
    // Re-indenting a line changes only whitespace: no pale block over it.
    let hs = hunks(
        "if (a) {\n    run();\n}\n",
        "if (a) {\n        run();\n}\n",
        3,
    );
    for l in hs[0].lines.iter().filter(|l| l.kind != LineKind::Context) {
        assert!(l.emphasis.is_empty(), "{:?} {:?}", l.text, l.emphasis);
    }
    // A real change next to new indentation is marked without the spaces around it.
    let hs = hunks("    foo(1);\n", "        foo(2);\n", 0);
    for l in hs[0].lines.iter().filter(|l| l.kind != LineKind::Context) {
        for r in &l.emphasis {
            let piece: String = l
                .text
                .encode_utf16()
                .skip(r[0] as usize)
                .take((r[1] - r[0]) as usize)
                .map(|u| u as u8 as char)
                .collect();
            assert_eq!(piece.trim(), piece, "{:?} {:?}", l.text, l.emphasis);
        }
    }
}

#[test]
fn word_emphasis_marks_only_changed_words() {
    let hs = hunks("let a = 1;\nkeep\n", "let b = 1;\nkeep\n", 3);
    let lines = &hs[0].lines;
    let del = lines.iter().find(|l| l.kind == LineKind::Deleted).unwrap();
    let add = lines.iter().find(|l| l.kind == LineKind::Added).unwrap();
    assert_eq!(del.emphasis, vec![[4, 5]]);
    assert_eq!(add.emphasis, vec![[4, 5]]);
    // Offsets are UTF-16: an astral character before the change counts as 2.
    let hs = hunks("😀 x\n", "😀 y\n", 0);
    let add = hs[0]
        .lines
        .iter()
        .find(|l| l.kind == LineKind::Added)
        .unwrap();
    assert_eq!(add.text, "😀 y");
    assert_eq!(add.emphasis, vec![[3, 4]]);
    // Pure additions carry no emphasis.
    let hs = hunks("", "new\n", 3);
    assert!(hs[0].lines[0].emphasis.is_empty());
    assert_eq!(
        (hs[0].old_start, hs[0].old_lines, hs[0].new_start),
        (0, 0, 1)
    );
}

#[test]
fn large_files_need_force_and_have_a_hard_limit() {
    let r = FixtureRepo::new();
    r.write("big.txt", "x\n".repeat(600 * 1024));
    let c = r.commit_all("big");
    let repo = open_repo(&r);
    let id = parse_commit(&c).unwrap();
    let d = file_diff(&repo, id, None, "big.txt", None, 3, false).unwrap();
    assert!(d.too_large && d.hunks.is_empty());
    let d = file_diff(&repo, id, None, "big.txt", None, 3, true).unwrap();
    assert!(!d.too_large);
    assert_eq!(d.hunks[0].new_lines, 600 * 1024);
    let detail = commit_detail(&repo, id, None).unwrap();
    assert_eq!(detail.files[0].additions, None, "no stats for large files");
}

#[test]
fn blame_matches_git_blame() {
    let r = linear(1);
    r.commit_file("poem.txt", "one\ntwo\nthree\n", "first draft");
    r.commit_file("poem.txt", "one\n2\nthree\nfour\n", "second draft");
    r.git(&["mv", "poem.txt", "verse.txt"]);
    r.commit_all("rename");
    r.commit_file("verse.txt", "zero\none\n2\nthree\nfour\n", "prepend");
    let repo = open_repo(&r);
    let head = r.head();
    let b = blame(&repo, parse_commit(&head).unwrap(), "verse.txt").unwrap();
    assert_eq!(b.lines, vec!["zero", "one", "2", "three", "four"]);

    // Expand our hunks to a commit per line and compare with git's.
    let mut ours = vec![String::new(); b.lines.len()];
    for h in &b.hunks {
        for i in 0..h.lines {
            ours[(h.start + i - 1) as usize] = h.commit.clone();
        }
    }
    let theirs: Vec<String> = r
        .git(&["blame", "-l", "-s", "--", "verse.txt"])
        .lines()
        .map(|l| l.split_whitespace().next().unwrap().to_string())
        .collect();
    assert_eq!(ours, theirs);
    let first = b.hunks.iter().find(|h| h.subject == "first draft").unwrap();
    assert_eq!(first.source_path.as_deref(), Some("poem.txt"));

    assert!(blame(&repo, parse_commit(&head).unwrap(), "missing.txt").is_err());
    assert!(blame(&repo, parse_commit(&head).unwrap(), "../verse.txt").is_err());
}

/// Resident memory in bytes (Linux only; 0 elsewhere).
fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096)
}

/// Graph build, paging and search times on large histories. Numbers go in ACTIONPLAN 2.8.
/// Run with `cargo test --release --lib -- --ignored --nocapture history_benchmark`.
#[test]
#[ignore = "slow benchmark; run on demand"]
fn history_benchmark() {
    use std::time::Instant;
    let sizes: Vec<usize> = std::env::var("PM_BENCH_SIZES")
        .ok()
        .map(|s| s.split(',').map(|n| n.trim().parse().unwrap()).collect())
        .unwrap_or_else(|| vec![10_000, 100_000, 500_000]);
    for n in sizes {
        let t = Instant::now();
        let r = branchy_history(n, 8, 7, false);
        let generate = t.elapsed();
        let before = rss();
        let t = Instant::now();
        let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
        let built = t.elapsed();
        let mem = rss().saturating_sub(before);
        let t = Instant::now();
        let first = g.rows(0, 200).unwrap();
        let first_rows = t.elapsed();
        let t = Instant::now();
        let _ = g.rows(n as u32 / 2, 200).unwrap();
        let mid_rows = t.elapsed();
        let t = Instant::now();
        let key = current_key(r.path(), &GraphOptions::default()).unwrap();
        let key_time = t.elapsed();
        assert!(key == g.key);
        let t = Instant::now();
        let hits = search(
            &g,
            &LogQuery {
                message: Some("commit 7".into()),
                ..Default::default()
            },
            || false,
        )
        .unwrap();
        let msg_search = t.elapsed();
        let t = Instant::now();
        let _ = search(
            &g,
            &LogQuery {
                path: Some("dir1".into()),
                ..Default::default()
            },
            || false,
        )
        .unwrap();
        let path_search = t.elapsed();
        println!(
            "n={n} lanes={} generate={generate:.2?} build={built:.2?} graph_rss≈{} MiB \
             first200={first_rows:.2?} mid200={mid_rows:.2?} key={key_time:.2?} \
             search_msg={msg_search:.2?} ({} hits) search_path={path_search:.2?}",
            g.layout.max_lanes,
            mem / (1024 * 1024),
            hits.rows.len(),
        );
        assert_eq!(first.len(), 200);
    }
}

fn branched_at(r: &FixtureRepo) -> Vec<(String, Vec<String>)> {
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    g.rows(0, g.len() as u32)
        .unwrap()
        .into_iter()
        .filter(|row| !row.branched.is_empty())
        .map(|row| (row.id, row.branched.into_iter().map(|b| b.name).collect()))
        .collect()
}

#[test]
fn a_branch_made_from_a_branch_that_has_not_moved_marks_where_it_branched() {
    let r = linear(2);
    r.git(&["branch", "-M", "main"]);
    let fork = r.rev("HEAD");
    r.git(&["switch", "-q", "-c", "dev"]);
    r.commit_file("a.txt", "a\n", "on dev 1");
    r.commit_file("b.txt", "b\n", "on dev 2");
    assert_eq!(branched_at(&r), vec![(fork, vec!["dev".to_string()])]);
}

#[test]
fn no_branch_point_when_the_branches_share_a_tip_or_never_meet() {
    // Two branches on one commit: nothing branched yet.
    let r = linear(2);
    r.git(&["branch", "other"]);
    assert_eq!(branched_at(&r), vec![]);
    // A branch with nothing else below it.
    let r = linear(3);
    assert_eq!(branched_at(&r), vec![]);
}

#[test]
fn a_remote_branch_of_the_same_name_is_not_another_branch() {
    let r = linear(3);
    r.git(&["branch", "-M", "main"]);
    // origin/main at HEAD~1 is main's own remote copy, so main branched from nowhere.
    r.git(&["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
    assert_eq!(branched_at(&r), vec![]);
    // origin/other at HEAD~1 is another branch's: main branched from there.
    r.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    r.git(&["update-ref", "refs/remotes/origin/other", "HEAD~1"]);
    assert_eq!(
        branched_at(&r),
        vec![(r.rev("HEAD~1"), vec!["main".to_string()])]
    );
}

#[test]
fn no_branch_point_once_the_branches_have_diverged() {
    // The graph draws two lanes there, so no extra mark is needed.
    let r = linear(2);
    r.git(&["branch", "-M", "main"]);
    r.git(&["switch", "-q", "-c", "dev"]);
    r.commit_file("a.txt", "a\n", "on dev");
    r.git(&["switch", "-q", "main"]);
    r.commit_file("m.txt", "m\n", "on main");
    assert_eq!(branched_at(&r), vec![]);
}

#[test]
fn a_branch_point_gives_the_branch_its_own_colour_on_the_shared_lane() {
    let r = linear(2);
    r.git(&["branch", "-M", "main"]);
    r.git(&["switch", "-q", "-c", "dev"]);
    r.commit_file("a.txt", "a\n", "on dev 1");
    r.commit_file("b.txt", "b\n", "on dev 2");
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    let rows = g.rows(0, g.len() as u32).unwrap();
    // dev 2, dev 1, main (the branch point), first commit.
    assert!(rows.iter().all(|row| row.lane == 0), "one lane");
    assert_eq!(rows[0].color, rows[1].color, "dev's commits share a colour");
    assert_ne!(rows[1].color, rows[2].color, "main has another one");
    assert_eq!(rows[2].color, 0, "main is colour 0");
    assert_eq!(rows[3].color, 0, "and so is its history below");
    assert_ne!(rows[0].color, 0, "no other branch takes colour 0");
    assert_eq!(
        rows[2].branched[0].color, rows[0].color,
        "the tag wears dev's"
    );
    // The line between dev's last commit and the branch point is still dev's.
    assert_eq!(rows[1].edges[0].color, rows[0].color);
}

#[test]
fn the_main_branch_is_colour_zero_and_others_never_are() {
    let r = linear(2);
    r.git(&["branch", "-M", "main"]);
    // Several branches, each with its own commit, and a merge back into main.
    for name in ["a", "b", "c"] {
        r.git(&["switch", "-q", "-c", name, "main"]);
        r.commit_file(&format!("{name}.txt"), "x\n", &format!("on {name}"));
    }
    r.git(&["switch", "-q", "main"]);
    r.commit_file("m.txt", "m\n", "on main");
    r.merge(&["a"], "merge a");
    let g = build(r.path(), &GraphOptions::default(), 1).unwrap();
    let main_ids: Vec<String> = r
        .git(&["rev-list", "--first-parent", "main"])
        .lines()
        .map(str::to_string)
        .collect();
    for row in g.rows(0, g.len() as u32).unwrap() {
        if main_ids.contains(&row.id) {
            assert_eq!(row.color, 0, "{}", row.subject);
        } else {
            assert_ne!(row.color, 0, "{}", row.subject);
        }
    }
}
