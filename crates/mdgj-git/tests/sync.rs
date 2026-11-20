//! Two devices syncing through a local bare repository: connect, fetch, push, and pulls where each
//! file changed on one side only, which apply at once.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;

use common::{author, overlap, two_devices, Device, Remote};
use mdgj_git::{
    ChangeKind, CommitInfo, CommitPushOutcome, Error, FileChange, OverlapKind, PullOutcome,
    PullPlan, PushOutcome,
};

fn commit_and_push(d: &Device, message: &str) -> CommitPushOutcome {
    let paths: Vec<String> = d
        .repo
        .status()
        .unwrap()
        .into_iter()
        .map(|c| c.path)
        .collect();
    d.repo
        .commit_and_push(message, &paths, &author(), None)
        .unwrap()
}

fn change(path: &str, kind: ChangeKind) -> FileChange {
    FileChange {
        path: path.into(),
        kind,
        old_path: None,
    }
}

// connect

#[test]
fn connect_to_an_empty_remote_pushes_local_history() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("a.md", "from a\n");
    let head = a.commit("First");

    let PullOutcome::UpToDate { state } = a.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected up to date");
    };
    assert!(state.has_remote);
    assert_eq!(
        (state.branch.as_str(), state.ahead, state.behind),
        ("main", 0, 0)
    );
    assert!(state.last_fetch.is_some());
    assert_eq!(remote.head("main"), Some(head));

    let config = a.git().config().unwrap().snapshot().unwrap();
    assert_eq!(config.get_str("branch.main.remote"), Ok("origin"));
    assert_eq!(config.get_str("branch.main.merge"), Ok("refs/heads/main"));
}

#[test]
fn connect_to_an_empty_remote_without_commits_pushes_nothing() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("draft.md", "not committed\n");
    let PullOutcome::UpToDate { state } = a.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected up to date");
    };
    assert_eq!(state.branch, "main");
    assert_eq!(state.changes.len(), 1);
    assert_eq!(remote.head("main"), None);
    assert_eq!(a.text("draft.md"), "not committed\n");
}

#[test]
fn connect_to_a_non_empty_remote_pulls_it_at_once() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("a.md", "from a\n");
    a.write("assets/a-1.jpg", "jpeg");
    a.commit("First");
    a.repo.connect(&remote.url, None, &author()).unwrap();

    // A fresh device with uncommitted work, including a file the remote has with the same bytes.
    let b = Device::new();
    b.write("draft.md", "draft\n");
    b.write("a.md", "from a\n");
    let PullOutcome::Applied { state, summary } =
        b.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected the pull to apply");
    };
    assert_eq!(b.head(), a.head());
    assert_eq!(b.text("a.md"), "from a\n");
    assert_eq!(b.text("assets/a-1.jpg"), "jpeg");
    assert_eq!(b.text("draft.md"), "draft\n");
    assert_eq!(summary.plan, PullPlan::FastForward);
    assert_eq!(summary.files, [change("assets/a-1.jpg", ChangeKind::Added)]);
    assert_eq!(b.changes(), [("draft.md".into(), ChangeKind::Added)]);
    assert_eq!((state.ahead, state.behind), (0, 0));
}

// 12. both_added through connect

