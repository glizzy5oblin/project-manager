use super::*;
use crate::testing::FixtureRepo;

fn merge_stops(r: &FixtureRepo) {
    // Two branches that change the same line differently.
    r.write("a.txt", "one\ntwo\nthree\n");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "base"]);
    r.git(&["checkout", "-q", "-b", "other"]);
    r.write("a.txt", "one\nTWO from other\nthree\n");
    r.git(&["commit", "-qam", "other change"]);
    r.git(&["checkout", "-q", "-"]);
    r.write("a.txt", "one\ntwo from main\nthree\n");
    r.git(&["commit", "-qam", "main change"]);
    let _ = GitCommand::new(r.path())
        .args(["merge", "-q", "other"])
        .run();
}

#[test]
fn both_changed_the_same_line() {
    let r = FixtureRepo::new();
    merge_stops(&r);
    let info = describe(r.path(), "a.txt").unwrap();
    assert_eq!(info.kind, ConflictKind::BothModified);
    assert_eq!(info.theirs_name.as_deref(), Some("other"));
    assert!(info.ours_name.is_some());
    assert_eq!(info.regions.len(), 1);
    let region = &info.regions[0];
    assert_eq!(region.ours, ["two from main"]);
    assert_eq!(region.theirs, ["TWO from other"]);
    assert_eq!((region.start_line, region.end_line), (2, 6));
}

#[test]
fn deleted_on_one_side() {
    let r = FixtureRepo::new();
    r.write("a.txt", "x\n");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "base"]);
    r.git(&["checkout", "-q", "-b", "other"]);
    r.git(&["rm", "-q", "a.txt"]);
    r.git(&["commit", "-q", "-m", "delete"]);
    r.git(&["checkout", "-q", "-"]);
    r.write("a.txt", "x\nchanged\n");
    r.git(&["commit", "-qam", "change"]);
    let _ = GitCommand::new(r.path())
        .args(["merge", "-q", "other"])
        .run();
    let info = describe(r.path(), "a.txt").unwrap();
    assert_eq!(info.kind, ConflictKind::DeletedByThem);
    assert!(info.regions.is_empty());
}

#[test]
fn a_file_that_is_not_in_conflict_is_resolved() {
    let r = FixtureRepo::new();
    r.write("a.txt", "x\n");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-q", "-m", "base"]);
    assert_eq!(
        describe(r.path(), "a.txt").unwrap().kind,
        ConflictKind::Resolved
    );
}

#[test]
fn markers_with_a_base_section_and_several_regions_are_read() {
    let text = "keep\n<<<<<<< HEAD\nmine\n||||||| base\norig\n=======\ntheirs\n>>>>>>> feat\nmid\n<<<<<<< HEAD\n=======\nadded\n>>>>>>> feat\n";
    let (regions, more) = parse_regions(text);
    assert!(!more);
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].base.as_deref(), Some(&["orig".to_string()][..]));
    assert_eq!((regions[0].start_line, regions[0].end_line), (2, 8));
    assert!(regions[1].ours.is_empty());
    assert_eq!(regions[1].theirs, ["added"]);
}

#[test]
fn a_long_side_is_cut_and_says_so() {
    let mut text = String::from("<<<<<<< HEAD\n");
    for i in 0..100 {
        text.push_str(&format!("line {i}\n"));
    }
    text.push_str("=======\nx\n>>>>>>> b\n");
    let (regions, _) = parse_regions(&text);
    assert_eq!(regions[0].ours.len(), MAX_LINES);
    assert!(regions[0].truncated);
}
