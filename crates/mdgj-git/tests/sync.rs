//! Two devices syncing through a local bare repository: connect, fetch, pull, push.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;

use common::{author, is_conflict_copy, Device, Remote};
use mdgj_git::{ChangeKind, CommitPushOutcome, Conflict, Error, PullOutcome, PushOutcome};

fn pull(d: &Device) -> PullOutcome {
    d.repo.pull(None, &author()).unwrap()
}

fn commit_and_push(d: &Device, message: &str) -> CommitPushOutcome {
    d.repo.commit_and_push(message, &author(), None).unwrap()
}

/// A remote holding `files` in one commit, and two devices connected to it.
fn two_devices(files: &[(&str, &str)]) -> (Remote, Device, Device) {
    let remote = Remote::new();
    let a = Device::new();
    for (path, content) in files {
        a.write(path, content);
    }
    a.commit("Base");
    a.repo.connect(&remote.url, None, &author()).unwrap();
    let b = Device::connected(&remote);
    assert_eq!(b.head(), a.head());
    (remote, a, b)
}

fn no_conflict_markers(d: &Device) {
    for file in d.files() {
        let content = d.read(&file);
        assert!(
            !content
                .windows(7)
                .any(|w| w == b"<<<<<<<" || w == b">>>>>>>"),
            "conflict markers in {file}"
        );
    }
}

// 2. connect

#[test]
fn connect_to_an_empty_remote_pushes_local_history() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("a.md", "from a\n");
    let head = a.commit("First");

    let out = a.repo.connect(&remote.url, None, &author()).unwrap();
    assert!(out.state.has_remote);
    assert_eq!(
        (out.state.branch.as_str(), out.state.ahead, out.state.behind),
        ("main", 0, 0)
    );
    assert!(out.state.last_fetch.is_some());
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
    let out = a.repo.connect(&remote.url, None, &author()).unwrap();
    assert_eq!(out.state.branch, "main");
    assert_eq!(out.state.changes.len(), 1);
    assert_eq!(remote.head("main"), None);
    assert_eq!(a.text("draft.md"), "not committed\n");
}

#[test]
fn connect_to_a_non_empty_remote_pulls_it() {
    let remote = Remote::new();
    let a = Device::new();
    a.write("a.md", "from a\n");
    a.write("assets/a-1.jpg", "jpeg");
    a.commit("First");
    a.repo.connect(&remote.url, None, &author()).unwrap();

    // A fresh device with uncommitted work, including a file whose name the remote also uses.
    let b = Device::new();
    b.write("draft.md", "draft\n");
    b.write("a.md", "b's own a\n");
    let out = b.repo.connect(&remote.url, None, &author()).unwrap();

    assert_eq!(b.head(), a.head());
    assert_eq!(b.text("a.md"), "from a\n");
    assert_eq!(b.text("assets/a-1.jpg"), "jpeg");
    assert_eq!(b.text("draft.md"), "draft\n");
    let [Conflict {
        path,
        copy: Some(copy),
    }] = &out.conflicts[..]
    else {
        panic!("expected one keep-both conflict: {:?}", out.conflicts);
    };
    assert_eq!(path, "a.md");
    assert!(is_conflict_copy(copy, "a", ".md"), "{copy}");
    assert_eq!(b.text(copy), "b's own a\n");
    assert_eq!(
        b.changes(),
        [
            (copy.clone(), ChangeKind::Added),
            ("draft.md".into(), ChangeKind::Added)
        ]
    );
    assert_eq!((out.state.ahead, out.state.behind), (0, 0));
}

#[test]
fn connect_with_unrelated_local_history_keeps_both() {
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
    let out = b.repo.connect(&remote.url, None, &author()).unwrap();

    assert_eq!(out.rebased, 2);
    let [Conflict {
        path,
        copy: Some(copy),
    }] = &out.conflicts[..]
    else {
        panic!("expected one keep-both conflict: {:?}", out.conflicts);
    };
    assert_eq!(path, "shared.md");
    assert_eq!(b.text("shared.md"), "a's version\n");
    assert_eq!(b.text(copy), "b's version\n");
    assert_eq!(b.text("a.md"), "from a\n");
    assert_eq!(b.text("b.md"), "from b\n");
    assert_eq!(b.changes(), []);
    assert_eq!(
        b.history(),
        [
            ("B's second".into(), 1),
            ("B's first".into(), 1),
            ("A's journal".into(), 0)
        ]
    );
    assert_eq!((out.state.ahead, out.state.behind), (2, 0));

    assert!(matches!(b.repo.push(None), Ok(PushOutcome::Pushed { .. })));
    pull(&a);
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
    let out = b.repo.connect(&remote.url, None, &author()).unwrap();
    assert_eq!(out.state.branch, "main");
    assert!(b
        .git()
        .find_branch("master", git2::BranchType::Local)
        .is_err());
    assert_eq!(b.files(), ["a.md", "b.md"]);

    let unborn = Device::with_branch("master");
    let out = unborn.repo.connect(&remote.url, None, &author()).unwrap();
    assert_eq!(out.state.branch, "main");
    assert_eq!(unborn.files(), ["a.md"]);
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

// 3. fetch

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
        d.repo.commit_and_push("A", &author(), None),
        Err(Error::NotConnected)
    );
    assert!(d.git().head().is_err(), "nothing may be committed");
}

