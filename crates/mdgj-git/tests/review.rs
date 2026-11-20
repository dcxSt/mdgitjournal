//! Files changed on both sides: the Sync review writes nothing until Apply, then exactly the
//! user's choices.

mod common;

use chrono::TimeZone;
use common::{author, merged, no_conflict_markers, overlap, resolved, two_devices, Device, State};
use mdgj_git::{
    ChangeKind, Choice, Error, MergeChunk, OverlapKind, PullOutcome, PullPlan, Resolution, Resolved,
};

const ENTRY: &str = "2026-11-16-1402-run.md";
const BASE: &str = "# Run\n\nFirst paragraph.\n\nSecond paragraph.\n\nThird paragraph.\n";

/// The "hhhmm" of a Keep both copy: GitHub's newest commit time on this device.
fn stamp(d: &Device) -> String {
    let git = d.git();
    let remote = git
        .find_reference("refs/remotes/origin/main")
        .unwrap()
        .peel_to_commit()
        .unwrap();
    chrono::Local
        .timestamp_opt(remote.time().seconds(), 0)
        .unwrap()
        .format("%Hh%M")
        .to_string()
}

/// Pulls on `d`, expecting a review that changed nothing but origin's branch.
fn review(d: &Device) -> (mdgj_git::PullPreview, State) {
    let before = d.state();
    let preview = d.pull_review();
    assert_eq!(d.state(), before, "a review writes nothing");
    (preview, before)
}

// 6. Overlaps write nothing

#[test]
fn an_overlap_opens_a_review_and_changes_nothing() {
    let cases = [
        (
            "different paragraphs",
            BASE.replace("First paragraph.", "First, edited on a."),
            BASE.replace("Third paragraph.", "Third, edited on b."),
        ),
        (
            "the same line",
            BASE.replace("Second paragraph.", "Second, a."),
            BASE.replace("Second paragraph.", "Second, b."),
        ),
        (
            "adjacent lines",
            BASE.replace(
                "Second paragraph.",
                "A's line, right above\nSecond paragraph.",
            ),
            BASE.replace("Second paragraph.", "Second, b."),
        ),
    ];
    for (case, theirs, mine) in cases {
        let (_remote, a, b) = two_devices(&[(ENTRY, BASE)]);
        a.write(ENTRY, &theirs);
        a.push("A");
        b.write(ENTRY, &mine);
        b.write("other.md", "untracked\n");
        let origin_before = b.git().refname_to_id("refs/remotes/origin/main").unwrap();
        let (preview, before) = review(&b);
        let origin_after = b.git().refname_to_id("refs/remotes/origin/main").unwrap();
        assert_ne!(
            origin_before, origin_after,
            "{case}: the fetch moved origin"
        );
        assert_eq!(preview.plan, PullPlan::FastForward, "{case}");
        assert!(!preview.automatic, "{case}");
        let found = overlap(&preview, ENTRY);
        assert_eq!(found.kind, OverlapKind::BothModified, "{case}");
        assert_eq!(
            common::sides(found),
            (BASE.to_owned(), mine.clone(), theirs.clone()),
            "{case}"
        );
        assert_eq!(found.mine_path.as_deref(), Some(ENTRY));
        assert_eq!(found.theirs_path.as_deref(), Some(ENTRY));
        assert!(!found.committed, "nothing to commit in a fast-forward");
        assert_eq!(
            found.choices,
            [Choice::Mine, Choice::Theirs, Choice::Both, Choice::Merged]
        );
        assert!(found
            .copy
            .as_deref()
            .is_some_and(|c| common::is_github_copy(c, "2026-11-16-1402-run", ".md")));
        assert_eq!(
            found.suggestion.as_ref().unwrap().clean,
            case == "different paragraphs"
        );

        // Previewing again (local only) writes nothing either, objects included, and gives the
        // same id. Cancel is no call at all.
        let objects = b.objects();
        let again = b.repo.preview_pull().unwrap().unwrap();
        assert_eq!(again, preview, "{case}");
        assert_eq!(b.objects(), objects, "{case}: no object written");
        assert_eq!(b.state(), before, "{case}");
        no_conflict_markers(&b);
    }
}

