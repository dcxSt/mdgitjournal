//! Resolve with git: a standard merge left in progress for the user's own tools, Abort, and
//! finishing it with git.

mod common;

use std::fs;
use std::path::Path;

use common::{author, two_devices, Device};
use mdgj_git::{ChangeKind, Error, MergeState, PullPreview};

const ENTRY: &str = "2026-11-16-1402-run.md";
const BASE: &str = "# Run\n\nFirst paragraph.\n\nSecond paragraph.\n";

/// A clashes with B on ENTRY (B's side committed), and adds a.md; B also added b.md.
fn committed_clash() -> (common::Remote, Device, Device, PullPreview) {
    let (remote, a, b) = two_devices(&[(ENTRY, BASE), ("x.md", "x\n")]);
    a.write(
        ENTRY,
        BASE.replace("First paragraph.", "First, from GitHub."),
    );
    a.write("a.md", "a\n");
    a.push("A");
    b.write(
        ENTRY,
        BASE.replace("First paragraph.", "First, committed here."),
    );
    b.write("b.md", "b\n");
    b.commit("B");
    let preview = b.pull_review();
    (remote, a, b, preview)
}

fn git_file(d: &Device, name: &str) -> Option<String> {
    fs::read_to_string(d.root().join(".git").join(name)).ok()
}

// 16. Resolve with git on a clean tree

#[test]
fn resolve_with_git_leaves_a_standard_merge_in_progress() {
    let (_remote, a, b, preview) = committed_clash();
    let head = b.head();
    b.repo
        .start_external_merge(&preview.id, None, &author())
        .unwrap();

    assert_eq!(b.head(), head, "HEAD stays");
    assert_eq!(
        git_file(&b, "MERGE_HEAD").unwrap().trim(),
        a.head().to_string()
    );
    assert_eq!(
        git_file(&b, "MERGE_MSG").unwrap(),
        "Merge remote-tracking branch 'origin/main'\n"
    );
    assert!(git_file(&b, "MERGE_MODE").is_some());
    assert_eq!(git_file(&b, "ORIG_HEAD").unwrap().trim(), head.to_string());
    assert_eq!(
        b.text(ENTRY),
        "# Run\n\n<<<<<<< HEAD\nFirst, committed here.\n=======\nFirst, from GitHub.\n\
         >>>>>>> origin/main\n\nSecond paragraph.\n"
    );
    assert_eq!(b.text("a.md"), "a\n", "clean paths are merged and staged");

    let git = b.git();
    let index = git.index().unwrap();
    let conflict = index.conflict_get(Path::new(ENTRY)).unwrap();
    let blob = |entry: Option<git2::IndexEntry>| {
        git.find_blob(entry.unwrap().id).unwrap().content().to_vec()
    };
    assert_eq!(blob(conflict.ancestor), BASE.as_bytes());
    assert_eq!(
        blob(conflict.our),
        BASE.replace("First paragraph.", "First, committed here.")
            .as_bytes()
    );
    assert_eq!(
        blob(conflict.their),
        BASE.replace("First paragraph.", "First, from GitHub.")
            .as_bytes()
    );
    assert!(index.get_path(Path::new("a.md"), 0).is_some());

    let state = b.repo.merge_state().unwrap();
    assert_eq!(
        state,
        Some(MergeState {
            conflicted: vec![ENTRY.into()],
            started_by_app: true
        })
    );
    assert_eq!(b.repo.sync_state().unwrap().merge, state);
    assert_eq!(b.repo.last_sync(), Ok(None));
    if let Some(status) = b.cli_ok(&["status"]) {
        assert!(status.contains("Unmerged paths"), "{status}");
        assert!(status.contains("You have unmerged paths"), "{status}");
    }

    // Everything that would change history waits; fetch, status and diff still work.
    let paths = [ENTRY.to_owned()];
    assert_eq!(b.repo.pull(None, &author()), Err(Error::MergeInProgress));
    assert_eq!(b.repo.preview_pull(), Err(Error::MergeInProgress));
    assert_eq!(
        b.repo.apply_pull(&preview.id, &[], Some("M"), &author()),
        Err(Error::MergeInProgress)
    );
    assert_eq!(
        b.repo.commit("C", &paths, &author()),
        Err(Error::MergeInProgress)
    );
    assert_eq!(
        b.repo.commit_all("C", &author()),
        Err(Error::MergeInProgress)
    );
    assert_eq!(
        b.repo.commit_and_push("C", &paths, &author(), None),
        Err(Error::MergeInProgress)
    );
    assert_eq!(b.repo.push(None), Err(Error::MergeInProgress));
    assert_eq!(b.repo.undo_last_sync(), Err(Error::MergeInProgress));
    assert_eq!(
        b.repo.start_external_merge(&preview.id, None, &author()),
        Err(Error::MergeInProgress)
    );
    let url = b.repo.remote_url().unwrap();
    assert_eq!(
        b.repo.connect(&url, None, &author()),
        Err(Error::MergeInProgress)
    );
    assert!(b.repo.fetch(None).is_ok());
    assert!(b.repo.status().is_ok());
    assert!(b.repo.diff(ENTRY).is_ok());
}