// 4. commit and push

#[test]
fn commit_and_push_pushes_or_asks_for_a_pull() {
    let (remote, a, b) = two_devices(&[("base.md", "base\n")]);
    assert!(matches!(
        commit_and_push(&a, "Nothing"),
        CommitPushOutcome::NothingToCommit { .. }
    ));
    assert_eq!(
        a.repo.commit_and_push(" ", &author(), None),
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
    pull(&b);
    let CommitPushOutcome::Pushed { state } = commit_and_push(&b, "Unused") else {
        panic!("expected the waiting commit to be pushed");
    };
    assert_eq!((state.ahead, state.behind), (0, 0));
    assert_eq!(remote.head("main"), Some(b.head()));
    assert_eq!(b.history()[0], ("B".into(), 1));
}

// 5. fast-forward with uncommitted, unrelated work

#[test]
fn fast_forward_keeps_unrelated_local_work_byte_for_byte() {
    let (_remote, a, b) =
        two_devices(&[("x.md", "x\n"), ("y.md", "y\n"), ("gone.md", "remove me\n")]);
    a.write("x.md", "x, edited on a\n");
    a.write("new.md", "new on a\n");
    a.write("assets/new-1.jpg", [0xff, 0xd8, 0xff]);
    a.remove("gone.md");
    commit_and_push(&a, "A's edits");

    let dirty = b"y\r\nedited on b, no final newline";
    b.write("y.md", dirty);
    b.write("draft.md", "untracked draft\n");
    let before = fs::metadata(b.root().join("y.md")).unwrap();

    let out = pull(&b);
    assert_eq!(
        out.updated,
        ["assets/new-1.jpg", "gone.md", "new.md", "x.md"]
    );
    assert_eq!(out.merged, Vec::<String>::new());
    assert_eq!(out.conflicts, []);
    assert_eq!(out.rebased, 0);

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
    assert!(!b.exists("gone.md"));
    assert_eq!(
        b.changes(),
        [
            ("draft.md".into(), ChangeKind::Added),
            ("y.md".into(), ChangeKind::Modified)
        ]
    );
    assert_eq!(out.state.changes.len(), 2);
}

#[test]
fn pull_when_up_to_date_changes_nothing() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    b.write("x.md", "dirty\n");
    let out = pull(&b);
    assert_eq!((out.updated.len(), out.conflicts.len()), (0, 0));
    assert_eq!(b.text("x.md"), "dirty\n");

    // Local commits the remote doesn't have yet: nothing to pull either.
    a.write("y.md", "y\n");
    a.commit("Y");
    let out = pull(&a);
    assert_eq!((out.updated.len(), out.state.ahead), (0, 1));
}

// 6. clean three-way merge of uncommitted edits

#[test]
fn edits_to_different_lines_merge_cleanly() {
    let base = "one\ntwo\nthree\nfour\nfive\n";
    let (_remote, a, b) = two_devices(&[("entry.md", base)]);
    a.write("entry.md", "ONE\ntwo\nthree\nfour\nfive\n");
    commit_and_push(&a, "A");
    b.write("entry.md", "one\ntwo\nthree\nfour\nFIVE\n");

    let out = pull(&b);
    assert_eq!(out.merged, ["entry.md"]);
    assert_eq!(out.updated, ["entry.md"]);
    assert_eq!(out.conflicts, []);
    assert_eq!(b.text("entry.md"), "ONE\ntwo\nthree\nfour\nFIVE\n");
    assert_eq!(b.head(), a.head());
    assert_eq!(b.changes(), [("entry.md".into(), ChangeKind::Modified)]);
}

// 7. true conflict with uncommitted edits