// 7. Each resolution, fast-forward: resolutions stay uncommitted on top of GitHub's commits

#[test]
fn each_resolution_on_uncommitted_edits() {
    let theirs = BASE.replace("Second paragraph.", "Second, from GitHub.");
    let mine = BASE.replace("Second paragraph.", "Second, typed here.");
    let setup = || {
        let (remote, a, b) = two_devices(&[(ENTRY, BASE), ("x.md", "x\n")]);
        a.write(ENTRY, &theirs);
        a.write("a.md", "one-sided\n");
        a.push("A");
        b.write(ENTRY, &mine);
        b.write("draft.md", "draft\n");
        let (preview, _) = review(&b);
        (remote, a, b, preview)
    };

    // Keep mine.
    let (_r, a, b, preview) = setup();
    let summary = b.apply(&preview, &[resolved(ENTRY, Resolution::Mine)]);
    assert_eq!(summary.plan, PullPlan::FastForward);
    assert_eq!(b.head(), a.head(), "HEAD = GitHub's");
    assert_eq!(b.text(ENTRY), mine);
    assert_eq!(b.text("a.md"), "one-sided\n");
    assert_eq!(
        b.changes(),
        [
            (ENTRY.into(), ChangeKind::Modified),
            ("draft.md".into(), ChangeKind::Added)
        ]
    );
    assert_eq!(summary.resolved.len(), 1);
    assert_eq!(summary.resolved[0].choice, Choice::Mine);
    let paths: Vec<_> = summary.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["a.md"], "only the one-sided file changed on disk");

    // Keep GitHub's.
    let (_r, a, b, preview) = setup();
    b.apply(&preview, &[resolved(ENTRY, Resolution::Theirs)]);
    assert_eq!(b.head(), a.head());
    assert_eq!(b.text(ENTRY), theirs);
    assert_eq!(b.changes(), [("draft.md".into(), ChangeKind::Added)]);

    // Keep both, with the first copy name taken.
    let (_r, a, b, _) = setup();
    let first = format!("2026-11-16-1402-run.from-github-{}.md", stamp(&b));
    b.write(&first, "already here\n");
    let preview = b.repo.preview_pull().unwrap().unwrap();
    let copy = overlap(&preview, ENTRY).copy.clone().unwrap();
    assert_eq!(copy, first.replace(".md", "-2.md"));
    let summary = b.apply(&preview, &[resolved(ENTRY, Resolution::Both)]);
    assert_eq!(b.head(), a.head());
    assert_eq!(b.text(ENTRY), mine, "yours stays at the path");
    assert_eq!(b.text(&copy), theirs, "GitHub's is saved as the copy");
    assert_eq!(b.text(&first), "already here\n");
    assert_eq!(summary.resolved[0].copy.as_deref(), Some(copy.as_str()));
    assert!(summary
        .files
        .iter()
        .any(|f| f.path == copy && f.kind == ChangeKind::Added));
    assert_eq!(
        b.changes(),
        [
            (copy.clone(), ChangeKind::Added),
            (first.clone(), ChangeKind::Added),
            (ENTRY.into(), ChangeKind::Modified),
            ("draft.md".into(), ChangeKind::Added),
        ]
    );

    // Merged: exactly the given bytes.
    let (_r, a, b, preview) = setup();
    let text = "# Run\r\n\r\nWritten by hand in the merge view, no final newline";
    b.apply(&preview, &[merged(ENTRY, text)]);
    assert_eq!(b.head(), a.head());
    assert_eq!(b.read(ENTRY), text.as_bytes());
    assert_eq!(
        b.changes(),
        [
            (ENTRY.into(), ChangeKind::Modified),
            ("draft.md".into(), ChangeKind::Added)
        ]
    );
    no_conflict_markers(&b);
}

// 7. Each resolution, diverged with a committed overlap: a real merge commit

