//! Test devices: journals in temp dirs, synced through a bare repository in another temp dir.
// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use git2::{Oid, PushOptions, Repository, RepositoryInitOptions, Sort};
use mdgj_git::{
    Author, ChangeKind, CommitPushOutcome, MergeChunk, Overlap, PullOutcome, PullPreview, Repo,
    Resolution, Resolved, SyncSummary,
};
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

    /// Commits everything and pushes it, expecting the push to go through.
    pub fn push(&self, message: &str) {
        let paths: Vec<String> = self
            .repo
            .status()
            .unwrap()
            .into_iter()
            .map(|c| c.path)
            .collect();
        match self
            .repo
            .commit_and_push(message, &paths, &author(), None)
            .unwrap()
        {
            CommitPushOutcome::Pushed { .. } => {}
            other => panic!("expected a push: {other:?}"),
        }
    }

    /// Pushes the branch with libgit2 directly, as the git command line would, bypassing the app.
    pub fn push_with_git(&self) {
        let git = self.git();
        let mut remote = git.find_remote("origin").unwrap();
        let branch = git.head().unwrap().shorthand().unwrap().to_owned();
        remote
            .push(
                &[format!("refs/heads/{branch}:refs/heads/{branch}")],
                Some(&mut PushOptions::new()),
            )
            .unwrap();
        // As `git push` does, the remote-tracking branch follows.
        let head = git.head().unwrap().target().unwrap();
        git.reference(&format!("refs/remotes/origin/{branch}"), head, true, "push")
            .unwrap();
    }

    pub fn pull(&self) -> PullOutcome {
        self.repo.pull(None, &author()).unwrap()
    }

    /// Pulls, expecting it to apply at once.
    pub fn pull_applied(&self) -> SyncSummary {
        match self.pull() {
            PullOutcome::Applied { summary, .. } => summary,
            other => panic!("expected the pull to apply: {other:?}"),
        }
    }

    /// Pulls, expecting a review.
    pub fn pull_review(&self) -> PullPreview {
        match self.pull() {
            PullOutcome::Review { preview, .. } => preview,
            other => panic!("expected a review: {other:?}"),
        }
    }

    pub fn apply(&self, preview: &PullPreview, resolutions: &[Resolved]) -> SyncSummary {
        self.repo
            .apply_pull(
                &preview.id,
                resolutions,
                Some("Merge from GitHub"),
                &author(),
            )
            .unwrap()
    }

    pub fn git(&self) -> Repository {
        Repository::open(self.root()).unwrap()
    }

    pub fn head(&self) -> Oid {
        self.git().head().unwrap().target().unwrap()
    }

    /// Every file in the working tree, sorted, without .git.
    pub fn files(&self) -> Vec<String> {
        self.file_bytes().into_keys().collect()
    }

    /// Every file in the working tree with its bytes (a link as "-> target").
    pub fn file_bytes(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().unwrap() == ".git" {
                    continue;
                }
                let meta = path.symlink_metadata().unwrap();
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                if meta.file_type().is_symlink() {
                    let target = fs::read_link(&path).unwrap();
                    out.insert(relative, format!("-> {}", target.display()).into_bytes());
                } else if meta.is_dir() {
                    walk(&path, root, out);
                } else {
                    out.insert(relative, fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(self.root(), self.root(), &mut out);
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

    /// Everything a pull could change on this device: local branches and HEAD, the index file's
    /// bytes, every working-tree file's bytes, and mdgitjournal's own records.
    pub fn state(&self) -> State {
        let git = self.git();
        let mut refs = BTreeMap::new();
        for reference in git.references().unwrap() {
            let reference = reference.unwrap();
            let name = reference.name().unwrap().to_owned();
            if !name.starts_with("refs/remotes/") {
                refs.insert(name, reference.target().map(|id| id.to_string()));
            }
        }
        let head = fs::read_to_string(git.path().join("HEAD")).unwrap();
        let read = |path: PathBuf| fs::read(path).ok();
        State {
            head,
            refs,
            index: read(git.path().join("index")),
            files: self.file_bytes(),
            records: ["undo.json", "undo-index", "merge.json", "merge-index"]
                .iter()
                .map(|name| read(git.path().join("mdgitjournal").join(name)))
                .collect(),
            merge_files: ["MERGE_HEAD", "MERGE_MSG", "MERGE_MODE"]
                .iter()
                .map(|name| git.path().join(name).exists())
                .collect(),
        }
    }

    /// Every object file in the object store (loose objects and packs).
    pub fn objects(&self) -> Vec<String> {
        fn walk(dir: &Path, out: &mut Vec<String>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path.display().to_string());
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root().join(".git/objects"), &mut out);
        out.sort();
        out
    }

    /// Runs the git command line in the journal, if git is installed; `None` otherwise.
    pub fn cli(&self, args: &[&str]) -> Option<Output> {
        let output = Command::new("git")
            .arg("-C")
            .arg(self.root())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "CLI User")
            .env("GIT_AUTHOR_EMAIL", "cli@example.com")
            .env("GIT_COMMITTER_NAME", "CLI User")
            .env("GIT_COMMITTER_EMAIL", "cli@example.com")
            .env("GIT_EDITOR", "true")
            .output()
            .ok()?;
        Some(output)
    }

    /// The git command line's stdout, if git is installed; panics if the command fails.
    pub fn cli_ok(&self, args: &[&str]) -> Option<String> {
        let output = self.cli(args)?;
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(String::from_utf8(output.stdout).unwrap())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub head: String,
    pub refs: BTreeMap<String, Option<String>>,
    pub index: Option<Vec<u8>>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub records: Vec<Option<Vec<u8>>>,
    pub merge_files: Vec<bool>,
}

impl State {
    /// The state without the undo record, which Resolve with git and commits end by design.
    pub fn without_undo(mut self) -> State {
        self.refs.remove("refs/mdgitjournal/undo");
        self.records[0] = None;
        self.records[1] = None;
        self
    }
}

pub fn resolved(path: &str, resolution: Resolution) -> Resolved {
    Resolved {
        path: path.into(),
        resolution,
    }
}

pub fn merged(path: &str, text: impl Into<String>) -> Resolved {
    resolved(path, Resolution::Merged { text: text.into() })
}

pub fn overlap<'a>(preview: &'a PullPreview, path: &str) -> &'a Overlap {
    preview
        .overlaps
        .iter()
        .find(|overlap| overlap.path == path)
        .unwrap_or_else(|| panic!("no overlap at {path}: {:?}", preview.overlaps))
}

