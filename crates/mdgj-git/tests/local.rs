//! Local-only operations: init, status, commit (all or chosen files) and diff, with no remote.

mod common;

use std::fs;
use std::path::Path;

use common::{author, Device};
use mdgj_git::{ChangeKind, Error, FileChange, LineKind, Repo, SyncState};

#[test]
fn init_commit_and_status() {
    let d = Device::new();
    assert_eq!(
        d.repo.sync_state().unwrap(),
        SyncState {
            has_remote: false,
            branch: "main".into(),
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
            last_fetch: None,
            merge: None,
            last_sync: None,
        }
    );

    d.write("2026-11-16-1402-run.md", "ran 5k\n");
    d.write(
        "assets/2026-11-16-1402-run-1.jpg",
        [0xff, 0xd8, 0x00, 0x0d, 0x0a],
    );
    assert_eq!(
        d.changes(),
        [
            ("2026-11-16-1402-run.md".into(), ChangeKind::Added),
            ("assets/2026-11-16-1402-run-1.jpg".into(), ChangeKind::Added),
        ]
    );

    assert_eq!(d.repo.commit_all("", &author()), Err(Error::EmptyMessage));
    assert_eq!(
        d.repo.commit_all(" \n\t ", &author()),
        Err(Error::EmptyMessage)
    );
    assert_eq!(
        d.repo
            .commit(" ", &["2026-11-16-1402-run.md".into()], &author()),
        Err(Error::EmptyMessage)
    );
    assert!(
        d.git().head().is_err(),
        "a rejected commit must not create one"
    );

    d.commit("First run");
    assert_eq!(d.changes(), []);
    assert_eq!(d.repo.commit_all("Again", &author()), Ok(None));
    assert_eq!(d.history(), [("First run".into(), 0)]);

    let git = d.git();
    let commit = git.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.author().name(), Ok("Test Author"));
    assert_eq!(commit.author().email(), Ok("test@example.com"));
}

#[test]
fn status_reports_modifications_deletions_and_renames() {
    let d = Device::new();
    let long = "A morning entry that is long enough for rename detection to pair it up.\n";
    d.write("2026-11-16-1402-run.md", "ran 5k\n");
    d.write("2026-11-16-0800-morning.md", long.repeat(4));
    d.write("assets/a-1.jpg", "jpeg");
    d.commit("Base");

    d.write("2026-11-16-1402-run.md", "ran 5k\nfelt good\n");
    d.remove("assets/a-1.jpg");
    fs::rename(
        d.root().join("2026-11-16-0800-morning.md"),
        d.root().join("2026-11-16-0800-sunrise.md"),
    )
    .unwrap();
    let mut status = d.repo.status().unwrap();
    status.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        status,
        [
            FileChange {
                path: "2026-11-16-0800-sunrise.md".into(),
                kind: ChangeKind::Renamed,
                old_path: Some("2026-11-16-0800-morning.md".into()),
            },
            FileChange {
                path: "2026-11-16-1402-run.md".into(),
                kind: ChangeKind::Modified,
                old_path: None,
            },
            FileChange {
                path: "assets/a-1.jpg".into(),
                kind: ChangeKind::Deleted,
                old_path: None,
            },
        ]
    );

    d.commit("Edits");
    assert_eq!(d.changes(), []);
    assert_eq!(d.committed("assets/a-1.jpg"), None);
    assert_eq!(
        d.committed("2026-11-16-0800-sunrise.md"),
        Some(long.repeat(4).into_bytes())
    );
}

#[test]
fn status_never_writes_to_the_repository() {
    let d = Device::new();
    d.write("tracked.md", "tracked\n");
    d.commit("Base");
    let index = fs::read(d.root().join(".git/index")).unwrap();

    d.write("tracked.md", "edited, not staged\n");
    d.write("untracked.md", "untracked\n");
    assert_eq!(d.changes().len(), 2);
    d.repo.sync_state().unwrap();
    d.repo.diff("tracked.md").unwrap();

    assert_eq!(fs::read(d.root().join(".git/index")).unwrap(), index);
    let git = d.git();
    for content in ["edited, not staged\n", "untracked\n"] {
        let id = git2::Oid::hash_object(git2::ObjectType::Blob, content.as_bytes()).unwrap();
        assert!(git.find_blob(id).is_err(), "status stored {content:?}");
    }
}

#[test]
fn ignored_files_are_neither_listed_nor_committed() {
    let d = Device::new();
    d.write(".gitignore", "*.tmp\n");
    d.write("scratch.tmp", "ignored\n");
    d.write("entry.md", "kept\n");
    assert_eq!(
        d.changes(),
        [
            (".gitignore".into(), ChangeKind::Added),
            ("entry.md".into(), ChangeKind::Added),
        ]
    );
    d.commit("Base");
    assert_eq!(d.committed("scratch.tmp"), None);
}

