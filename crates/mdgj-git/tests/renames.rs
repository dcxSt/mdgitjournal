//! Renames are followed: an entry retitled on one device (its file renamed) and edited or
//! retitled on the other is one file changed on both sides, with Merge offered, never a deletion
//! and a new file, nor two entries made behind the user's back.

mod common;

use std::fs;

use common::{merged, no_conflict_markers, overlap, resolved, two_devices, Device};
use mdgj_git::{ChangeKind, Choice, OverlapKind, PullPlan, Resolution};

const OLD: &str = "2026-11-16-1402-run.md";
const NEW: &str = "2026-11-16-1402-long-run.md";
const OTHER: &str = "2026-11-16-1402-evening-run.md";

fn body() -> String {
    "# Run\n\nOut along the river, five miles, easy pace.\n\nCold, but the sun came out.\n\n\
     Legs fine afterwards.\n"
        .to_owned()
}

fn edited(text: &str, line: &str) -> String {
    format!("{text}\n{line}\n")
}

fn rename(d: &Device, from: &str, to: &str) {
    fs::rename(d.root().join(from), d.root().join(to)).unwrap();
}

#[test]
fn renamed_on_github_and_edited_here_merges_into_the_new_name() {
    let setup = || {
        let (remote, a, b) = two_devices(&[(OLD, &body())]);
        rename(&a, OLD, NEW);
        a.push("Retitle");
        b.write(OLD, edited(&body(), "Edited on B."));
        let preview = b.pull_review();
        (remote, a, b, preview)
    };
    let (_r, _a, b, preview) = setup();
    assert_eq!(preview.plan, PullPlan::FastForward);
    assert_eq!(preview.auto, [], "the new name isn't a new file");
    assert_eq!(preview.overlaps.len(), 1);
    let found = overlap(&preview, NEW);
    assert_eq!(found.kind, OverlapKind::ModifiedRenamed);
    assert_eq!(found.mine_path.as_deref(), Some(OLD));
    assert_eq!(found.theirs_path.as_deref(), Some(NEW));
    assert_eq!(
        found.choices,
        [Choice::Mine, Choice::Theirs, Choice::Both, Choice::Merged]
    );
    assert_eq!(
        found.copy.as_deref(),
        Some(NEW),
        "Keep both: each under its name"
    );
    let suggestion = found.suggestion.clone().unwrap();
    assert!(suggestion.clean);
    let text = suggestion.text().unwrap();
    assert_eq!(text, edited(&body(), "Edited on B."));

    // Merge: B's edit, under GitHub's new name.
    b.apply(&preview, &[merged(NEW, &text)]);
    assert!(!b.exists(OLD));
    assert_eq!(b.text(NEW), edited(&body(), "Edited on B."));
    assert_eq!(b.changes(), [(NEW.into(), ChangeKind::Modified)]);
    no_conflict_markers(&b);

    // Keep mine: the file stays as it is here; GitHub's rename isn't taken.
    let (_r, _a, b, preview) = setup();
    b.apply(&preview, &[resolved(NEW, Resolution::Mine)]);
    assert_eq!(b.files(), [OLD]);
    assert_eq!(b.text(OLD), edited(&body(), "Edited on B."));

    // Keep GitHub's: the rename, without B's edit.
    let (_r, _a, b, preview) = setup();
    b.apply(&preview, &[resolved(NEW, Resolution::Theirs)]);
    assert_eq!(b.files(), [NEW]);
    assert_eq!(b.text(NEW), body());
    assert_eq!(b.changes(), []);

    // Keep both: each version under its own name.
    let (_r, _a, b, preview) = setup();
    b.apply(&preview, &[resolved(NEW, Resolution::Both)]);
    assert_eq!(b.text(OLD), edited(&body(), "Edited on B."));
    assert_eq!(b.text(NEW), body());
}

#[test]
fn renamed_and_committed_here_while_github_edited_keeps_both_in_the_merge() {
    let (_remote, a, b) = two_devices(&[(OLD, &body())]);
    a.write(OLD, edited(&body(), "Edited on A."));
    a.push("Edit");
    rename(&b, OLD, NEW);
    b.commit("Retitle");

    let preview = b.pull_review();
    assert_eq!(preview.plan, PullPlan::Merge);
    let found = overlap(&preview, NEW);
    assert_eq!(found.kind, OverlapKind::RenamedModified);
    assert_eq!(found.mine_path.as_deref(), Some(NEW));
    assert_eq!(found.theirs_path.as_deref(), Some(OLD));
    assert!(found.committed);
    let text = found.suggestion.as_ref().unwrap().text().unwrap();
    assert_eq!(text, edited(&body(), "Edited on A."));
    let summary = b.apply(&preview, &[merged(NEW, &text)]);
    assert_eq!(summary.resolved[0].kind, OverlapKind::RenamedModified);
    assert_eq!(b.files(), [NEW]);
    assert_eq!(b.text(NEW), text);
    assert_eq!(b.committed(NEW), Some(text.clone().into_bytes()));
    assert_eq!(b.committed(OLD), None);
    assert_eq!(b.changes(), []);

    // A gets the rename with its own edit kept, without a question.
    assert!(matches!(
        b.repo.push(None),
        Ok(mdgj_git::PushOutcome::Pushed { .. })
    ));
    let summary = a.pull_applied();
    assert_eq!(summary.plan, PullPlan::FastForward);
    assert_eq!(a.files(), [NEW]);
    assert_eq!(a.text(NEW), text);
}

