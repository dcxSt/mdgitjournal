//! Local repository operations: open or init, status, commit, remote config, sync state.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use git2::{
    Commit, Delta, DiffDelta, DiffFindOptions, ErrorCode, Oid, Repository, RepositoryInitOptions,
    RepositoryState, Signature, Tree,
};
use serde::{Deserialize, Serialize};

use crate::external::MergeState;
use crate::pull::SyncSummary;
use crate::worktree::{build_tree, entry, tree_files, Snapshot, GITLINK};
use crate::{Error, Result};

pub(crate) const REMOTE: &str = "origin";
const INITIAL_BRANCH: &str = "main";
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Inside the git directory, so it is never part of the journal.
const LAST_FETCH_FILE: &str = "mdgitjournal-last-fetch";
/// mdgitjournal's own records (Undo, Resolve with git), inside the git directory.
const STATE_DIR: &str = "mdgitjournal";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub has_remote: bool,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
    pub last_fetch: Option<i64>,
    /// A git merge is in progress (`MERGE_HEAD` exists).
    pub merge: Option<MergeState>,
    /// The pull Undo would reverse, while that is still possible.
    pub last_sync: Option<SyncSummary>,
}

/// A change between two trees, with exact paths.
pub(crate) struct RawChange {
    pub kind: ChangeKind,
    pub path: Vec<u8>,
    pub old_path: Option<Vec<u8>>,
}

impl RawChange {
    pub fn public(&self) -> FileChange {
        FileChange {
            path: lossy(&self.path),
            kind: self.kind,
            old_path: self.old_path.as_deref().map(lossy),
        }
    }
}

pub struct Repo {
    pub(crate) git: Repository,
}

impl Repo {
    /// Opens the repository at `path`, or creates one there (and the folder) with branch "main".
    pub fn open_or_init(path: &Path) -> Result<Repo> {
        let git = match Repository::open(path) {
            Ok(git) => git,
            Err(e) if e.code() == ErrorCode::NotFound => Repository::init_opts(
                path,
                RepositoryInitOptions::new().initial_head(INITIAL_BRANCH),
            )?,
            Err(e) => return Err(e.into()),
        };
        if git.is_bare() {
            return Err(Error::Git(format!(
                "{} is a bare repository, not a journal folder",
                path.display()
            )));
        }
        Ok(Repo { git })
    }

    /// Working tree vs HEAD, including untracked files that aren't ignored, with renames detected.
    pub fn status(&self) -> Result<Vec<FileChange>> {
        let snapshot = Snapshot::take(&self.git)?;
        let head = head_tree_id(&snapshot.repo)?;
        Ok(changes(&snapshot.repo, head, snapshot.tree)?
            .iter()
            .map(RawChange::public)
            .collect())
    }

    /// Commits exactly the changes in `paths` (as `status` names them; a rename is chosen by
    /// either name): HEAD's tree with those paths taken from the working tree. The rest stays
    /// uncommitted, and the index keeps every other entry as it was, so something staged with git
    /// stays staged. Requested paths that are no longer changed are ignored. Returns the new
    /// commit, or `None` if nothing was left to commit.
    pub fn commit(
        &self,
        message: &str,
        paths: &[String],
        author: &Author,
    ) -> Result<Option<String>> {
        self.commit_selected(message, Some(paths), author)
    }

    /// Commits every change in the working tree; `None` if it already matches HEAD.
    pub fn commit_all(&self, message: &str, author: &Author) -> Result<Option<String>> {
        self.commit_selected(message, None, author)
    }

    fn commit_selected(
        &self,
        message: &str,
        paths: Option<&[String]>,
        author: &Author,
    ) -> Result<Option<String>> {
        if message.trim().is_empty() {
            return Err(Error::EmptyMessage);
        }
        self.locked()?;
        let snapshot = Snapshot::take(&self.git)?;
        let id = self.commit_snapshot(&snapshot, message, paths, author)?;
        if id.is_some() {
            self.clear_undo()?;
        }
        Ok(id.map(|id| id.to_string()))
    }

