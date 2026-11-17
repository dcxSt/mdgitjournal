//! Test devices: journals in temp dirs, synced through a bare repository in another temp dir.
// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use git2::{Oid, Repository, RepositoryInitOptions, Sort};
use mdgj_git::{Author, ChangeKind, Repo};
use tempfile::TempDir;

pub fn author() -> Author {
    Author {
        name: "Test Author".into(),
        email: "test@example.com".into(),
    }
}

/// A bare repository reached through a file:// URL, like GitHub but local.
pub struct Remote {
    _dir: TempDir,
    pub path: PathBuf,
    pub url: String,
}

impl Remote {
    pub fn new() -> Remote {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.git");
        Repository::init_opts(
            &path,
            RepositoryInitOptions::new().bare(true).initial_head("main"),
        )
        .unwrap();
        Remote {
            url: format!("file://{}", path.display()),
            path,
            _dir: dir,
        }
    }

    pub fn head(&self, branch: &str) -> Option<Oid> {
        let repo = Repository::open_bare(&self.path).unwrap();
        repo.refname_to_id(&format!("refs/heads/{branch}")).ok()
    }
}

pub struct Device {
    dir: TempDir,
    pub repo: Repo,
}

impl Device {
    pub fn new() -> Device {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repo::open_or_init(dir.path()).unwrap();
        Device { dir, repo }
    }

    /// A device whose journal is a git repository created outside the app, on `branch`.
    pub fn with_branch(branch: &str) -> Device {
        let dir = tempfile::tempdir().unwrap();
        Repository::init_opts(
            dir.path(),
            RepositoryInitOptions::new().initial_head(branch),
        )
        .unwrap();
        let repo = Repo::open_or_init(dir.path()).unwrap();
        Device { dir, repo }
    }

    pub fn connected(remote: &Remote) -> Device {
        let device = Device::new();
        device.repo.connect(&remote.url, None, &author()).unwrap();
        device
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn write(&self, path: &str, content: impl AsRef<[u8]>) {
        let full = self.root().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }

    pub fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.root().join(path)).unwrap()
    }

    pub fn text(&self, path: &str) -> String {
        String::from_utf8(self.read(path)).unwrap()
    }

    pub fn exists(&self, path: &str) -> bool {
        self.root().join(path).exists()
    }

    pub fn remove(&self, path: &str) {
        fs::remove_file(self.root().join(path)).unwrap();
    }

    pub fn commit(&self, message: &str) -> Oid {
        let id = self.repo.commit_all(message, &author()).unwrap().unwrap();
        Oid::from_str(&id).unwrap()
    }

    pub fn git(&self) -> Repository {
        Repository::open(self.root()).unwrap()
    }

    pub fn head(&self) -> Oid {
        self.git().head().unwrap().target().unwrap()
    }

    /// Every file in the working tree, sorted, without .git.
    pub fn files(&self) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().unwrap() == ".git" {
                    continue;
                }
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    let relative = path.strip_prefix(root).unwrap();
                    out.push(relative.to_str().unwrap().to_owned());
                }
            }
        }
        let mut out = Vec::new();
        walk(self.root(), self.root(), &mut out);
        out.sort();
        out
    }

    pub fn changes(&self) -> Vec<(String, ChangeKind)> {
        let mut changes: Vec<_> = self
            .repo
            .status()
            .unwrap()
            .into_iter()
            .map(|change| (change.path, change.kind))
            .collect();
        changes.sort_by(|a, b| a.0.cmp(&b.0));
        changes
    }

    /// (message, parent count) for every commit reachable from HEAD, newest first.
    pub fn history(&self) -> Vec<(String, usize)> {
        let git = self.git();
        let mut walk = git.revwalk().unwrap();
        walk.set_sorting(Sort::TOPOLOGICAL).unwrap();
        walk.push_head().unwrap();
        walk.map(|id| {
            let commit = git.find_commit(id.unwrap()).unwrap();
            let message = commit.message().unwrap().trim_end().to_owned();
            (message, commit.parent_count())
        })
        .collect()
    }

    /// The content of `path` in the HEAD commit.
    pub fn committed(&self, path: &str) -> Option<Vec<u8>> {
        let git = self.git();
        let tree = git.head().unwrap().peel_to_tree().unwrap();
        let entry = tree.get_path(Path::new(path)).ok()?;
        let blob = git.find_blob(entry.id()).unwrap();
        Some(blob.content().to_vec())
    }
}

/// Whether `copy` is `<stem>.conflict-<hhmm><ext>`.
pub fn is_conflict_copy(copy: &str, stem: &str, ext: &str) -> bool {
    copy.strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix(".conflict-"))
        .and_then(|rest| rest.strip_suffix(ext))
        .is_some_and(|hhmm| hhmm.len() == 4 && hhmm.bytes().all(|b| b.is_ascii_digit()))
}