#[test]
fn retitled_differently_on_both_devices_waits_for_the_user() {
    for commit in [false, true] {
        let (_remote, a, b) = two_devices(&[(OLD, &body())]);
        rename(&a, OLD, NEW);
        a.write(NEW, edited(&body(), "A edit."));
        a.push("A");
        rename(&b, OLD, OTHER);
        b.write(OTHER, format!("B edit.\n{}", body()));
        if commit {
            b.commit("B");
        }

        let preview = b.pull_review();
        assert!(!preview.automatic, "commit: {commit}");
        assert_eq!(preview.auto, []);
        let found = overlap(&preview, OTHER);
        assert_eq!(found.kind, OverlapKind::BothRenamed);
        assert_eq!(found.mine_path.as_deref(), Some(OTHER));
        assert_eq!(found.theirs_path.as_deref(), Some(NEW));
        assert_eq!(found.copy.as_deref(), Some(NEW));
        let text = found.suggestion.as_ref().unwrap().text().unwrap();
        assert_eq!(text, format!("B edit.\n{}", edited(&body(), "A edit.")));

        b.apply(&preview, &[merged(OTHER, &text)]);
        assert_eq!(b.files(), [OTHER], "one entry, under my name");
        assert_eq!(b.text(OTHER), text);
        if commit {
            assert_eq!(b.committed(OTHER), Some(text.into_bytes()));
            assert_eq!(b.committed(NEW), None);
        }
    }
}

#[test]
fn retitled_alike_on_both_devices_merges_against_the_old_text() {
    let (_remote, a, b) = two_devices(&[(OLD, &body())]);
    rename(&a, OLD, NEW);
    a.write(NEW, edited(&body(), "A edit."));
    a.push("A");
    rename(&b, OLD, NEW);
    b.write(NEW, format!("B edit.\n{}", body()));

    let preview = b.pull_review();
    let found = overlap(&preview, NEW);
    assert_eq!(found.kind, OverlapKind::BothModified);
    let suggestion = found.suggestion.clone().unwrap();
    assert!(suggestion.clean, "the base is the old file's text");
    let text = suggestion.text().unwrap();
    b.apply(&preview, &[merged(NEW, &text)]);
    assert_eq!(b.files(), [NEW]);
    assert_eq!(
        b.text(NEW),
        format!("B edit.\n{}", edited(&body(), "A edit."))
    );
}

#[test]
fn a_rename_against_a_deletion_asks() {
    // Deleted here, renamed on GitHub.
    let (_remote, a, b) = two_devices(&[(OLD, &body())]);
    rename(&a, OLD, NEW);
    a.push("Retitle");
    b.remove(OLD);
    let preview = b.pull_review();
    let found = overlap(&preview, NEW);
    assert_eq!(found.kind, OverlapKind::DeletedRenamed);
    assert_eq!(found.choices, [Choice::Mine, Choice::Theirs]);
    assert_eq!(found.mine_path, None);
    assert_eq!(preview.auto, []);
    b.apply(&preview, &[resolved(NEW, Resolution::Mine)]);
    assert_eq!(b.files(), Vec::<String>::new(), "it stays deleted");
    b.repo.undo_last_sync().unwrap();
    let preview = b.repo.preview_pull().unwrap().unwrap();
    b.apply(&preview, &[resolved(NEW, Resolution::Theirs)]);
    assert_eq!(b.files(), [NEW]);

    // Renamed here, deleted on GitHub.
    let (_remote, a, b) = two_devices(&[(OLD, &body())]);
    a.remove(OLD);
    a.push("Delete");
    rename(&b, OLD, NEW);
    let preview = b.pull_review();
    let found = overlap(&preview, NEW);
    assert_eq!(found.kind, OverlapKind::RenamedDeleted);
    assert_eq!(found.theirs_path, None);
    b.apply(&preview, &[resolved(NEW, Resolution::Theirs)]);
    assert_eq!(b.files(), Vec::<String>::new(), "deleted, as on GitHub");
}

#[test]
fn a_rename_on_one_side_only_still_flows() {
    let (_remote, a, b) = two_devices(&[(OLD, &body()), ("x.md", "x\n")]);
    rename(&a, OLD, NEW);
    a.push("Retitle");
    b.write("x.md", "x, edited\n");
    let summary = b.pull_applied();
    assert_eq!(b.text(NEW), body());
    assert!(!b.exists(OLD));
    assert_eq!(b.text("x.md"), "x, edited\n");
    assert!(summary
        .files
        .iter()
        .any(|f| f.kind == ChangeKind::Renamed && f.path == NEW));
}