    /// Commits the chosen changes between HEAD and `snapshot` (all of them for `None`).
    pub(crate) fn commit_snapshot(
        &self,
        snapshot: &Snapshot,
        message: &str,
        paths: Option<&[String]>,
        author: &Author,
    ) -> Result<Option<Oid>> {
        let parent = head_commit(&self.git)?;
        let head_tree = match &parent {
            Some(parent) => parent.tree_id(),
            None => Oid::from_str(EMPTY_TREE)?,
        };
        let chosen: Vec<RawChange> = changes(&snapshot.repo, head_tree, snapshot.tree)?
            .into_iter()
            .filter(|change| {
                paths.is_none_or(|paths| {
                    let change = change.public();
                    paths
                        .iter()
                        .any(|p| *p == change.path || Some(p) == change.old_path.as_ref())
                })
            })
            .collect();
        if chosen.is_empty() {
            return Ok(None);
        }

        let mut files = tree_files(&self.git.find_tree(head_tree)?)?;
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for change in &chosen {
            if let Some(old) = &change.old_path {
                removed.push(old.clone());
                files.remove(old);
            }
            match snapshot.files.get(&change.path) {
                Some(&entry) if change.kind != ChangeKind::Deleted => {
                    if entry.1 != GITLINK {
                        self.persist(snapshot, entry.0)?;
                    }
                    files.insert(change.path.clone(), entry);
                    added.push(change.path.clone());
                }
                _ => {
                    files.remove(&change.path);
                    removed.push(change.path.clone());
                }
            }
        }
        let tree = build_tree(&self.git, &files)?;
        if tree == head_tree {
            return Ok(None);
        }

        let signature = signature(author)?;
        let message = git2::message_prettify(message, None)?;
        let parents: Vec<&Commit> = parent.iter().collect();
        let id = self.git.commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &self.git.find_tree(tree)?,
            &parents,
        )?;

