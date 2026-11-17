//! Local repository operations: open or init, status, commit, remote config, sync state.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use git2::{
    Commit, Delta, DiffDelta, DiffFindOptions, ErrorCode, Oid, Repository, RepositoryInitOptions,
    Signature, Tree,
};
use serde::Serialize;

use crate::worktree::Snapshot;
use crate::{Error, Result};

pub(crate) const REMOTE: &str = "origin";
const INITIAL_BRANCH: &str = "main";
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Inside the git directory, so it is never part of the journal.
const LAST_FETCH_FILE: &str = "mdgitjournal-last-fetch";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
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
        let repo = &snapshot.repo;
        let head = match head_commit(repo)? {
            Some(commit) => commit.tree()?,
            None => empty_tree(repo)?,
        };
        let mut diff =
            repo.diff_tree_to_tree(Some(&head), Some(&repo.find_tree(snapshot.tree)?), None)?;
        diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
        Ok(diff.deltas().filter_map(file_change).collect())
    }

    /// Stages every change (additions, modifications, deletions) and commits it on the current branch.
    /// Returns the new commit id, or `None` if the working tree already matches HEAD.
    pub fn commit_all(&self, message: &str, author: &Author) -> Result<Option<String>> {
        if message.trim().is_empty() {
            return Err(Error::EmptyMessage);
        }
        let mut index = self.git.index()?;
        index.read(false)?;
        index.add_all(["*"], git2::IndexAddOption::DEFAULT, None)?;
        index.update_all(["*"], None)?;
        index.write()?;
        let tree = index.write_tree()?;
        let parent = head_commit(&self.git)?;
        let unchanged = match &parent {
            Some(parent) => parent.tree_id() == tree,
            None => index.is_empty(),
        };
        if unchanged {
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
        Ok(Some(id.to_string()))
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

    /// Removes remote "origin", its remote-tracking branches and upstream settings. Local files and
    /// commits stay.
    pub fn remove_remote(&self) -> Result<()> {
        match self.git.remote_delete(REMOTE) {
            Err(e) if e.code() != ErrorCode::NotFound => return Err(e.into()),
            _ => {}
        }
        match fs::remove_file(self.last_fetch_path()) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Branch, ahead/behind counts and changes, from local data only.
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
        })
    }

    /// The branch HEAD points at, which may be unborn. Fails if HEAD is detached.
    pub(crate) fn branch(&self) -> Result<String> {
        self.branch_name()?.ok_or_else(|| {
            Error::Git("HEAD is not on a branch; check out a branch with git first".into())
        })
    }

    fn branch_name(&self) -> Result<Option<String>> {
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

pub(crate) fn head_commit(repo: &Repository) -> Result<Option<Commit<'_>>> {
    match repo.head() {
        Ok(head) => Ok(Some(head.peel_to_commit()?)),
        Err(e) if matches!(e.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => Ok(None),
        Err(e) => Err(e.into()),
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

fn file_change(delta: DiffDelta<'_>) -> Option<FileChange> {
    let path = |file: git2::DiffFile<'_>| file.path_bytes().map(lossy);
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
    Some(FileChange {
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