#[test]
fn each_resolution_of_a_committed_clash_makes_a_merge_commit() {
    let theirs = BASE.replace("Second paragraph.", "Second, from GitHub.");
    let mine = BASE.replace("Second paragraph.", "Second, committed here.");
    let setup = || {
        let (remote, a, b) = two_devices(&[(ENTRY, BASE), ("x.md", "x\n")]);
        a.write(ENTRY, &theirs);
        a.write("a.md", "one-sided\n");
        a.push("A");
        b.write(ENTRY, &mine);
        b.write("b.md", "committed on b\n");
        b.commit("B's edits");
        b.write("x.md", "x, uncommitted on b\n");
        let (preview, _) = review(&b);
        assert_eq!(preview.plan, PullPlan::Merge);
        assert_eq!(preview.local_count, 1);
        (remote, a, b, preview)
    };
    let check_merge = |b: &Device, a: &Device, local: git2::Oid, message: &str| {
        let git = b.git();
        let head = git.head().unwrap().peel_to_commit().unwrap();
        let parents: Vec<_> = head.parent_ids().collect();
        assert_eq!(parents, [local, a.head()]);
        assert_eq!(head.message().unwrap(), message);
        assert_eq!(head.author().name(), Ok("Test Author"));
        assert_eq!(b.committed("a.md"), Some(b"one-sided\n".to_vec()));
        assert_eq!(b.committed("b.md"), Some(b"committed on b\n".to_vec()));
        assert_eq!(
            b.committed("x.md"),
            Some(b"x\n".to_vec()),
            "uncommitted stays out"
        );
        assert_eq!(b.text("x.md"), "x, uncommitted on b\n");
        if let Some(log) = b.cli_ok(&["log", "--graph", "--format=%s"]) {
            assert!(log.starts_with("*   "), "{log}");
            assert!(log.contains("B's edits") && log.contains("|/"), "{log}");
        }
    };
    let message = "Merge from GitHub\n\n- 2026-11-16-1402-run.md: kept mine\n";

    let (_r, a, b, preview) = setup();
    let local = b.head();
    b.repo
        .apply_pull(
            &preview.id,
            &[resolved(ENTRY, Resolution::Mine)],
            Some(message),
            &author(),
        )
        .unwrap();
    check_merge(&b, &a, local, message);
    assert_eq!(b.committed(ENTRY), Some(mine.clone().into_bytes()));
    assert_eq!(b.changes(), [("x.md".into(), ChangeKind::Modified)]);
    if let Some(status) = b.cli_ok(&["status", "--porcelain"]) {
        assert_eq!(status, " M x.md\n");
    }

    let (_r, a, b, preview) = setup();
    let local = b.head();
    b.repo
        .apply_pull(
            &preview.id,
            &[resolved(ENTRY, Resolution::Theirs)],
            Some(message),
            &author(),
        )
        .unwrap();
    check_merge(&b, &a, local, message);
    assert_eq!(b.committed(ENTRY), Some(theirs.clone().into_bytes()));
    assert_eq!(b.text(ENTRY), theirs);
    assert_eq!(b.changes(), [("x.md".into(), ChangeKind::Modified)]);

    let (_r, a, b, preview) = setup();
    let local = b.head();
    let copy = overlap(&preview, ENTRY).copy.clone().unwrap();
    b.repo
        .apply_pull(
            &preview.id,
            &[resolved(ENTRY, Resolution::Both)],
            Some(message),
            &author(),
        )
        .unwrap();
    check_merge(&b, &a, local, message);
    assert_eq!(b.committed(ENTRY), Some(mine.clone().into_bytes()));
    assert_eq!(b.committed(&copy), Some(theirs.clone().into_bytes()));
    assert_eq!(b.text(&copy), theirs);
    assert_eq!(b.changes(), [("x.md".into(), ChangeKind::Modified)]);

    let (_r, a, b, preview) = setup();
    let local = b.head();
    let text = "# Run\n\nSecond, from GitHub and committed here.\n";
    b.repo
        .apply_pull(
            &preview.id,
            &[merged(ENTRY, text)],
            Some("\n\nMerge  \n\n\n"),
            &author(),
        )
        .unwrap();
    check_merge(&b, &a, local, "Merge\n");
    assert_eq!(b.committed(ENTRY), Some(text.as_bytes().to_vec()));
    assert_eq!(b.read(ENTRY), text.as_bytes());
    assert_eq!(b.changes(), [("x.md".into(), ChangeKind::Modified)]);

    // The other device pulls the merge at once.
    assert!(matches!(
        b.repo.push(None),
        Ok(mdgj_git::PushOutcome::Pushed { .. })
    ));
    let summary = a.pull_applied();
    assert_eq!(summary.plan, PullPlan::FastForward);
    assert_eq!(a.text(ENTRY), text);
}

