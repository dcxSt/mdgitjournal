//! Local-only operations: init, status and commit, with no remote involved.

mod common;

use common::{author, Device};
use mdgj_git::{ChangeKind, Error, FileChange, Repo, SyncState};

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
    std::fs::rename(
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
    let index = std::fs::read(d.root().join(".git/index")).unwrap();

    d.write("tracked.md", "edited, not staged\n");
    d.write("untracked.md", "untracked\n");
    assert_eq!(d.changes().len(), 2);

    assert_eq!(std::fs::read(d.root().join(".git/index")).unwrap(), index);
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
    std::fs::write(root.join("a.md"), "a\n").unwrap();
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
    let config = std::fs::read_to_string(d.root().join(".git/config")).unwrap();
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