#[test]
fn opening_creates_the_folder_and_reopening_keeps_history() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Application Support/mdgitjournal/journal");
    let repo = Repo::open_or_init(&root).unwrap();
    fs::write(root.join("a.md"), "a\n").unwrap();
    repo.commit_all("First", &author()).unwrap().unwrap();
    drop(repo);

    let repo = Repo::open_or_init(&root).unwrap();
    assert_eq!(repo.sync_state().unwrap().branch, "main");
    assert_eq!(repo.status().unwrap(), []);
}

#[test]
fn bare_repositories_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    git2::Repository::init_bare(dir.path()).unwrap();
    assert!(matches!(Repo::open_or_init(dir.path()), Err(Error::Git(_))));
}

#[test]
fn remote_urls_are_stored_without_credentials() {
    let d = Device::new();
    d.repo
        .set_remote("https://x-access-token:ghs_secret@github.com/o/journal.git")
        .unwrap();
    assert_eq!(
        d.repo.remote_url().as_deref(),
        Some("https://github.com/o/journal.git")
    );
    let config = fs::read_to_string(d.root().join(".git/config")).unwrap();
    assert!(!config.contains("ghs_secret"));

    d.repo.remove_remote().unwrap();
    assert_eq!(d.repo.remote_url(), None);
    assert!(!d.repo.sync_state().unwrap().has_remote);
    d.repo.remove_remote().unwrap();
}

#[test]
fn repos_can_move_to_a_worker_thread() {
    fn send<T: Send>(_: T) {}
    send(Device::new().repo);
}

// 19. commit(message, paths)

#[test]
fn only_the_ticked_files_are_committed() {
    let d = Device::new();
    let long = "An entry that is long enough for rename detection to pair it up.\n".repeat(4);
    d.write("keep.md", "keep\n");
    d.write("old-name.md", &long);
    d.write("gone.md", "gone\n");
    d.commit("Base");

    d.write("keep.md", "keep, edited\n");
    d.write("new.md", "new\n");
    d.write("other.md", "not ticked\n");
    d.remove("gone.md");
    fs::rename(d.root().join("old-name.md"), d.root().join("new-name.md")).unwrap();

    let ticked = ["keep.md", "new-name.md", "gone.md", "not-changed.md"].map(String::from);
    let id = d.repo.commit("Some", &ticked, &author()).unwrap().unwrap();
    assert_eq!(d.head().to_string(), id);
    assert_eq!(d.committed("keep.md"), Some(b"keep, edited\n".to_vec()));
    assert_eq!(d.committed("new-name.md"), Some(long.clone().into_bytes()));
    assert_eq!(
        d.committed("old-name.md"),
        None,
        "a ticked rename removes the old path"
    );
    assert_eq!(d.committed("gone.md"), None);
    assert_eq!(d.committed("new.md"), None);
    assert_eq!(d.committed("other.md"), None);
    assert_eq!(
        d.changes(),
        [
            ("new.md".into(), ChangeKind::Added),
            ("other.md".into(), ChangeKind::Added),
        ]
    );
    // A rename may be ticked by its old name too.
    fs::rename(d.root().join("new-name.md"), d.root().join("third-name.md")).unwrap();
    d.repo
        .commit("Rename", &["new-name.md".into()], &author())
        .unwrap()
        .unwrap();
    assert_eq!(d.committed("third-name.md"), Some(long.into_bytes()));

    // Nothing ticked is still changed: no commit.
    let head = d.head();
    assert_eq!(
        d.repo.commit("Nothing", &["keep.md".into()], &author()),
        Ok(None)
    );
    assert_eq!(d.repo.commit("Nothing", &[], &author()), Ok(None));
    assert_eq!(d.head(), head);
}

#[test]
fn something_staged_with_git_stays_staged() {
    let d = Device::new();
    d.write("a.md", "a\n");
    d.write("b.md", "b\n");
    d.commit("Base");
    // Staged with the git command line, then edited again.
    d.write("b.md", "b, staged\n");
    let git = d.git();
    let mut index = git.index().unwrap();
    index.add_path(Path::new("b.md")).unwrap();
    index.write().unwrap();
    d.write("b.md", "b, staged, then edited\n");
    d.write("a.md", "a, edited\n");

    d.repo
        .commit("Only a", &["a.md".into()], &author())
        .unwrap()
        .unwrap();
    assert_eq!(d.committed("b.md"), Some(b"b\n".to_vec()));
    let mut index = git.index().unwrap();
    index.read(true).unwrap();
    let staged = index.get_path(Path::new("b.md"), 0).unwrap();
    let blob = git.find_blob(staged.id).unwrap();
    assert_eq!(blob.content(), b"b, staged\n");
    let a = index.get_path(Path::new("a.md"), 0).unwrap();
    assert_eq!(git.find_blob(a.id).unwrap().content(), b"a, edited\n");
    assert_eq!(d.changes(), [("b.md".into(), ChangeKind::Modified)]);
    if let Some(status) = d.cli_ok(&["status", "--porcelain"]) {
        assert_eq!(status, "MM b.md\n", "a.md is clean for git too");
    }
}