        // The index follows the commit for the chosen paths only. A file is re-read for its stat
        // data, which is only kept if it still holds what was committed.
        let mut index = self.git.index()?;
        index.read(false)?;
        for path in &removed {
            if !files.contains_key(path) {
                ignore_missing(index.remove_path(Path::new(OsStr::from_bytes(path))))?;
            }
        }
        for path in &added {
            let (blob, mode) = files[path];
            let relative = Path::new(OsStr::from_bytes(path));
            let fresh = mode != GITLINK
                && index.add_path(relative).is_ok()
                && index
                    .get_path(relative, 0)
                    .is_some_and(|e| e.id == blob && e.mode == mode);
            if !fresh {
                index.add(&entry(path, blob, mode))?;
            }
        }
        index.write()?;
        Ok(Some(id))
    }

    pub fn remote_url(&self) -> Option<String> {
        self.git
            .find_remote(REMOTE)
            .ok()?
            .url()
            .ok()
            .map(str::to_owned)
    }

    /// Points remote "origin" at `url`, with any user name or password removed from it.
    pub fn set_remote(&self, url: &str) -> Result<()> {
        let url = without_userinfo(url);
        match self.git.find_remote(REMOTE) {
            Ok(_) => self.git.remote_set_url(REMOTE, &url)?,
            Err(e) if e.code() == ErrorCode::NotFound => {
                self.git.remote(REMOTE, &url)?;
            }
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// Removes remote "origin", its remote-tracking branches and upstream settings, and the undo
    /// record. Local files and commits stay.
    pub fn remove_remote(&self) -> Result<()> {
        match self.git.remote_delete(REMOTE) {
            Err(e) if e.code() != ErrorCode::NotFound => return Err(e.into()),
            _ => {}
        }
        remove_if_exists(&self.last_fetch_path())?;
        self.clear_undo()
    }

    /// Branch, ahead/behind counts, changes, merge and undo state, from local data only. Never
    /// writes.
    pub fn sync_state(&self) -> Result<SyncState> {
        let has_remote = self.git.find_remote(REMOTE).is_ok();
        let branch = self.branch_name()?;
        let (ahead, behind) = match &branch {
            Some(branch) if has_remote => self.ahead_behind(branch)?,
            _ => (0, 0),
        };
        Ok(SyncState {
            has_remote,
            branch: branch.unwrap_or_default(),
            ahead,
            behind,
            changes: self.status()?,
            last_fetch: if has_remote { self.last_fetch() } else { None },
            merge: self.merge_state()?,
            last_sync: self.last_sync()?,
        })
    }

    /// Fails while git is in the middle of something (a merge: `MergeInProgress`). A record of a
    /// merge the user has since finished or aborted with git is deleted.
    pub(crate) fn locked(&self) -> Result<()> {
        let operation = match self.git.state() {
            RepositoryState::Clean => return self.forget_merge(),
            RepositoryState::Merge => return Err(Error::MergeInProgress),
            RepositoryState::Revert | RepositoryState::RevertSequence => "revert",
            RepositoryState::CherryPick | RepositoryState::CherryPickSequence => "cherry-pick",
            RepositoryState::Bisect => "bisect",
            RepositoryState::ApplyMailbox | RepositoryState::ApplyMailboxOrRebase => "am",
            _ => "rebase",
        };
        Err(Error::Git(format!(
            "a git {operation} is in progress; finish it with git first"
        )))
    }

    /// The branch HEAD points at, which may be unborn. Fails if HEAD is detached.
    pub(crate) fn branch(&self) -> Result<String> {
        self.branch_name()?.ok_or_else(|| {
            Error::Git("HEAD is not on a branch; check out a branch with git first".into())
        })
    }

    pub(crate) fn branch_name(&self) -> Result<Option<String>> {
        let head = self.git.find_reference("HEAD")?;
        Ok(head
            .symbolic_target()?
            .and_then(|target| target.strip_prefix("refs/heads/"))
            .map(str::to_owned))
    }

    pub(crate) fn remote_branch(&self, branch: &str) -> Result<Option<Oid>> {
        match self
            .git
            .refname_to_id(&format!("refs/remotes/{REMOTE}/{branch}"))
        {
            Ok(id) => Ok(Some(id)),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Counts HEAD vs origin/<branch>. A side that doesn't exist yet (an unborn HEAD, or a branch
    /// the remote doesn't have) counts as having none of the other side's commits.
    pub(crate) fn ahead_behind(&self, branch: &str) -> Result<(u32, u32)> {
        let head = head_commit(&self.git)?.map(|commit| commit.id());
        let (ahead, behind) = match (head, self.remote_branch(branch)?) {
            (Some(head), Some(upstream)) => self.git.graph_ahead_behind(head, upstream)?,
            (Some(head), None) => (self.count_commits(head)?, 0),
            (None, Some(upstream)) => (0, self.count_commits(upstream)?),
            (None, None) => (0, 0),
        };
        let saturate = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Ok((saturate(ahead), saturate(behind)))
    }

    fn count_commits(&self, tip: Oid) -> Result<usize> {
        let mut walk = self.git.revwalk()?;
        walk.push(tip)?;
        Ok(walk.count())
    }

    /// Records `branch.<b>.remote` and `branch.<b>.merge`, so plain git sees the upstream too.
    pub(crate) fn set_upstream(&self, branch: &str) -> Result<()> {
        let mut config = self.git.config()?;
        config.set_str(&format!("branch.{branch}.remote"), REMOTE)?;
        config.set_str(
            &format!("branch.{branch}.merge"),
            &format!("refs/heads/{branch}"),
        )?;
        Ok(())
    }

    /// Copies a blob the snapshot found (an uncommitted file, a merged text) into the repository's
    /// object store, for a commit to use.
    pub(crate) fn persist(&self, snapshot: &Snapshot, id: Oid) -> Result<()> {
        if !self.git.odb()?.exists(id) {
            let blob = snapshot.repo.find_blob(id)?;
            self.git.blob(blob.content())?;
        }
        Ok(())
    }

    /// `.git/mdgitjournal/<name>`.
    pub(crate) fn state_file(&self, name: &str) -> PathBuf {
        self.git.path().join(STATE_DIR).join(name)
    }

    pub(crate) fn read_state_file(&self, name: &str) -> Result<Option<Vec<u8>>> {
        read_if_exists(&self.state_file(name))
    }

    /// Writes `.git/mdgitjournal/<name>` atomically.
    pub(crate) fn write_state_file(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let path = self.state_file(name);
        fs::create_dir_all(path.parent().expect("a state file has a folder"))?;
        let temp = self.state_file(&format!(".{name}.tmp"));
        remove_if_exists(&temp)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &path)?;
        Ok(())
    }

    pub(crate) fn remove_state_file(&self, name: &str) -> Result<()> {
        remove_if_exists(&self.state_file(name))
    }

    /// The bytes of `.git/index`, if there is one.
    pub(crate) fn index_bytes(&self) -> Result<Option<Vec<u8>>> {
        read_if_exists(&self.git.path().join("index"))
    }

    /// Replaces `.git/index` with `bytes` (removes it for `None`), taking git's `index.lock` so a
    /// git command running at the same time is never overwritten.
    pub(crate) fn replace_index(&self, bytes: Option<&[u8]>) -> Result<()> {
        let path = self.git.path().join("index");
        let lock = self.git.path().join("index.lock");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|e| match e.kind() {
                io::ErrorKind::AlreadyExists => Error::Git(
                    "the git index is locked (.git/index.lock exists); if no git command is \
                     running, remove that file and try again"
                        .into(),
                ),
                _ => e.into(),
            })?;
        let written = match bytes {
            Some(bytes) => file
                .write_all(bytes)
                .and_then(|()| file.sync_all())
                .and_then(|()| fs::rename(&lock, &path)),
            None => match fs::remove_file(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => fs::remove_file(&lock),
            },
        };
        if written.is_err() {
            let _ = fs::remove_file(&lock);
        }
        written?;
        // libgit2 caches the index; make it read the new one.
        self.git.index()?.read(true)?;
        Ok(())
    }

    fn last_fetch_path(&self) -> PathBuf {
        self.git.path().join(LAST_FETCH_FILE)
    }

    fn last_fetch(&self) -> Option<i64> {
        fs::read_to_string(self.last_fetch_path())
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    pub(crate) fn record_fetch(&self) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        fs::write(self.last_fetch_path(), format!("{now}\n"))?;
        Ok(())
    }
}

/// The changes from tree `old` to tree `new`, with renames detected.
pub(crate) fn changes(repo: &Repository, old: Oid, new: Oid) -> Result<Vec<RawChange>> {
    let mut diff = repo.diff_tree_to_tree(
        Some(&repo.find_tree(old)?),
        Some(&repo.find_tree(new)?),
        None,
    )?;
    diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
    Ok(diff.deltas().filter_map(raw_change).collect())
}

pub(crate) fn head_commit(repo: &Repository) -> Result<Option<Commit<'_>>> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_commit()?)),
        Err(e) if matches!(e.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// HEAD's tree, or the empty tree on an unborn branch.
pub(crate) fn head_tree_id(repo: &Repository) -> Result<Oid> {
    match head_commit(repo)? {
        Some(commit) => Ok(commit.tree_id()),
        None => Ok(Oid::from_str(EMPTY_TREE)?),
    }
}

pub(crate) fn empty_tree(repo: &Repository) -> Result<Tree<'_>> {
    // libgit2 knows the empty tree without it being stored, so this never writes an object.
    Ok(repo.find_tree(Oid::from_str(EMPTY_TREE)?)?)
}

pub(crate) fn workdir(repo: &Repository) -> Result<&Path> {
    repo.workdir()
        .ok_or_else(|| Error::Git("the repository has no working tree".into()))
}

pub(crate) fn signature(author: &Author) -> Result<Signature<'static>> {
    Signature::now(&author.name, &author.email).map_err(|e| {
        Error::Git(format!(
            "invalid commit author \"{} <{}>\": {}",
            author.name,
            author.email,
            e.message()
        ))
    })
}