#[test]
fn connect_with_a_file_both_sides_have_differently_opens_a_review() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("a.md", "from a\n");
    a.write("shared.md", "a's version\n");
    a.commit("A's journal");
    a.repo.connect(&remote.url, None, &author()).unwrap();

    let b = Device::new();
    b.write("b.md", "from b\n");
    b.commit("B's first");
    b.write("shared.md", "b's version\n");
    b.commit("B's second");
    let before = b.state();
    let PullOutcome::Review { state, preview } =
        b.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected a review");
    };
    assert!(state.has_remote, "still connected");
    assert_eq!(b.repo.remote_url(), Some(remote.url.clone()));
    assert_eq!(b.state(), before, "nothing written");
    assert_eq!(preview.plan, PullPlan::Merge, "the clash is committed here");
    let shared = overlap(&preview, "shared.md");
    assert_eq!(shared.kind, OverlapKind::BothAdded);
    assert_eq!(
        common::sides(shared),
        ("".into(), "b's version\n".into(), "a's version\n".into())
    );
    assert_eq!(preview.overlaps.len(), 1);
    assert_eq!(preview.auto, [change("a.md", ChangeKind::Added)]);
    assert_eq!(preview.local_count, 2);
    assert_eq!(preview.incoming_count, 1);

    // The same content on both sides needs no review.
    let c = Device::new();
    c.write("shared.md", "a's version\n");
    c.write("c.md", "c\n");
    let outcome = c.repo.connect(&remote.url, None, &author()).unwrap();
    assert!(
        matches!(outcome, PullOutcome::Applied { .. }),
        "{outcome:?}"
    );
    assert_eq!(c.files(), ["a.md", "c.md", "shared.md"]);
    assert_eq!(c.changes(), [("c.md".into(), ChangeKind::Added)]);
}

#[test]
fn connect_with_unrelated_history_on_other_files_rebases_it() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("a.md", "from a\n");
    a.commit("A's journal");
    a.repo.connect(&remote.url, None, &author()).unwrap();

    let b = Device::new();
    b.write("b.md", "from b\n");
    b.commit("B's first");
    let PullOutcome::Applied { state, summary } =
        b.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected the pull to apply");
    };
    assert_eq!(summary.plan, PullPlan::Rebase);
    assert_eq!(summary.rebased, 1);
    assert_eq!(
        b.history(),
        [("B's first".into(), 1), ("A's journal".into(), 0)]
    );
    assert_eq!((state.ahead, state.behind), (1, 0));
    assert!(matches!(b.repo.push(None), Ok(PushOutcome::Pushed { .. })));
    a.pull_applied();
    assert_eq!(a.files(), b.files());
}

#[test]
fn connect_adopts_the_remote_branch_name() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("a.md", "a\n");
    a.commit("A");
    a.repo.connect(&remote.url, None, &author()).unwrap();

    let b = Device::with_branch("master");
    b.write("b.md", "b\n");
    b.commit("B");
    let PullOutcome::Applied { state, .. } = b.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected the pull to apply");
    };
    assert_eq!(state.branch, "main");
    assert!(b
        .git()
        .find_branch("master", git2::BranchType::Local)
        .is_err());
    assert_eq!(b.files(), ["a.md", "b.md"]);

    let unborn = Device::with_branch("master");
    let PullOutcome::Applied { state, .. } =
        unborn.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected the pull to apply");
    };
    assert_eq!(state.branch, "main");
    assert_eq!(unborn.files(), ["a.md"]);
}

#[test]
fn connect_follows_the_remotes_default_branch() {
    let remote = Remote::new();
    let a = Device::with_branch("journal");
    a.write("a.md", "a\n");
    a.commit("A");
    a.repo.connect(&remote.url, None, &author()).unwrap();
    git2::Repository::open_bare(&remote.path)
        .unwrap()
        .set_head("refs/heads/journal")
        .unwrap();

    let b = Device::new();
    b.write("b.md", "b\n");
    b.commit("B");
    let PullOutcome::Applied { state, summary } =
        b.repo.connect(&remote.url, None, &author()).unwrap()
    else {
        panic!("expected the pull to apply");
    };
    assert_eq!(state.branch, "journal");
    assert_eq!(summary.rebased, 1);
    assert_eq!(b.files(), ["a.md", "b.md"]);
}

#[test]
fn failed_connect_leaves_the_previous_remote() {
    let d = Device::new();
    let missing = "file:///nonexistent/mdgitjournal/remote.git";
    assert!(d.repo.connect(missing, None, &author()).is_err());
    assert_eq!(d.repo.remote_url(), None);

    let remote = Remote::new();
    d.repo.connect(&remote.url, None, &author()).unwrap();
    assert!(d.repo.connect(missing, None, &author()).is_err());
    assert_eq!(d.repo.remote_url(), Some(remote.url.clone()));
}