#[test]
fn a_committed_clash_with_more_edits_on_top_commits_only_what_was_committed() {
    let typed = BASE
        .replace("First paragraph.", "First, committed.")
        .replace("Third paragraph.", "Third, typed.");
    let setup = || {
        let (remote, a, b) = two_devices(&[(ENTRY, BASE)]);
        a.write(ENTRY, BASE.replace("First paragraph.", "First, GitHub."));
        a.push("A");
        b.write(ENTRY, BASE.replace("First paragraph.", "First, committed."));
        b.commit("B");
        b.write(ENTRY, &typed);
        let (preview, _) = review(&b);
        (remote, a, b, preview)
    };

    // Keep mine: the merge commit records the committed version; the typed line stays a change.
    let (_r, _a, b, preview) = setup();
    assert_eq!(preview.plan, PullPlan::Merge);
    let found = overlap(&preview, ENTRY);
    assert!(found.committed);
    assert_eq!(common::sides(found).1, typed, "mine is the working tree");
    b.apply(&preview, &[resolved(ENTRY, Resolution::Mine)]);
    assert_eq!(
        b.committed(ENTRY).unwrap(),
        BASE.replace("First paragraph.", "First, committed.")
            .into_bytes()
    );
    assert_eq!(b.text(ENTRY), typed);
    assert_eq!(b.changes(), [(ENTRY.into(), ChangeKind::Modified)]);

    // Merged: the merge without the typed line, which stays uncommitted on top of it.
    let (_r, _a, b, preview) = setup();
    let merged_text = BASE
        .replace("First paragraph.", "First, merged.")
        .replace("Third paragraph.", "Third, typed.");
    b.apply(&preview, &[merged(ENTRY, &merged_text)]);
    assert_eq!(
        b.committed(ENTRY).unwrap(),
        BASE.replace("First paragraph.", "First, merged.")
            .into_bytes()
    );
    assert_eq!(b.text(ENTRY), merged_text);
    assert_eq!(b.changes(), [(ENTRY.into(), ChangeKind::Modified)]);

    // A merge that rewrote the typed line itself records it, as the user resolved it.
    let (_r, _a, b, preview) = setup();
    let merged_text = BASE
        .replace("First paragraph.", "First, merged.")
        .replace("Third paragraph.", "Third, typed and merged.");
    b.apply(&preview, &[merged(ENTRY, &merged_text)]);
    assert_eq!(b.text(ENTRY), merged_text);
    assert_eq!(b.committed(ENTRY).unwrap(), merged_text.into_bytes());
    assert_eq!(b.changes(), []);
}

// 8. Diverged, overlap only in uncommitted edits: a merge commit of the commits, resolution
// uncommitted