// 17. Abort

#[test]
fn abort_after_a_clean_start_restores_everything_byte_for_byte() {
    let (_remote, _a, b, preview) = committed_clash();
    let before = b.state();
    b.repo
        .start_external_merge(&preview.id, None, &author())
        .unwrap();
    // The user starts resolving, then gives up.
    b.write(ENTRY, "half resolved\n");
    let mut restored = b.repo.abort_merge().unwrap();
    restored.sort();
    assert_eq!(restored, [ENTRY, "a.md"]);
    assert_eq!(b.state(), before, "refs, index bytes and files as before");
    assert_eq!(b.repo.merge_state(), Ok(None));
    assert!(git_file(&b, "MERGE_HEAD").is_none());
    assert!(git_file(&b, "MERGE_MSG").is_none());
    assert!(!b.root().join(".git/mdgitjournal/merge.json").exists());
    assert_eq!(
        b.repo.abort_merge(),
        Err(Error::Invalid("No merge is in progress.".into()))
    );
    // And the review is there again.
    assert_eq!(b.repo.preview_pull().unwrap().unwrap().id, preview.id);
}

#[test]
fn resolve_with_git_commits_uncommitted_edits_first_and_abort_takes_that_back() {
    let (_remote, a, b) = two_devices(&[(ENTRY, BASE), ("x.md", "x\n")]);
    a.write(
        ENTRY,
        BASE.replace("First paragraph.", "First, from GitHub."),
    );
    a.push("A");
    b.write(
        ENTRY,
        BASE.replace("First paragraph.", "First, typed here."),
    );
    b.write("untracked.md", "untracked\n");
    b.write("x.md", "x, edited\n");
    let preview = b.pull_review();
    let local = b.head();
    let before = b.state();
    let status = b.repo.status().unwrap();

    for message in [None, Some(""), Some("  ")] {
        assert_eq!(
            b.repo.start_external_merge(&preview.id, message, &author()),
            Err(Error::EmptyMessage)
        );
        assert_eq!(b.state(), before);
    }
    b.repo
        .start_external_merge(&preview.id, Some("Work in progress"), &author())
        .unwrap();
    let git = b.git();
    let precommit = git.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(precommit.message(), Ok("Work in progress\n"));
    assert_eq!(precommit.parent_id(0).unwrap(), local);
    assert_eq!(b.committed("untracked.md"), Some(b"untracked\n".to_vec()));
    assert!(b.text(ENTRY).contains("<<<<<<< HEAD\nFirst, typed here.\n"));
    assert_eq!(b.text("x.md"), "x, edited\n");
    assert_eq!(b.repo.merge_state().unwrap().unwrap().conflicted, [ENTRY]);

    b.repo.abort_merge().unwrap();
    assert_eq!(b.head(), local, "the commit of my edits is taken back");
    assert_eq!(b.repo.status().unwrap(), status);
    assert_eq!(b.state(), before.without_undo());
}

#[test]
fn resolve_with_git_on_an_unborn_branch_and_abort() {
    let remote = common::Remote::new();
    let a = Device::new();
    a.write("shared.md", "a's\n");
    a.commit("A");
    a.repo.connect(&remote.url, None, &author()).unwrap();
    let b = Device::new();
    b.write("shared.md", "b's\n");
    let mdgj_git::PullOutcome::Review { preview, .. } =
        b.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected a review");
    };
    let before = b.state();
    b.repo
        .start_external_merge(&preview.id, Some("B's journal"), &author())
        .unwrap();
    assert_eq!(
        b.text("shared.md"),
        "<<<<<<< HEAD\nb's\n=======\na's\n>>>>>>> origin/main\n"
    );
    b.repo.abort_merge().unwrap();
    assert!(b.git().head().is_err(), "unborn again");
    assert_eq!(b.state(), before.without_undo());
    assert_eq!(b.changes(), [("shared.md".into(), ChangeKind::Added)]);
}

#[test]
fn resolve_with_git_needs_something_to_merge() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    a.write("x.md", "x2\n");
    a.push("A");
    b.repo.fetch(None).unwrap();
    let preview = b.repo.preview_pull().unwrap().unwrap();
    assert!(preview.automatic);
    assert!(matches!(
        b.repo.start_external_merge(&preview.id, None, &author()),
        Err(Error::Invalid(_))
    ));
}

// 18. Finishing with git