#[test]
fn connect_that_cannot_push_leaves_no_remote() {
    use std::os::unix::fs::PermissionsExt;

    let remote = Remote::new();
    let heads = remote.path.join("refs/heads");
    let mode = |m| fs::set_permissions(&heads, fs::Permissions::from_mode(m)).unwrap();
    let d = Device::new();
    d.write("a.md", "a\n");
    d.commit("A");
    mode(0o555);
    let result = d.repo.connect(&remote.url, None, &author());
    mode(0o755);
    assert!(
        matches!(result, Err(Error::Git(_) | Error::Io(_))),
        "{result:?}"
    );
    assert_eq!(d.repo.remote_url(), None);
    assert_eq!(d.text("a.md"), "a\n");
}

// fetch

#[test]
fn fetch_counts_ahead_and_behind() {
    let (_remote, a, b) = two_devices(&[("base.md", "base\n")]);
    for n in 1..=2 {
        a.write(&format!("a{n}.md"), "a\n");
        commit_and_push(&a, &format!("A{n}"));
    }
    let state = b.repo.sync_state().unwrap();
    assert_eq!(
        (state.ahead, state.behind),
        (0, 0),
        "no network without fetch"
    );

    let state = b.repo.fetch(None).unwrap();
    assert_eq!((state.ahead, state.behind), (0, 2));
    assert!(state.last_fetch.is_some());
    assert!(!b.exists("a1.md"), "fetch must not touch the working tree");

    b.write("b.md", "b\n");
    b.commit("B");
    let state = b.repo.sync_state().unwrap();
    assert_eq!((state.ahead, state.behind), (1, 2));
}

#[test]
fn remote_operations_need_a_remote() {
    let d = Device::new();
    d.write("a.md", "a\n");
    assert_eq!(d.repo.fetch(None), Err(Error::NotConnected));
    assert_eq!(
        d.repo.pull(None, &author()).err(),
        Some(Error::NotConnected)
    );
    assert_eq!(d.repo.push(None), Err(Error::NotConnected));
    assert_eq!(
        d.repo
            .commit_and_push("A", &["a.md".into()], &author(), None),
        Err(Error::NotConnected)
    );
    assert_eq!(d.repo.preview_pull(), Ok(None));
    assert!(d.git().head().is_err(), "nothing may be committed");
}

// commit and push

#[test]
fn commit_and_push_pushes_or_asks_for_a_pull() {
    let (remote, a, b) = two_devices(&[("base.md", "base\n")]);
    assert!(matches!(
        commit_and_push(&a, "Nothing"),
        CommitPushOutcome::NothingToCommit { .. }
    ));
    assert_eq!(
        a.repo.commit_and_push(" ", &[], &author(), None),
        Err(Error::EmptyMessage)
    );

    a.write("a.md", "a\n");
    let CommitPushOutcome::Pushed { state } = commit_and_push(&a, "A") else {
        panic!("expected a push");
    };
    assert_eq!((state.ahead, state.behind), (0, 0));
    assert_eq!(remote.head("main"), Some(a.head()));

    b.write("b.md", "b\n");
    let CommitPushOutcome::NeedsPull { state } = commit_and_push(&b, "B") else {
        panic!("expected NeedsPull");
    };
    assert_eq!((state.ahead, state.behind), (1, 1));
    assert_eq!(b.history()[0], ("B".into(), 1), "the commit stays local");
    assert_eq!(remote.head("main"), Some(a.head()));

    // Nothing new to commit, but the earlier commit still waits for a pull.
    assert!(matches!(
        commit_and_push(&b, "Again"),
        CommitPushOutcome::NeedsPull { .. }
    ));
    b.pull_applied();
    let CommitPushOutcome::Pushed { state } = commit_and_push(&b, "Unused") else {
        panic!("expected the waiting commit to be pushed");
    };
    assert_eq!((state.ahead, state.behind), (0, 0));
    assert_eq!(remote.head("main"), Some(b.head()));
    assert_eq!(b.history()[0], ("B".into(), 1));
}