#[test]
fn an_overlap_in_uncommitted_edits_with_unpushed_commits_elsewhere_makes_a_merge_commit() {
    let (_remote, a, b) = two_devices(&[(ENTRY, BASE)]);
    a.write(ENTRY, BASE.replace("First paragraph.", "First, GitHub."));
    a.push("A");
    b.write("b1.md", "b1\n");
    b.commit("b1");
    b.write("b2.md", "b2\n");
    b.commit("b2");
    b.write(
        ENTRY,
        BASE.replace("Third paragraph.", "Third, typed here."),
    );

    let (preview, _) = review(&b);
    assert_eq!(preview.plan, PullPlan::Merge, "decision C: no rebase");
    assert!(!preview.automatic);
    assert_eq!(preview.local_count, 2);
    let messages: Vec<_> = preview.local.iter().map(|c| c.message.as_str()).collect();
    assert_eq!(messages, ["b2", "b1"]);
    let found = overlap(&preview, ENTRY);
    assert!(!found.committed, "only uncommitted edits clash");
    let suggestion = found.suggestion.clone().unwrap();
    assert!(suggestion.clean);
    let text = suggestion.text().unwrap();
    let local = b.head();
    let summary = b.apply(&preview, &[merged(ENTRY, &text)]);
    assert_eq!(summary.plan, PullPlan::Merge);
    assert_eq!(summary.rebased, 0);
    let git = b.git();
    let commit = git.find_commit(b.head()).unwrap();
    assert_eq!(summary.merge_commit, Some(commit.id().to_string()));
    let parents: Vec<_> = commit.parent_ids().collect();
    assert_eq!(parents, [local, a.head()], "B's commits keep their ids");
    assert_eq!(
        b.committed(ENTRY).unwrap(),
        BASE.replace("First paragraph.", "First, GitHub.")
            .into_bytes(),
        "the merge commit records commits only"
    );
    assert_eq!(
        b.text(ENTRY),
        BASE.replace("First paragraph.", "First, GitHub.")
            .replace("Third paragraph.", "Third, typed here.")
    );
    assert_eq!(b.changes(), [(ENTRY.into(), ChangeKind::Modified)]);
}

// 9. Suggestions

#[test]
fn suggestions_prefill_the_merge() {
    let (_remote, a, b) = two_devices(&[(ENTRY, "one\ntwo\nthree\nfour\nfive\nsix\n")]);
    a.write(ENTRY, "ONE\ntwo\nthree\nfour\nFIVE\nsix\n");
    a.push("A");
    b.write(ENTRY, "ONE\ntwo\nthree\n4\nfive\n");
    let (preview, _) = review(&b);
    let suggestion = overlap(&preview, ENTRY).suggestion.clone().unwrap();
    let s = |text: &str| text.to_owned();
    assert_eq!(
        suggestion.chunks,
        [
            MergeChunk::Both {
                text: s("ONE\n"),
                base: s("one\n")
            },
            MergeChunk::Same {
                text: s("two\nthree\n")
            },
            MergeChunk::Clash {
                base: s("four\nfive\nsix\n"),
                mine: s("4\nfive\n"),
                theirs: s("four\nFIVE\nsix\n")
            },
        ]
    );
    assert!(!suggestion.clean);

    // CRLF and a missing final newline survive chunking and the merged write.
    let base = "one\r\ntwo\r\nthree\r\nfour";
    let (_remote, a, b) = two_devices(&[(ENTRY, base)]);
    a.write(ENTRY, "ONE\r\ntwo\r\nthree\r\nfour");
    a.push("A");
    b.write(ENTRY, "one\r\ntwo\r\nthree\r\nfour, here");
    let (preview, _) = review(&b);
    let suggestion = overlap(&preview, ENTRY).suggestion.clone().unwrap();
    assert!(suggestion.clean);
    let text = suggestion.text().unwrap();
    assert_eq!(text, "ONE\r\ntwo\r\nthree\r\nfour, here");
    assert_eq!(
        suggestion.chunks.last(),
        Some(&MergeChunk::Mine {
            text: s("four, here"),
            base: s("four")
        })
    );
    b.apply(&preview, &[merged(ENTRY, &text)]);
    assert_eq!(b.read(ENTRY), text.as_bytes());

    // Deleted lines are chunks with empty text that keep their base.
    let (_remote, a, b) = two_devices(&[(ENTRY, "a\nb\nc\nd\ne\n")]);
    a.write(ENTRY, "a\nb\nc\nd\n");
    a.push("A");
    b.write(ENTRY, "a\nc\nd\ne\n");
    let (preview, _) = review(&b);
    let chunks = overlap(&preview, ENTRY).suggestion.clone().unwrap().chunks;
    assert!(chunks.contains(&MergeChunk::Mine {
        text: s(""),
        base: s("b\n")
    }));
    assert!(chunks.contains(&MergeChunk::Theirs {
        text: s(""),
        base: s("e\n")
    }));
}