#[test]
fn finishing_the_merge_with_git_returns_to_normal() {
    let (remote, a, b, preview) = committed_clash();
    b.repo
        .start_external_merge(&preview.id, None, &author())
        .unwrap();

    // Resolve, stage and commit as `git commit` would, through libgit2.
    let resolved = BASE.replace("First paragraph.", "First, resolved in vim.");
    b.write(ENTRY, &resolved);
    let git = b.git();
    let mut index = git.index().unwrap();
    index.add_path(Path::new(ENTRY)).unwrap();
    index.write().unwrap();
    let tree = git.find_tree(index.write_tree().unwrap()).unwrap();
    let head = git.head().unwrap().peel_to_commit().unwrap();
    let theirs = git.find_commit(a.head()).unwrap();
    let sig = git2::Signature::now("CLI User", "cli@example.com").unwrap();
    let message = git_file(&b, "MERGE_MSG").unwrap();
    git.commit(Some("HEAD"), &sig, &sig, &message, &tree, &[&head, &theirs])
        .unwrap();
    git.cleanup_state().unwrap();

    assert_eq!(b.repo.merge_state(), Ok(None));
    assert_eq!(b.repo.sync_state().unwrap().merge, None);
    assert!(
        b.root().join(".git/mdgitjournal/merge.json").exists(),
        "until the next operation"
    );
    assert_eq!(b.repo.preview_pull(), Ok(None));
    assert!(!b.root().join(".git/mdgitjournal/merge.json").exists());
    assert_eq!(b.changes(), []);
    assert!(matches!(
        b.repo.push(None),
        Ok(mdgj_git::PushOutcome::Pushed { .. })
    ));
    assert_eq!(remote.head("main"), Some(b.head()));

    let summary = a.pull_applied();
    assert_eq!(summary.plan, mdgj_git::PullPlan::FastForward);
    assert_eq!(a.text(ENTRY), resolved);
    assert_eq!(a.files(), b.files());
}

#[test]
fn finishing_the_merge_with_the_git_command_line() {
    let (_remote, a, b, preview) = committed_clash();
    b.repo
        .start_external_merge(&preview.id, None, &author())
        .unwrap();
    b.write(ENTRY, BASE.replace("First paragraph.", "First, resolved."));
    if b.cli_ok(&["add", ENTRY]).is_none() {
        return; // no git on PATH
    }
    b.cli_ok(&["commit", "--no-edit", "-q"]).unwrap();
    let log = b.cli_ok(&["log", "-1", "--format=%P%n%B"]).unwrap();
    let mut lines = log.lines();
    let parents: Vec<_> = lines.next().unwrap().split(' ').collect();
    assert_eq!(parents.len(), 2);
    assert_eq!(parents[1], a.head().to_string());
    assert_eq!(
        lines.next(),
        Some("Merge remote-tracking branch 'origin/main'")
    );
    assert_eq!(b.repo.merge_state(), Ok(None));
    assert!(matches!(
        b.repo.push(None),
        Ok(mdgj_git::PushOutcome::Pushed { .. })
    ));
    a.pull_applied();
    assert_eq!(a.files(), b.files());
}

#[test]
fn a_merge_started_on_the_command_line_is_shown_and_can_be_aborted() {
    let (_remote, _a, b, _preview) = committed_clash();
    let before = b.state();
    let Some(output) = b.cli(&["merge", "origin/main"]) else {
        return; // no git on PATH
    };
    assert!(!output.status.success(), "the merge conflicts");
    assert_eq!(
        b.repo.merge_state(),
        Ok(Some(MergeState {
            conflicted: vec![ENTRY.into()],
            started_by_app: false
        }))
    );
    assert_eq!(b.repo.pull(None, &author()), Err(Error::MergeInProgress));
    b.repo.abort_merge().unwrap();
    assert_eq!(b.repo.merge_state(), Ok(None));
    assert_eq!(b.state().files, before.files);
    assert_eq!(b.state().refs, before.refs);
    assert_eq!(b.changes(), []);
    if let Some(status) = b.cli_ok(&["status", "--porcelain"]) {
        assert_eq!(status, "");
    }
}

#[test]
fn a_cli_abort_leaves_the_commit_of_edits_and_the_app_carries_on() {
    let (_remote, a, b) = two_devices(&[(ENTRY, BASE)]);
    a.write(
        ENTRY,
        BASE.replace("First paragraph.", "First, from GitHub."),
    );
    a.push("A");
    b.write(
        ENTRY,
        BASE.replace("First paragraph.", "First, typed here."),
    );
    let preview = b.pull_review();
    b.repo
        .start_external_merge(&preview.id, Some("My edits"), &author())
        .unwrap();
    if b.cli_ok(&["merge", "--abort"]).is_none() {
        return;
    }
    assert_eq!(b.repo.merge_state(), Ok(None));
    assert_eq!(
        b.history()[0],
        ("My edits".into(), 1),
        "the user's own git command"
    );
    let preview = b.pull_review();
    assert_eq!(preview.plan, mdgj_git::PullPlan::Merge);
    assert!(!b.root().join(".git/mdgitjournal/merge.json").exists());
}