#[test]
fn same_line_conflict_keeps_both_versions() {
    let (_remote, a, b) = two_devices(&[("2026-11-16-1402-run.md", "ran\n")]);
    a.write("2026-11-16-1402-run.md", "ran 5k on a\n");
    commit_and_push(&a, "A");
    let local = b"ran 10k on b\r\nno final newline";
    b.write("2026-11-16-1402-run.md", local);

    let out = pull(&b);
    let [Conflict {
        path,
        copy: Some(copy),
    }] = &out.conflicts[..]
    else {
        panic!("expected one keep-both conflict: {:?}", out.conflicts);
    };
    assert_eq!(path, "2026-11-16-1402-run.md");
    assert!(
        is_conflict_copy(copy, "2026-11-16-1402-run", ".md"),
        "{copy}"
    );
    assert_eq!(b.text(path), "ran 5k on a\n");
    assert_eq!(b.read(copy), local);
    assert_eq!(out.merged, Vec::<String>::new());
    assert_eq!(out.updated, [copy.clone(), path.clone()]);
    assert_eq!(b.changes(), [(copy.clone(), ChangeKind::Added)]);
    no_conflict_markers(&b);
}

// 8. diverged commits on different files

#[test]
fn diverged_commits_are_rebased_into_a_line() {
    let (remote, a, b) = two_devices(&[("base.md", "base\n")]);
    for name in ["a1", "a2"] {
        a.write(&format!("{name}.md"), name);
        a.commit(name);
    }
    a.repo.push(None).unwrap();
    for name in ["b1", "b2"] {
        b.write(&format!("{name}.md"), name);
        b.commit(name);
    }

    let out = pull(&b);
    assert_eq!(out.rebased, 2);
    assert_eq!(out.conflicts, []);
    assert_eq!(out.updated, ["a1.md", "a2.md"]);
    assert_eq!(b.files(), ["a1.md", "a2.md", "b1.md", "b2.md", "base.md"]);
    assert_eq!(
        b.history(),
        [
            ("b2".into(), 1),
            ("b1".into(), 1),
            ("a2".into(), 1),
            ("a1".into(), 1),
            ("Base".into(), 0)
        ]
    );
    assert_eq!(b.changes(), []);
    assert_eq!((out.state.ahead, out.state.behind), (2, 0));
    let git = b.git();
    let replayed = git.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(replayed.author().name(), Ok("Test Author"));

    assert!(matches!(b.repo.push(None), Ok(PushOutcome::Pushed { .. })));
    assert_eq!(remote.head("main"), Some(b.head()));
    pull(&a);
    assert_eq!(a.files(), b.files());
}

// 9. diverged commits on the same line

#[test]
fn rebase_conflicts_keep_both_versions_in_the_commit() {
    let (_remote, a, b) = two_devices(&[("entry.md", "base\n")]);
    a.write("entry.md", "from a\n");
    commit_and_push(&a, "A");
    b.write("entry.md", "from b\n");
    b.commit("B");

    let out = pull(&b);
    assert_eq!(out.rebased, 1);
    let [Conflict {
        path,
        copy: Some(copy),
    }] = &out.conflicts[..]
    else {
        panic!("expected one keep-both conflict: {:?}", out.conflicts);
    };
    assert_eq!(path, "entry.md");
    assert!(is_conflict_copy(copy, "entry", ".md"), "{copy}");
    assert_eq!(b.text("entry.md"), "from a\n");
    assert_eq!(b.text(copy), "from b\n");
    assert_eq!(b.committed(copy), Some(b"from b\n".to_vec()));
    assert_eq!(b.committed("entry.md"), Some(b"from a\n".to_vec()));
    assert_eq!(b.changes(), []);
    assert_eq!(b.history()[0], ("B".into(), 1));
    no_conflict_markers(&b);

    assert!(matches!(b.repo.push(None), Ok(PushOutcome::Pushed { .. })));
    pull(&a);
    assert_eq!(a.text(copy), "from b\n");
}

// 10. deleted on one side, changed on the other

#[test]
fn remote_deletion_keeps_a_local_modification() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n"), ("y.md", "y\n")]);
    a.remove("x.md");
    a.remove("y.md");
    commit_and_push(&a, "Delete");
    b.write("x.md", "x, edited on b\n");

    let out = pull(&b);
    assert_eq!(b.text("x.md"), "x, edited on b\n");
    assert!(
        !b.exists("y.md"),
        "an unmodified file follows the remote deletion"
    );
    assert_eq!(out.updated, ["y.md"]);
    assert_eq!(b.changes(), [("x.md".into(), ChangeKind::Added)]);
}