// 10. Binary files

#[test]
fn two_different_images_at_one_path_are_binary() {
    let path = "assets/2026-11-16-1402-run-5f3a09c2.jpg";
    let (_remote, a, b) = two_devices(&[("base.md", "base\n")]);
    let (ours, theirs) = (
        vec![0xff, 0xd8, 0x00, 1, 2, 3],
        vec![0xff, 0xd8, 0x00, 9, 9],
    );
    a.write(path, &theirs);
    a.push("A");
    b.write(path, &ours);
    let (preview, _) = review(&b);
    let found = overlap(&preview, path);
    assert_eq!(found.kind, OverlapKind::BothAdded);
    assert!(found.binary);
    assert_eq!(found.suggestion, None);
    assert_eq!(
        (found.mine_path.as_deref(), found.theirs_path.as_deref()),
        (Some(path), Some(path))
    );
    assert_eq!(found.choices, [Choice::Mine, Choice::Theirs, Choice::Both]);
    let copy = found.copy.clone().unwrap();
    assert!(
        common::is_github_copy(&copy, "assets/2026-11-16-1402-run-5f3a09c2", ".jpg"),
        "{copy}"
    );
    b.apply(&preview, &[resolved(path, Resolution::Both)]);
    assert_eq!(b.read(path), ours);
    assert_eq!(b.read(&copy), theirs);
}

// 11. Deleted on one side, modified on the other

#[test]
fn delete_and_modify_offer_keep_or_take_and_do_each() {
    let setup = || {
        let (remote, a, b) = two_devices(&[("ours.md", "o\n"), ("theirs.md", "t\n")]);
        a.remove("ours.md");
        a.write("theirs.md", "t, edited on GitHub\n");
        a.push("A");
        b.write("ours.md", "o, edited here\n");
        b.remove("theirs.md");
        let (preview, _) = review(&b);
        (remote, a, b, preview)
    };
    let (_r, _a, b, preview) = setup();
    let ours = overlap(&preview, "ours.md");
    assert_eq!(ours.kind, OverlapKind::ModifiedDeleted);
    assert_eq!(ours.choices, [Choice::Mine, Choice::Theirs]);
    assert_eq!((ours.copy.clone(), ours.suggestion.clone()), (None, None));
    assert_eq!(ours.theirs_path, None);
    let theirs = overlap(&preview, "theirs.md");
    assert_eq!(theirs.kind, OverlapKind::DeletedModified);
    assert_eq!(theirs.choices, [Choice::Mine, Choice::Theirs]);
    assert_eq!(theirs.mine_path, None);

    // Keep mine: my edit stays, my deletion stays.
    b.apply(
        &preview,
        &[
            resolved("ours.md", Resolution::Mine),
            resolved("theirs.md", Resolution::Mine),
        ],
    );
    assert_eq!(b.text("ours.md"), "o, edited here\n");
    assert!(!b.exists("theirs.md"));
    assert_eq!(
        b.changes(),
        [
            ("ours.md".into(), ChangeKind::Added),
            ("theirs.md".into(), ChangeKind::Deleted)
        ]
    );

    // Keep GitHub's: the deletion and the edit arrive.
    let (_r, _a, b, preview) = setup();
    b.apply(
        &preview,
        &[
            resolved("ours.md", Resolution::Theirs),
            resolved("theirs.md", Resolution::Theirs),
        ],
    );
    assert!(!b.exists("ours.md"));
    assert_eq!(b.text("theirs.md"), "t, edited on GitHub\n");
    assert_eq!(b.changes(), []);
}