/// A remote holding `files` in one commit, and two devices connected to it.
pub fn two_devices(files: &[(&str, &str)]) -> (Remote, Device, Device) {
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

/// Whether `copy` is `<stem>.from-github-<hh>h<mm><ext>`.
pub fn is_github_copy(copy: &str, stem: &str, ext: &str) -> bool {
    copy.strip_prefix(stem)
        .and_then(|rest| rest.strip_prefix(".from-github-"))
        .and_then(|rest| rest.strip_suffix(ext))
        .is_some_and(|stamp| {
            let b = stamp.as_bytes();
            b.len() == 5 && b[2] == b'h' && [0, 1, 3, 4].iter().all(|&i| b[i].is_ascii_digit())
        })
}

/// The base, mine and theirs texts an overlap's suggestion was made from.
pub fn sides(overlap: &Overlap) -> (String, String, String) {
    let (mut b, mut m, mut t) = (String::new(), String::new(), String::new());
    for chunk in &overlap.suggestion.as_ref().expect("a text overlap").chunks {
        let (base, mine, theirs) = match chunk {
            MergeChunk::Same { text } => (text, text, text),
            MergeChunk::Mine { text, base } => (base, text, base),
            MergeChunk::Theirs { text, base } => (base, base, text),
            MergeChunk::Both { text, base } => (base, text, text),
            MergeChunk::Clash { base, mine, theirs } => (base, mine, theirs),
        };
        b.push_str(base);
        m.push_str(mine);
        t.push_str(theirs);
    }
    (b, m, t)
}

pub fn no_conflict_markers(d: &Device) {
    for (file, content) in d.file_bytes() {
        assert!(
            !content
                .windows(7)
                .any(|w| w == b"<<<<<<<" || w == b">>>>>>>"),
            "conflict markers in {file}"
        );
    }
}