#[test]
fn commit_and_push_takes_only_the_ticked_files() {
    let (remote, a, _b) = two_devices(&[("base.md", "base\n")]);
    a.write("ticked.md", "ticked\n");
    a.write("unticked.md", "unticked\n");
    let outcome = a
        .repo
        .commit_and_push("Ticked", &["ticked.md".into()], &author(), None)
        .unwrap();
    assert!(matches!(outcome, CommitPushOutcome::Pushed { .. }));
    assert_eq!(remote.head("main"), Some(a.head()));
    assert_eq!(a.committed("ticked.md"), Some(b"ticked\n".to_vec()));
    assert_eq!(a.committed("unticked.md"), None);
    assert_eq!(a.changes(), [("unticked.md".into(), ChangeKind::Added)]);
}

// 11. push race

#[test]
fn push_race_is_rejected_without_losing_anything() {
    let (remote, a, b) = two_devices(&[("base.md", "base\n")]);
    a.write("a.md", "a\n");
    a.commit("A");
    assert_eq!(a.repo.fetch(None).unwrap().behind, 0);
    b.write("b.md", "b\n");
    assert!(matches!(
        commit_and_push(&b, "B"),
        CommitPushOutcome::Pushed { .. }
    ));

    let PushOutcome::Rejected { state } = a.repo.push(None).unwrap() else {
        panic!("expected the push to be rejected");
    };
    assert_eq!((state.ahead, state.behind), (1, 1));
    assert_eq!(remote.head("main"), Some(b.head()));
    assert_eq!(a.text("a.md"), "a\n");
    assert_eq!(a.history()[0], ("A".into(), 1));

    a.pull_applied();
    assert!(matches!(a.repo.push(None), Ok(PushOutcome::Pushed { .. })));
    b.pull_applied();
    assert_eq!(b.files(), ["a.md", "b.md", "base.md"]);
    assert_eq!(a.files(), b.files());
}

// 1. up to date

#[test]
fn pull_when_up_to_date_writes_nothing() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    b.write("x.md", "dirty\n");
    b.write("untracked.md", "untracked\n");
    let before = b.state();
    let PullOutcome::UpToDate { state } = b.pull() else {
        panic!("expected up to date");
    };
    assert_eq!(state.changes.len(), 2);
    assert_eq!(b.state(), before);
    assert_eq!(b.repo.preview_pull(), Ok(None));

    // Local commits the remote doesn't have yet: nothing to pull either.
    a.write("y.md", "y\n");
    a.commit("Y");
    let before = a.state();
    let PullOutcome::UpToDate { state } = a.pull() else {
        panic!("expected up to date");
    };
    assert_eq!(state.ahead, 1);
    assert_eq!(a.state(), before);
}

// 2. no overlap, fast-forward, dirty unrelated files