// 13. Staleness

#[test]
fn a_stale_review_writes_nothing_and_a_fresh_one_applies() {
    type Change = fn(&Device, &Device);
    let changes: [(&str, Change); 4] = [
        ("GitHub moved", |a, b| {
            a.write("later.md", "later\n");
            a.push("Later");
            b.repo.fetch(None).unwrap();
        }),
        ("an entry edited on disk", |_, b| {
            b.write(ENTRY, "edited in vim\n")
        }),
        ("a new untracked file", |_, b| b.write("new.md", "new\n")),
        ("a commit", |_, b| {
            b.write("x.md", "x2\n");
            b.repo
                .commit("X", &["x.md".into()], &author())
                .unwrap()
                .unwrap();
        }),
    ];
    for (case, change) in changes {
        let (_remote, a, b) = two_devices(&[(ENTRY, BASE), ("x.md", "x\n")]);
        a.write(ENTRY, BASE.replace("First paragraph.", "First, GitHub."));
        a.push("A");
        b.write(ENTRY, BASE.replace("First paragraph.", "First, here."));
        let (preview, _) = review(&b);
        change(&a, &b);
        let before = b.state();
        let choice = [resolved(ENTRY, Resolution::Theirs)];
        assert_eq!(
            b.repo
                .apply_pull(&preview.id, &choice, Some("M"), &author()),
            Err(Error::StaleReview),
            "{case}"
        );
        assert_eq!(
            b.repo
                .start_external_merge(&preview.id, Some("M"), &author()),
            Err(Error::StaleReview),
            "{case}"
        );
        assert_eq!(b.state(), before, "{case}: nothing written");

        let fresh = b.repo.preview_pull().unwrap().unwrap();
        assert_ne!(fresh.id, preview.id, "{case}");
        let resolutions: Vec<Resolved> = fresh
            .overlaps
            .iter()
            .map(|o| resolved(&o.path, Resolution::Theirs))
            .collect();
        b.repo
            .apply_pull(&fresh.id, &resolutions, Some("Merge"), &author())
            .unwrap();
        assert_eq!(b.repo.preview_pull(), Ok(None), "{case}");
    }
}

#[test]
fn an_apply_that_fails_after_writing_puts_everything_back() {
    // A first pull, so there is an undo record to keep.
    let (_remote, a, b) = two_devices(&[(ENTRY, BASE), ("x.md", "x\n")]);
    a.write("first.md", "first\n");
    a.push("First");
    b.pull_applied();
    a.write(ENTRY, BASE.replace("First paragraph.", "First, GitHub."));
    a.write("x.md", "x from GitHub\n");
    a.write("new.md", "new\n");
    a.push("A");
    b.write(ENTRY, BASE.replace("First paragraph.", "First, here."));
    let (preview, before) = review(&b);
    assert!(before.records[0].is_some(), "the first pull's undo record");

    // Files and index are written, then the branch can't move: git holds its lock.
    let lock = b.root().join(".git/refs/heads/main.lock");
    std::fs::write(&lock, "").unwrap();
    let result = b.repo.apply_pull(
        &preview.id,
        &[resolved(ENTRY, Resolution::Theirs)],
        None,
        &author(),
    );
    std::fs::remove_file(&lock).unwrap();
    assert!(
        matches!(result, Err(Error::Git(_) | Error::Io(_))),
        "{result:?}"
    );
    assert_eq!(
        b.state(),
        before,
        "files, index, refs and the old undo record are back"
    );

    let summary = b.apply(&preview, &[resolved(ENTRY, Resolution::Theirs)]);
    assert_eq!(b.text("x.md"), "x from GitHub\n");
    assert_eq!(b.repo.last_sync().unwrap(), Some(summary));
}

// 14. Invalid resolutions