#[test]
fn commits_work_on_an_unborn_branch_and_keep_the_stat_cache() {
    let d = Device::new();
    d.write("a.md", "a\n");
    d.write("b.md", "b\n");
    d.repo
        .commit("A only", &["a.md".into()], &author())
        .unwrap()
        .unwrap();
    assert_eq!(d.history(), [("A only".into(), 0)]);
    assert_eq!(d.changes(), [("b.md".into(), ChangeKind::Added)]);
    let git = d.git();
    let index = git.index().unwrap();
    let entry = index.get_path(Path::new("a.md"), 0).unwrap();
    assert!(
        entry.file_size > 0 && entry.mtime.seconds() > 0,
        "{entry:?}"
    );
}

// 20. diff

#[test]
fn diffs_show_modified_added_deleted_renamed_and_binary_files() {
    let d = Device::new();
    let lines: String = (1..=12).map(|n| format!("line {n}\n")).collect();
    let long = "A long enough entry for rename detection to recognise it.\n".repeat(4);
    d.write("entry.md", &lines);
    d.write("gone.md", "bye\n");
    d.write("old.md", &long);
    d.write("assets/photo.jpg", [0xff, 0xd8, 0x00, 0x01]);
    d.commit("Base");

    d.write("entry.md", lines.replace("line 6\n", "line six\r\n"));
    d.write("added.md", "hello\nworld");
    d.remove("gone.md");
    fs::rename(d.root().join("old.md"), d.root().join("new.md")).unwrap();
    d.write("new.md", format!("{long}one more\n"));
    d.write("assets/photo.jpg", [0xff, 0xd8, 0x00, 0x02]);

    let diff = d.repo.diff("entry.md").unwrap();
    assert_eq!(
        (diff.kind, diff.binary, diff.old_path),
        (ChangeKind::Modified, false, None)
    );
    let [hunk] = &diff.hunks[..] else {
        panic!("{:?}", diff.hunks)
    };
    assert_eq!(
        (
            hunk.old_start,
            hunk.old_lines,
            hunk.new_start,
            hunk.new_lines
        ),
        (3, 7, 3, 7)
    );
    let shown: Vec<(LineKind, &str)> = hunk
        .lines
        .iter()
        .map(|l| (l.kind, l.text.as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            (LineKind::Context, "line 3"),
            (LineKind::Context, "line 4"),
            (LineKind::Context, "line 5"),
            (LineKind::Removed, "line 6"),
            (LineKind::Added, "line six"),
            (LineKind::Context, "line 7"),
            (LineKind::Context, "line 8"),
            (LineKind::Context, "line 9"),
        ]
    );

    let added = d.repo.diff("added.md").unwrap();
    assert_eq!(added.kind, ChangeKind::Added);
    let texts: Vec<_> = added.hunks[0]
        .lines
        .iter()
        .map(|l| (l.kind, l.text.clone()))
        .collect();
    assert_eq!(
        texts,
        [
            (LineKind::Added, "hello".into()),
            (LineKind::Added, "world".into())
        ]
    );

    let gone = d.repo.diff("gone.md").unwrap();
    assert_eq!(gone.kind, ChangeKind::Deleted);
    assert_eq!(gone.hunks[0].lines[0].kind, LineKind::Removed);
    assert_eq!(gone.hunks[0].lines[0].text, "bye");

    let renamed = d.repo.diff("new.md").unwrap();
    assert_eq!(renamed.kind, ChangeKind::Renamed);
    assert_eq!(renamed.old_path.as_deref(), Some("old.md"));
    let last = renamed.hunks.last().unwrap().lines.last().unwrap();
    assert_eq!(
        (last.kind, last.text.as_str()),
        (LineKind::Added, "one more")
    );

    let photo = d.repo.diff("assets/photo.jpg").unwrap();
    assert!(photo.binary);
    assert!(photo.hunks.is_empty());

    assert!(matches!(
        d.repo.diff("unchanged.md"),
        Err(Error::Invalid(_))
    ));
}
