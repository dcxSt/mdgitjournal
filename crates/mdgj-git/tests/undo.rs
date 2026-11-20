//! Undo: every applied pull can be put back exactly, unless that would lose something.

mod common;

use common::{author, merged, resolved, two_devices, Device, Remote};
use mdgj_git::{ChangeKind, Error, PullOutcome, Resolution};

const UNDO_REF: &str = "refs/mdgitjournal/undo";

fn record_exists(d: &Device) -> bool {
    d.root().join(".git/mdgitjournal/undo.json").exists()
}

// 3. Undo of a one-sided pull

#[test]
fn undo_puts_back_the_branch_the_index_and_every_touched_file() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n"), ("gone.md", "remove me\n")]);
    a.write("x.md", "x, edited on a\n");
    a.write("assets/new-1.jpg", [0xff, 0xd8, 0xff]);
    a.remove("gone.md");
    a.push("A");
    b.write("draft.md", "untracked draft\n");
    b.write("x.md", "x\n"); // unchanged bytes, rewritten: no edit
    b.repo.fetch(None).unwrap();
    let before = b.state();
    let behind = b.repo.sync_state().unwrap().behind;
    assert_eq!(behind, 1);

    b.pull_applied();
    assert!(record_exists(&b));
    assert!(b.git().find_reference(UNDO_REF).is_ok());
    assert!(!b.exists("gone.md"));

    let mut restored = b.repo.undo_last_sync().unwrap();
    restored.sort();
    assert_eq!(restored, ["assets/new-1.jpg", "gone.md", "x.md"]);
    // (The record of B's first pull, at connect, was replaced: Undo is one level.)
    assert_eq!(
        b.state(),
        before.without_undo(),
        "branch, index bytes and files as before"
    );
    assert!(!b.exists("assets"), "the folder the pull made is gone too");
    assert_eq!(b.text("gone.md"), "remove me\n");
    assert!(b.git().find_reference(UNDO_REF).is_err());
    assert!(!record_exists(&b));
    let state = b.repo.sync_state().unwrap();
    assert_eq!((state.behind, state.last_sync), (behind, None));
    assert_eq!(
        b.repo.undo_last_sync(),
        Err(Error::Invalid("There is no pull to undo.".into()))
    );

    // And the pull can simply be made again.
    b.pull_applied();
    assert_eq!(b.head(), a.head());
}

// 4. Undo of a rebase

#[test]
fn undo_of_a_rebase_restores_the_original_commits() {
    let (_remote, a, b) = two_devices(&[("base.md", "base\n")]);
    a.write("a.md", "a\n");
    a.push("A");
    b.write("b1.md", "b1\n");
    let b1 = b.commit("b1");
    b.write("b2.md", "b2\n");
    let b2 = b.commit("b2");
    b.repo.fetch(None).unwrap();
    let before = b.state();

    let summary = b.pull_applied();
    assert_eq!(summary.rebased, 2);
    assert_ne!(b.head(), b2);
    b.repo.undo_last_sync().unwrap();
    assert_eq!(b.head(), b2);
    assert_eq!(b.git().find_commit(b2).unwrap().parent_id(0).unwrap(), b1);
    assert_eq!(b.state(), before);
    assert!(!b.exists("a.md"));
}

#[test]
fn undo_of_a_reviewed_pull_brings_back_my_uncommitted_text_even_after_gc() {
    let (_remote, a, b) = two_devices(&[("entry.md", "base\n")]);
    a.write("entry.md", "GitHub's\n");
    a.push("A");
    b.write("entry.md", "mine, never committed\n");
    let preview = b.pull_review();
    let before = b.state().without_undo();
    let copy = common::overlap(&preview, "entry.md").copy.clone().unwrap();
    b.apply(&preview, &[resolved("entry.md", Resolution::Both)]);
    assert_eq!(b.text(&copy), "GitHub's\n");
    // Resolved the other way after all: GitHub's text replaces mine.
    b.repo.undo_last_sync().unwrap();
    assert_eq!(b.state(), before);
    let preview = b.repo.preview_pull().unwrap().unwrap();
    b.apply(&preview, &[resolved("entry.md", Resolution::Theirs)]);
    assert_eq!(b.text("entry.md"), "GitHub's\n");

    // Mine exists only in the undo record now; `git gc` keeps it.
    if b.cli_ok(&["gc", "--prune=now", "--quiet"]).is_some() {
        assert!(b.cli_ok(&["fsck", "--no-progress"]).is_some());
    }
    b.repo.undo_last_sync().unwrap();
    assert_eq!(b.text("entry.md"), "mine, never committed\n");
    assert_eq!(b.state(), before);
}

#[test]
fn undo_of_a_merge_commit_and_of_a_merged_text() {
    let (_remote, a, b) = two_devices(&[("entry.md", "one\ntwo\nthree\n")]);
    a.write("entry.md", "ONE\ntwo\nthree\n");
    a.push("A");
    b.write("entry.md", "one\ntwo\nTHREE\n");
    let local = b.commit("B");
    b.write("other.md", "uncommitted\n");
    let preview = b.pull_review();
    let before = b.state();
    b.apply(&preview, &[merged("entry.md", "ONE\ntwo\nTHREE\n")]);
    assert_eq!(b.history()[0].1, 2);
    b.repo.undo_last_sync().unwrap();
    assert_eq!(b.head(), local);
    assert_eq!(b.state(), before);
}