#[test]
fn resolutions_must_match_the_review() {
    let (_remote, a, b) = two_devices(&[
        (ENTRY, BASE),
        ("gone.md", "g\n"),
        ("assets/p.jpg", "\u{0}jpeg"),
    ]);
    a.write(ENTRY, BASE.replace("First paragraph.", "First, GitHub."));
    a.remove("gone.md");
    a.write("assets/p.jpg", "\u{0}jpeg, GitHub");
    a.push("A");
    b.write(ENTRY, BASE.replace("First paragraph.", "First, here."));
    b.write("gone.md", "g, edited\n");
    b.write("assets/p.jpg", "\u{0}jpeg, here");
    let (preview, before) = review(&b);
    assert_eq!(preview.overlaps.len(), 3);
    let good = || {
        vec![
            resolved(ENTRY, Resolution::Mine),
            resolved("gone.md", Resolution::Mine),
            resolved("assets/p.jpg", Resolution::Mine),
        ]
    };
    let mut missing = good();
    missing.pop();
    let mut extra = good();
    extra.push(resolved("other.md", Resolution::Mine));
    let mut twice = good();
    twice.push(resolved(ENTRY, Resolution::Theirs));
    let mut both_on_delete = good();
    both_on_delete[1] = resolved("gone.md", Resolution::Both);
    let mut merged_binary = good();
    merged_binary[2] = merged("assets/p.jpg", "text");
    let mut merged_delete = good();
    merged_delete[1] = merged("gone.md", "text");
    for (case, resolutions) in [
        ("missing", missing),
        ("extra", extra),
        ("twice", twice),
        ("both on delete/modify", both_on_delete),
        ("merged on binary", merged_binary),
        ("merged on delete/modify", merged_delete),
    ] {
        let result = b
            .repo
            .apply_pull(&preview.id, &resolutions, None, &author());
        assert!(
            matches!(result, Err(Error::Invalid(_))),
            "{case}: {result:?}"
        );
        assert_eq!(b.state(), before, "{case}");
    }
    b.repo
        .apply_pull(&preview.id, &good(), None, &author())
        .unwrap();
}

#[test]
fn a_merge_needs_a_message() {
    let (_remote, a, b) = two_devices(&[(ENTRY, BASE)]);
    a.write(ENTRY, BASE.replace("First paragraph.", "First, GitHub."));
    a.push("A");
    b.write(ENTRY, BASE.replace("First paragraph.", "First, here."));
    b.commit("B");
    let (preview, before) = review(&b);
    assert_eq!(preview.plan, PullPlan::Merge);
    for message in [None, Some(""), Some(" \n ")] {
        assert_eq!(
            b.repo.apply_pull(
                &preview.id,
                &[resolved(ENTRY, Resolution::Mine)],
                message,
                &author()
            ),
            Err(Error::EmptyMessage)
        );
        assert_eq!(b.state(), before);
    }
    // The message is ignored when no merge commit is made.
    let (_remote, a, b) = two_devices(&[(ENTRY, BASE)]);
    a.write(ENTRY, BASE.replace("First paragraph.", "First, GitHub."));
    a.push("A");
    b.write(ENTRY, BASE.replace("First paragraph.", "First, here."));
    let (preview, _) = review(&b);
    b.repo
        .apply_pull(
            &preview.id,
            &[resolved(ENTRY, Resolution::Mine)],
            Some("ignored"),
            &author(),
        )
        .unwrap();
    assert_eq!(b.history()[0], ("A".into(), 1));
}

#[test]
fn pull_reviews_say_review_and_up_to_date_is_none() {
    let (_remote, a, b) = two_devices(&[(ENTRY, BASE)]);
    a.write(ENTRY, BASE.replace("First paragraph.", "First, GitHub."));
    a.push("A");
    b.write(ENTRY, BASE.replace("First paragraph.", "First, here."));
    let PullOutcome::Review { state, preview } = b.pull() else {
        panic!("expected a review");
    };
    assert_eq!(state.behind, 1);
    assert_eq!(preview.branch, "main");
    assert_eq!(preview.incoming_count, 1);
    assert_eq!(preview.incoming[0].message, "A");
    assert!(preview.id.starts_with("main:"), "{}", preview.id);
}