#[test]
fn a_one_sided_pull_applies_at_once_and_keeps_local_work_byte_for_byte() {
    let long = "A long enough entry for rename detection to recognise it.\n".repeat(4);
    let (_remote, a, b) = two_devices(&[
        ("x.md", "x\n"),
        ("y.md", "y\n"),
        ("gone.md", "remove me\n"),
        ("old.md", &long),
    ]);
    a.write("x.md", "x, edited on a\n");
    a.write("new.md", "new on a\n");
    a.write("assets/new-1.jpg", [0xff, 0xd8, 0xff]);
    a.remove("gone.md");
    commit_and_push(&a, "A's edits\n\nWith a body.");
    fs::rename(a.root().join("old.md"), a.root().join("renamed.md")).unwrap();
    commit_and_push(&a, "A's rename");

    let dirty = b"y\r\nedited on b, no final newline";
    b.write("y.md", dirty);
    b.write("draft.md", "untracked draft\n");
    let before = fs::metadata(b.root().join("y.md")).unwrap();

    let summary = b.pull_applied();
    assert_eq!(summary.plan, PullPlan::FastForward);
    assert_eq!(summary.rebased, 0);
    assert_eq!(summary.merge_commit, None);
    assert_eq!(summary.resolved, []);
    assert_eq!(summary.incoming_count, 2);
    let messages: Vec<_> = summary
        .incoming
        .iter()
        .map(|c| c.message.as_str())
        .collect();
    assert_eq!(
        messages,
        ["A's rename", "A's edits\n\nWith a body."],
        "newest first"
    );
    let CommitInfo {
        id, author, time, ..
    } = &summary.incoming[0];
    assert_eq!(id, &a.head().to_string());
    assert_eq!(author, "Test Author");
    assert!((chrono::Utc::now().timestamp_millis() - time).abs() < 60_000);
    let mut files = summary.files.clone();
    files.sort_by(|x, y| x.path.cmp(&y.path));
    assert_eq!(
        files,
        [
            change("assets/new-1.jpg", ChangeKind::Added),
            change("gone.md", ChangeKind::Deleted),
            change("new.md", ChangeKind::Added),
            FileChange {
                path: "renamed.md".into(),
                kind: ChangeKind::Renamed,
                old_path: Some("old.md".into())
            },
            change("x.md", ChangeKind::Modified),
        ]
    );

    assert_eq!(b.head(), a.head());
    assert_eq!(b.read("y.md"), dirty);
    let after = fs::metadata(b.root().join("y.md")).unwrap();
    assert_eq!(
        (after.ino(), after.mtime_nsec()),
        (before.ino(), before.mtime_nsec())
    );
    assert_eq!(b.text("draft.md"), "untracked draft\n");
    assert_eq!(b.text("x.md"), "x, edited on a\n");
    assert_eq!(b.read("assets/new-1.jpg"), [0xff, 0xd8, 0xff]);
    assert_eq!(b.text("renamed.md"), long);
    assert!(!b.exists("gone.md"));
    assert!(!b.exists("old.md"));
    assert_eq!(
        b.changes(),
        [
            ("draft.md".into(), ChangeKind::Added),
            ("y.md".into(), ChangeKind::Modified)
        ]
    );
    // The index is GitHub's tree.
    let git = b.git();
    let mut index = git.index().unwrap();
    assert_eq!(
        index.write_tree().unwrap(),
        git.head().unwrap().peel_to_tree().unwrap().id()
    );
    assert_eq!(b.repo.last_sync().unwrap(), Some(summary.clone()));
    assert_eq!(b.repo.sync_state().unwrap().last_sync, Some(summary));
    if let Some(status) = b.cli_ok(&["status", "--porcelain"]) {
        assert_eq!(status, " M y.md\n?? draft.md\n");
    }
}

// 4. diverged, no overlap

#[test]
fn diverged_commits_on_different_files_are_rebased_into_a_line() {
    let (remote, a, b) = two_devices(&[("base.md", "base\n")]);
    for name in ["a1", "a2", "a3"] {
        a.write(&format!("{name}.md"), name);
        a.commit(name);
    }
    a.repo.push(None).unwrap();
    for name in ["b1", "b2"] {
        b.write(&format!("{name}.md"), name);
        b.commit(name);
    }

    let summary = b.pull_applied();
    assert_eq!(summary.plan, PullPlan::Rebase);
    assert_eq!(summary.rebased, 2);
    assert_eq!(summary.incoming_count, 3);
    let mut files: Vec<_> = summary.files.iter().map(|f| f.path.as_str()).collect();
    files.sort();
    assert_eq!(files, ["a1.md", "a2.md", "a3.md"]);
    assert_eq!(
        b.files(),
        ["a1.md", "a2.md", "a3.md", "b1.md", "b2.md", "base.md"]
    );
    assert_eq!(
        b.history(),
        [
            ("b2".into(), 1),
            ("b1".into(), 1),
            ("a3".into(), 1),
            ("a2".into(), 1),
            ("a1".into(), 1),
            ("Base".into(), 0)
        ]
    );
    assert_eq!(b.changes(), []);
    let state = b.repo.sync_state().unwrap();
    assert_eq!((state.ahead, state.behind), (2, 0));
    let git = b.git();
    let replayed = git.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(replayed.author().name(), Ok("Test Author"));

    assert!(matches!(b.repo.push(None), Ok(PushOutcome::Pushed { .. })));
    assert_eq!(remote.head("main"), Some(b.head()));
    a.pull_applied();
    assert_eq!(a.files(), b.files());
}