#[test]
fn undo_of_a_pull_onto_an_unborn_branch_makes_it_unborn_again() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("a.md", "a\n");
    a.commit("A");
    a.repo.connect(&remote.url, None, &author()).unwrap();

    let b = Device::new();
    b.write("draft.md", "draft\n");
    let PullOutcome::Applied { .. } = b.repo.connect(&remote.url, None, &author()).unwrap() else {
        panic!("expected the pull to apply");
    };
    assert_eq!(b.files(), ["a.md", "draft.md"]);
    b.repo.undo_last_sync().unwrap();
    assert!(b.git().head().is_err(), "unborn again");
    assert_eq!(b.repo.sync_state().unwrap().branch, "main");
    assert_eq!(b.files(), ["draft.md"]);
    assert_eq!(b.changes(), [("draft.md".into(), ChangeKind::Added)]);
    assert!(!b.root().join(".git/index").exists());
}

// 5. Undo refused

#[test]
fn undo_refuses_to_overwrite_a_file_edited_since() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n"), ("y.md", "y\n")]);
    a.write("x.md", "x, from GitHub\n");
    a.write("y.md", "y, from GitHub\n");
    a.push("A");
    b.pull_applied();
    b.write("x.md", "typed after the pull\n");
    b.write("y.md", "and this too\n");
    let before = b.state();
    let result = b.repo.undo_last_sync();
    let Err(Error::UndoRefused(message)) = result else {
        panic!("{result:?}");
    };
    assert_eq!(
        message,
        "You've changed \"x.md\" and \"y.md\" since the pull; Undo would overwrite them."
    );
    assert_eq!(b.state(), before, "nothing changed");
    assert!(
        b.repo.last_sync().unwrap().is_some(),
        "Undo is still offered"
    );

    // Put back as the pull left it, Undo works again.
    b.write("x.md", "x, from GitHub\n");
    b.write("y.md", "y, from GitHub\n");
    b.repo.undo_last_sync().unwrap();
    assert_eq!(b.text("x.md"), "x\n");
}

#[test]
fn a_commit_ends_undo() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    a.write("x.md", "x2\n");
    a.push("A");
    b.pull_applied();
    b.write("new.md", "new\n");
    b.repo.commit("New", &["new.md".into()], &author()).unwrap();
    assert_eq!(b.repo.last_sync(), Ok(None));
    assert!(!record_exists(&b));
    assert!(b.git().find_reference(UNDO_REF).is_err());
    assert_eq!(
        b.repo.undo_last_sync(),
        Err(Error::Invalid("There is no pull to undo.".into()))
    );
}

#[test]
fn undo_refuses_a_commit_made_with_git_and_a_pushed_rebase() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    a.write("x.md", "x2\n");
    a.push("A");
    b.pull_applied();
    // A commit made with the git command line, which the app doesn't see happen.
    b.write("cli.md", "cli\n");
    if b.cli_ok(&["add", "cli.md"]).is_some() {
        b.cli_ok(&["commit", "-q", "-m", "From the CLI"]).unwrap();
        assert_eq!(b.repo.last_sync(), Ok(None), "the branch moved on");
        assert_eq!(
            b.repo.undo_last_sync(),
            Err(Error::UndoRefused(
                "You've committed since the pull, so it can't be undone.".into()
            ))
        );
    }

    // A rebase pushed with git, not the app.
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    a.write("a.md", "a\n");
    a.push("A");
    b.write("b.md", "b\n");
    b.commit("B");
    b.pull_applied();
    b.push_with_git();
    let before = b.state();
    assert_eq!(
        b.repo.undo_last_sync(),
        Err(Error::UndoRefused(
            "The pull has been pushed since, so it can't be undone.".into()
        ))
    );
    assert_eq!(b.state(), before);

    // Pushed by the app: the record is gone.
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    a.write("a.md", "a\n");
    a.push("A");
    b.write("b.md", "b\n");
    b.commit("B");
    b.pull_applied();
    assert!(matches!(
        b.repo.push(None),
        Ok(mdgj_git::PushOutcome::Pushed { .. })
    ));
    assert_eq!(b.repo.last_sync(), Ok(None));
    assert!(matches!(b.repo.undo_last_sync(), Err(Error::Invalid(_))));
}

#[test]
fn a_new_pull_replaces_the_undo_record_and_disconnecting_clears_it() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    a.write("x.md", "x2\n");
    a.push("A");
    b.pull_applied();
    let middle = b.state();
    a.write("x.md", "x3\n");
    a.push("A again");
    let second = b.pull_applied();
    assert_eq!(b.repo.last_sync(), Ok(Some(second)));
    b.repo.undo_last_sync().unwrap();
    assert_eq!(b.state().files, middle.files);
    assert_eq!(b.text("x.md"), "x2\n");
    assert!(
        matches!(b.repo.undo_last_sync(), Err(Error::Invalid(_))),
        "one level"
    );

    b.pull_applied();
    b.repo.remove_remote().unwrap();
    assert_eq!(b.repo.last_sync(), Ok(None));
    assert!(!record_exists(&b));
}