#[test]
fn local_deletion_keeps_a_remote_modification() {
    let (_remote, a, b) = two_devices(&[("x.md", "x\n")]);
    a.write("x.md", "x, edited on a\n");
    commit_and_push(&a, "Edit");
    b.remove("x.md");

    let out = pull(&b);
    assert_eq!(b.text("x.md"), "x, edited on a\n");
    assert_eq!(
        out.conflicts,
        [Conflict {
            path: "x.md".into(),
            copy: None
        }]
    );
    assert_eq!(b.changes(), []);
}

#[test]
fn committed_deletions_keep_the_other_sides_modification() {
    let (_remote, a, b) = two_devices(&[("ours.md", "o\n"), ("theirs.md", "t\n")]);
    a.write("ours.md", "o, edited on a\n");
    a.remove("theirs.md");
    commit_and_push(&a, "A");
    b.remove("ours.md");
    b.write("theirs.md", "t, edited on b\n");
    b.commit("B");

    let out = pull(&b);
    assert_eq!(b.text("ours.md"), "o, edited on a\n");
    assert_eq!(b.text("theirs.md"), "t, edited on b\n");
    assert_eq!(b.committed("ours.md"), Some(b"o, edited on a\n".to_vec()));
    assert_eq!(b.committed("theirs.md"), Some(b"t, edited on b\n".to_vec()));
    assert_eq!(
        out.conflicts,
        [Conflict {
            path: "ours.md".into(),
            copy: None
        }]
    );
    assert_eq!(b.changes(), []);
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

    pull(&a);
    assert!(matches!(a.repo.push(None), Ok(PushOutcome::Pushed { .. })));
    pull(&b);
    assert_eq!(b.files(), ["a.md", "b.md", "base.md"]);
    assert_eq!(a.files(), b.files());
}

// 12. line endings and missing final newlines

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

    // A clean merge of CRLF text keeps CRLF, and an edit keeps the missing final newline.
    a.write("crlf.md", "ONE\r\ntwo\r\nthree\r\n");
    a.write("no-newline.md", "still no final newline");
    commit_and_push(&a, "Edits");
    b.write("crlf.md", "one\r\ntwo\r\nTHREE\r\n");
    let out = pull(&b);
    assert_eq!(out.merged, ["crlf.md"]);
    assert_eq!(b.read("crlf.md"), b"ONE\r\ntwo\r\nTHREE\r\n");
    assert_eq!(b.read("no-newline.md"), b"still no final newline");
    assert_eq!(b.read("mixed.md"), files[2].1);
}

// Beyond the basics

#[test]
fn a_remote_rename_carries_the_local_edit() {
    let body = "A long enough entry for rename detection to recognise it.\n".repeat(4);
    let (_remote, a, b) = two_devices(&[("2026-11-16-1402-old.md", &body)]);
    fs::rename(
        a.root().join("2026-11-16-1402-old.md"),
        a.root().join("2026-11-16-1402-new.md"),
    )
    .unwrap();
    commit_and_push(&a, "Rename");
    let edited = format!("{body}One more line from b.\n");
    b.write("2026-11-16-1402-old.md", &edited);

    let out = pull(&b);
    assert_eq!(out.conflicts, []);
    assert_eq!(b.files(), ["2026-11-16-1402-new.md"]);
    assert_eq!(b.text("2026-11-16-1402-new.md"), edited);
    assert_eq!(
        b.changes(),
        [("2026-11-16-1402-new.md".into(), ChangeKind::Modified)]
    );
}

#[test]
fn local_merge_commits_are_merged_not_rebased() {
    let (_remote, a, b) = two_devices(&[("base.md", "base\n")]);
    a.write("a.md", "a\n");
    commit_and_push(&a, "A");

    // B merges a side branch with plain git before pulling.
    b.write("b.md", "b\n");
    let main = b.commit("B");
    let git = b.git();
    let base = git.find_commit(main).unwrap().parent(0).unwrap();
    let mut index = git.index().unwrap();
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
    index.read_tree(&tree).unwrap();
    index.write().unwrap();
    git.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    let out = pull(&b);
    assert_eq!(out.rebased, 0);
    assert_eq!(b.history()[0].1, 2, "a merge commit joins the two sides");
    assert_eq!(b.files(), ["a.md", "b.md", "base.md", "side.md"]);
    assert_eq!(b.changes(), []);
    assert!(matches!(b.repo.push(None), Ok(PushOutcome::Pushed { .. })));
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
    a.commit("Track an ignored path");
    a.repo.push(None).unwrap();

    let out = pull(&b);
    assert_eq!(b.text("notes.local"), "tracked on a\n");
    let [Conflict {
        copy: Some(copy), ..
    }] = &out.conflicts[..]
    else {
        panic!(
            "expected the ignored file to be moved aside: {:?}",
            out.conflicts
        );
    };
    assert!(is_conflict_copy(copy, "notes", ".local"), "{copy}");
    assert_eq!(b.text(copy), "private, ignored\n");
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
    let out = b.repo.connect(&remote.url, None, &author()).unwrap();
    assert_eq!(out.state.branch, "journal");
    assert_eq!(out.rebased, 1);
    assert_eq!(b.files(), ["a.md", "b.md"]);
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

    pull(&b);
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
}