#[test]
fn a_diverged_pull_with_a_same_change_on_both_sides_is_automatic() {
    let (_remote, a, b) = two_devices(&[("entry.md", "base\n")]);
    a.write("entry.md", "same edit\n");
    a.write("a.md", "a\n");
    commit_and_push(&a, "A");
    b.write("entry.md", "same edit\n");
    b.write("b.md", "b\n");
    b.commit("B");
    let summary = b.pull_applied();
    assert_eq!(summary.plan, PullPlan::Rebase);
    assert_eq!(b.text("entry.md"), "same edit\n");
    assert_eq!(b.files(), ["a.md", "b.md", "entry.md"]);
}

// 15. local merge commits

#[test]
fn local_merge_commits_need_a_merge_commit_even_without_overlaps() {
    let (_remote, a, b) = two_devices(&[("base.md", "base\n")]);
    a.write("a.md", "a\n");
    commit_and_push(&a, "A");

    // B merges a side branch with plain git before pulling.
    b.write("b.md", "b\n");
    let main = b.commit("B");
    let git = b.git();
    let base = git.find_commit(main).unwrap().parent(0).unwrap();
    let side_blob = git.blob(b"side\n").unwrap();
    let mut builder = git.treebuilder(Some(&base.tree().unwrap())).unwrap();
    builder.insert("side.md", side_blob, 0o100644).unwrap();
    let side_tree = git.find_tree(builder.write().unwrap()).unwrap();
    let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
    let side = git
        .commit(None, &sig, &sig, "Side", &side_tree, &[&base])
        .unwrap();
    let side = git.find_commit(side).unwrap();
    let main = git.find_commit(main).unwrap();
    let mut merged = git.merge_commits(&main, &side, None).unwrap();
    let tree = git.find_tree(merged.write_tree_to(&git).unwrap()).unwrap();
    git.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "Merge side",
        &tree,
        &[&main, &side],
    )
    .unwrap();
    let mut index = git.index().unwrap();
    index.read_tree(&tree).unwrap();
    index.write().unwrap();
    git.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    let before = b.state();
    let preview = b.pull_review();
    assert_eq!(b.state(), before);
    assert_eq!(preview.plan, PullPlan::Merge);
    assert!(!preview.automatic);
    assert_eq!(preview.overlaps, []);
    assert_eq!(preview.local_count, 3);
    assert_eq!(
        b.repo.apply_pull(&preview.id, &[], None, &author()),
        Err(Error::EmptyMessage)
    );
    assert_eq!(b.state(), before);
    let summary = b
        .repo
        .apply_pull(&preview.id, &[], Some("Merge from GitHub"), &author())
        .unwrap();
    assert_eq!(summary.plan, PullPlan::Merge);
    assert_eq!(summary.merge_commit, Some(b.head().to_string()));
    assert_eq!(b.history()[0], ("Merge from GitHub".into(), 2));
    assert_eq!(b.files(), ["a.md", "b.md", "base.md", "side.md"]);
    assert_eq!(b.changes(), []);
    assert!(matches!(b.repo.push(None), Ok(PushOutcome::Pushed { .. })));
}

// 21. kept from before: line endings, ignored files, links, case

#[test]
fn pulled_bytes_are_exact() {
    let remote = Remote::new();
    let a = Device::new();
    let files: [(&str, &[u8]); 4] = [
        ("crlf.md", b"one\r\ntwo\r\nthree\r\n"),
        ("no-newline.md", b"no final newline"),
        ("mixed.md", b"lf\ncrlf\r\nlone cr\rend"),
        ("assets/binary.jpg", b"\xff\xd8\x00\r\n\x00\xff"),
    ];
    for (path, content) in files {
        a.write(path, content);
    }
    a.commit("Files");
    a.repo.connect(&remote.url, None, &author()).unwrap();

    let b = Device::connected(&remote);
    for (path, content) in files {
        assert_eq!(b.read(path), content, "{path}");
    }
    a.write("crlf.md", "ONE\r\ntwo\r\nthree\r\n");
    a.write("no-newline.md", "still no final newline");
    commit_and_push(&a, "Edits");
    b.pull_applied();
    assert_eq!(b.read("crlf.md"), b"ONE\r\ntwo\r\nthree\r\n");
    assert_eq!(b.read("no-newline.md"), b"still no final newline");
    assert_eq!(b.read("mixed.md"), files[2].1);
}