pub(crate) fn lossy(path: &[u8]) -> String {
    String::from_utf8_lossy(path).into_owned()
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn read_if_exists(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub(crate) fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn ignore_missing(result: std::result::Result<(), git2::Error>) -> Result<()> {
    match result {
        Err(e) if e.code() != ErrorCode::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

fn raw_change(delta: DiffDelta<'_>) -> Option<RawChange> {
    let path = |file: git2::DiffFile<'_>| file.path_bytes().map(<[u8]>::to_vec);
    let (kind, path, old_path) = match delta.status() {
        Delta::Added | Delta::Untracked | Delta::Copied => {
            (ChangeKind::Added, path(delta.new_file()), None)
        }
        Delta::Modified | Delta::Typechange => (ChangeKind::Modified, path(delta.new_file()), None),
        Delta::Deleted => (ChangeKind::Deleted, path(delta.old_file()), None),
        Delta::Renamed => (
            ChangeKind::Renamed,
            path(delta.new_file()),
            path(delta.old_file()),
        ),
        _ => return None,
    };
    Some(RawChange {
        path: path?,
        kind,
        old_path,
    })
}

/// "https://user:secret@host/x.git" -> "https://host/x.git", so credentials never reach .git/config.
pub(crate) fn without_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::without_userinfo;

    #[test]
    fn userinfo_is_removed_from_urls() {
        let cases = [
            (
                "https://x-access-token:ghs_secret@github.com/o/r.git",
                "https://github.com/o/r.git",
            ),
            (
                "https://user@github.com/o/r.git",
                "https://github.com/o/r.git",
            ),
            ("https://github.com/o/r.git", "https://github.com/o/r.git"),
            (
                "https://github.com/o/r@x.git",
                "https://github.com/o/r@x.git",
            ),
            ("file:///tmp/remote.git", "file:///tmp/remote.git"),
            ("/tmp/remote.git", "/tmp/remote.git"),
        ];
        for (url, expected) in cases {
            assert_eq!(without_userinfo(url), expected, "{url}");
        }
    }
}