#[test]
fn rebase_and_uncommitted_edits_merge_together() {
    let (_remote, a, b) = two_devices(&[("entry.md", "1\n2\n3\n4\n5\n6\n7\n")]);
    a.write("entry.md", "1 from a\n2\n3\n4\n5\n6\n7\n");
    commit_and_push(&a, "A");
    b.write("entry.md", "1\n2\n3\n4 committed on b\n5\n6\n7\n");
    b.commit("B");
    b.write(
        "entry.md",
        "1\n2\n3\n4 committed on b\n5\n6\n7 typed on b\n",
    );

    let out = pull(&b);
    assert_eq!((out.rebased, out.conflicts.len()), (1, 0));
    assert_eq!(out.merged, ["entry.md"]);
    assert_eq!(
        b.committed("entry.md"),
        Some(b"1 from a\n2\n3\n4 committed on b\n5\n6\n7\n".to_vec())
    );
    assert_eq!(
        b.text("entry.md"),
        "1 from a\n2\n3\n4 committed on b\n5\n6\n7 typed on b\n"
    );
    assert_eq!(b.changes(), [("entry.md".into(), ChangeKind::Modified)]);
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

    let before = b.head();
    let result = b.repo.pull(None, &author());
    assert!(
        matches!(&result, Err(Error::Git(m)) if m.contains("symbolic link")),
        "{result:?}"
    );
    assert_eq!(b.head(), before);
    assert_eq!(b.files(), ["base.md"]);
    assert!(b.root().join("assets").symlink_metadata().is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn a_remote_rename_and_edit_with_a_local_edit_is_reported() {
    let body = "A long enough entry for rename detection to recognise it.\n".repeat(4);
    let (old, new) = ("2026-11-16-1402-old.md", "2026-11-16-1402-new.md");
    let (_remote, a, b) = two_devices(&[(old, &body)]);
    a.remove(old);
    a.write(new, format!("Top line from a.\n{body}"));
    commit_and_push(&a, "Rename and edit");
    let local = format!("{body}Bottom line from b.\n");
    b.write(old, &local);

    let out = pull(&b);
    assert_eq!(
        b.text(new),
        format!("Top line from a.\n{body}Bottom line from b.\n")
    );
    assert_eq!(b.text(old), local, "b's exact bytes stay on disk");
    assert_eq!(
        out.conflicts,
        [Conflict {
            path: new.into(),
            copy: Some(old.into())
        }]
    );
}

/// A small PNG with some texture; `seed` changes it.
fn png(seed: u32) -> Vec<u8> {
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

#[test]
fn photos_added_to_one_entry_on_two_devices_both_survive() {
    use mdgj_core::{ImageOpts, Journal};
    const OPTS: ImageOpts = ImageOpts {
        max_edge: 1024,
        quality: 60,
    };
    let entry = "2026-11-16-1402-run.md";
    let base = "Line one.\nLine two.\nLine three.\n";
    for (b_commits_first, b_seed) in [(false, 1), (true, 1), (false, 0), (true, 0)] {
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

        let out = pull(&b);
        let case = format!(
            "b commits first: {b_commits_first}, same photo: {}",
            b_seed == 0
        );
        assert_eq!(out.conflicts, [], "{case}");
        assert_eq!(
            b.text(entry),
            format!("Line one.\n![]({from_a})\nLine two.\nLine three.\n![]({from_b})\n"),
            "{case}"
        );
        assert_eq!(b.read(&from_a), a.read(&from_a), "{case}");
        assert_eq!(b.read(&from_b), b_bytes, "{case}");
        assert_eq!(from_a == from_b, b_seed == 0, "{case}");
    }
}