#[test]
fn ignored_local_files_are_never_overwritten() {
    let (_remote, a, b) = two_devices(&[(".gitignore", "*.local\n")]);
    b.write("notes.local", "private, ignored\n");
    // The remote starts tracking a file at the ignored path (plain git can force that).
    a.write("notes.local", "tracked on a\n");
    let git = a.git();
    let mut index = git.index().unwrap();
    index.add_path(std::path::Path::new("notes.local")).unwrap();
    index.write().unwrap();
    git.commit(
        Some("HEAD"),
        &git2::Signature::now("A", "a@example.com").unwrap(),
        &git2::Signature::now("A", "a@example.com").unwrap(),
        "Track an ignored path",
        &git.find_tree(index.write_tree().unwrap()).unwrap(),
        &[&git.head().unwrap().peel_to_commit().unwrap()],
    )
    .unwrap();
    a.repo.push(None).unwrap();

    let before = b.state();
    let result = b.repo.pull(None, &author());
    assert!(
        matches!(&result, Err(Error::Git(m)) if m.contains("notes.local") && m.contains("ignored")),
        "{result:?}"
    );
    assert_eq!(b.state(), before);
    assert_eq!(b.text("notes.local"), "private, ignored\n");
}

#[test]
fn a_remote_symlink_is_refused_and_nothing_is_written() {
    let (_remote, a, b) = two_devices(&[("base.md", "base\n")]);
    let outside = tempfile::tempdir().unwrap();
    // Committed with plain git: the app never makes links.
    let git = a.git();
    let link = git
        .blob(outside.path().to_str().unwrap().as_bytes())
        .unwrap();
    let note = git.blob(b"note\n").unwrap();
    let head = git.head().unwrap().peel_to_commit().unwrap();
    let mut builder = git.treebuilder(Some(&head.tree().unwrap())).unwrap();
    builder.insert("assets", link, 0o120000).unwrap();
    builder.insert("note.md", note, 0o100644).unwrap();
    let tree = git.find_tree(builder.write().unwrap()).unwrap();
    let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
    git.commit(Some("HEAD"), &sig, &sig, "Link", &tree, &[&head])
        .unwrap();
    a.repo.push(None).unwrap();

    let before = b.state();
    let result = b.repo.pull(None, &author());
    assert!(
        matches!(&result, Err(Error::Git(m)) if m.contains("symbolic link")),
        "{result:?}"
    );
    assert_eq!(b.state(), before);
    assert!(b.root().join("assets").symlink_metadata().is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    assert!(matches!(b.repo.preview_pull(), Err(Error::Git(_))));
}

#[test]
fn names_that_differ_only_in_case_are_refused() {
    let (_remote, a, b) = two_devices(&[("base.md", "base\n")]);
    let git = a.git();
    let blob = git.blob(b"x\n").unwrap();
    let head = git.head().unwrap().peel_to_commit().unwrap();
    let mut builder = git.treebuilder(Some(&head.tree().unwrap())).unwrap();
    builder.insert("Note.md", blob, 0o100644).unwrap();
    builder.insert("note.md", blob, 0o100644).unwrap();
    let tree = git.find_tree(builder.write().unwrap()).unwrap();
    let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
    git.commit(Some("HEAD"), &sig, &sig, "Case", &tree, &[&head])
        .unwrap();
    a.repo.push(None).unwrap();

    let before = b.state();
    let result = b.repo.pull(None, &author());
    assert!(
        matches!(&result, Err(Error::Git(m)) if m.contains("same name")),
        "{result:?}"
    );
    assert_eq!(b.state(), before);
}

#[test]
fn a_case_only_rename_loses_nothing() {
    let (_remote, a, b) = two_devices(&[("entry.md", "text\n")]);
    // Committed with plain git: the app itself only writes lowercase names.
    let git = a.git();
    let blob = git.blob(b"text\n").unwrap();
    let head = git.head().unwrap().peel_to_commit().unwrap();
    let mut builder = git.treebuilder(Some(&head.tree().unwrap())).unwrap();
    builder.remove("entry.md").unwrap();
    builder.insert("Entry.md", blob, 0o100644).unwrap();
    let tree = git.find_tree(builder.write().unwrap()).unwrap();
    let sig = git2::Signature::now("Test Author", "test@example.com").unwrap();
    git.commit(Some("HEAD"), &sig, &sig, "Capitalise", &tree, &[&head])
        .unwrap();
    a.repo.push(None).unwrap();

    b.pull_applied();
    // Whatever the file system's case rules, the remote's file is there and nothing is deleted.
    assert_eq!(b.text("Entry.md"), "text\n");
    assert!(b
        .repo
        .status()
        .unwrap()
        .iter()
        .all(|change| change.kind != ChangeKind::Deleted));
    for file in b.files() {
        assert_eq!(b.text(&file), "text\n", "{file}");
    }
    // And Undo brings the old name back.
    b.repo.undo_last_sync().unwrap();
    assert_eq!(b.text("entry.md"), "text\n");
    assert_eq!(b.changes(), []);
}

/// A small PNG with some texture; `seed` changes it.
pub fn png(seed: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(96, 64, |x, y| {
        image::Rgb([
            ((x + seed) % 256) as u8,
            (y % 256) as u8,
            ((x ^ y) % 256) as u8,
        ])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

// 10 (second half). Photos added to one entry on two devices.

#[test]
fn photos_added_to_one_entry_on_two_devices_both_arrive() {
    use mdgj_core::{ImageOpts, Journal};
    const OPTS: ImageOpts = ImageOpts {
        max_edge: 1024,
        quality: 60,
    };
    let entry = "2026-11-16-1402-run.md";
    let base = "Line one.\nLine two.\nLine three.\n";
    for (b_commits_first, b_seed) in [(false, 1), (true, 1), (false, 0), (true, 0)] {
        let case = format!(
            "b commits first: {b_commits_first}, same photo: {}",
            b_seed == 0
        );
        let (_remote, a, b) = two_devices(&[(entry, base)]);
        let from_a = Journal::open(a.root())
            .unwrap()
            .import_image(entry, &png(0), OPTS)
            .unwrap()
            .path;
        a.write(
            entry,
            format!("Line one.\n![]({from_a})\nLine two.\nLine three.\n"),
        );
        commit_and_push(&a, "Photo on a");

        let from_b = Journal::open(b.root())
            .unwrap()
            .import_image(entry, &png(b_seed), OPTS)
            .unwrap()
            .path;
        let b_bytes = b.read(&from_b);
        b.write(entry, format!("{base}![]({from_b})\n"));
        if b_commits_first {
            b.commit("Photo on b");
        }

        // The photos never clash (named by content); the entry's two edits merge cleanly.
        let preview = b.pull_review();
        assert_eq!(preview.overlaps.len(), 1, "{case}");
        let text = overlap(&preview, entry);
        let suggestion = text.suggestion.as_ref().unwrap();
        assert!(suggestion.clean, "{case}");
        let merged = suggestion.text().unwrap();
        assert_eq!(
            merged,
            format!("Line one.\n![]({from_a})\nLine two.\nLine three.\n![]({from_b})\n"),
            "{case}"
        );
        let photo_arrives = from_a != from_b;
        assert_eq!(
            preview.auto.iter().any(|f| f.path == from_a),
            photo_arrives,
            "{case}"
        );
        b.apply(&preview, &[common::merged(entry, &merged)]);
        assert_eq!(b.text(entry), merged, "{case}");
        assert_eq!(b.read(&from_a), a.read(&from_a), "{case}");
        assert_eq!(b.read(&from_b), b_bytes, "{case}");
        assert_eq!(from_a == from_b, b_seed == 0, "{case}");
    }
}
