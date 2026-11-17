//! Remote operations: fetch, pull, push, commit and push, connect.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;

use git2::{
    AutotagOption, BranchType, Commit, Cred, CredentialType, Delta, DiffFindOptions, ErrorCode,
    FetchOptions, FetchPrune, ProxyOptions, PushOptions, Remote, RemoteCallbacks, Repository, Sort,
    Tree,
};
use serde::Serialize;

use crate::merge::{merge_keep_both, tree_files};
use crate::repo::{empty_tree, head_commit, lossy, signature, without_userinfo, workdir, REMOTE};
use crate::worktree::{apply, exists, files, Snapshot};
use crate::{Author, Error, Repo, Result, SyncState};

const FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";
const MAX_AUTH_ATTEMPTS: u32 = 3;
/// Branch names tried, in order, when the remote doesn't say which branch is its default.
const DEFAULT_BRANCHES: [&str; 2] = ["main", "master"];

/// HTTPS credentials, held in memory only.
#[derive(Clone)]
pub struct Creds {
    pub username: String,
    pub password: String,
    /// The credentials are only sent to URLs starting with this, e.g. "https://github.com/". git
    /// may contact another URL than origin's (`pushurl`, `insteadOf`, redirects).
    pub url_prefix: String,
}

impl fmt::Debug for Creds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Creds")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("url_prefix", &self.url_prefix)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub path: String,
    /// Where the local version was saved; `None` if it had been deleted locally.
    pub copy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullOutcome {
    pub state: SyncState,
    /// Paths the pull wrote, moved or removed in the working tree.
    pub updated: Vec<String>,
    /// Paths where local and remote changes were merged cleanly.
    pub merged: Vec<String>,
    pub conflicts: Vec<Conflict>,
    /// Local commits replayed onto the remote branch.
    pub rebased: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushOutcome {
    Pushed {
        state: SyncState,
    },
    NothingToPush {
        state: SyncState,
    },
    /// The remote branch moved on; pull first.
    Rejected {
        state: SyncState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitPushOutcome {
    Pushed {
        state: SyncState,
    },
    /// Committed locally, but the remote has commits that must be pulled first.
    NeedsPull {
        state: SyncState,
    },
    NothingToCommit {
        state: SyncState,
    },
}

/// The remote's branches, as listed during the last fetch.
struct RemoteBranches {
    default: Option<String>,
    names: Vec<String>,
}

impl Repo {
    /// Updates `refs/remotes/origin/*` and returns the new sync state.
    pub fn fetch(&self, creds: Option<&Creds>) -> Result<SyncState> {
        self.fetch_origin(creds)?;
        self.sync_state()
    }

    /// Fetches, then brings the remote's commits into the branch and working tree. Unpushed local
    /// commits are rebased onto the remote; uncommitted edits stay uncommitted. Conflicts keep both
    /// versions, so no text is ever lost.
    pub fn pull(&self, creds: Option<&Creds>, author: &Author) -> Result<PullOutcome> {
        self.fetch_origin(creds)?;
        self.pull_fetched(author)
    }

    /// Pushes the current branch. Never forces.
    pub fn push(&self, creds: Option<&Creds>) -> Result<PushOutcome> {
        let mut remote = self.origin()?;
        let branch = self.branch()?;
        if self.ahead_behind(&branch)?.0 == 0 {
            return Ok(PushOutcome::NothingToPush {
                state: self.sync_state()?,
            });
        }
        let rejection = RefCell::new(None);
        let auth = Auth::new(creds);
        let mut callbacks = auth.callbacks();
        callbacks.push_update_reference(|_, status| {
            if let Some(status) = status {
                rejection.replace(Some(status.to_owned()));
            }
            Ok(())
        });
        let mut options = PushOptions::new();
        options.remote_callbacks(callbacks).proxy_options(proxy());
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        match remote.push(&[refspec.as_str()], Some(&mut options)) {
            Ok(()) => {}
            Err(e) if auth.failure.borrow().is_none() && e.code() == ErrorCode::NotFastForward => {
                rejection.replace(Some(e.message().to_owned()));
            }
            Err(e) => return Err(auth.error(e)),
        }
        if let Some(reason) = rejection.take() {
            // Tells "the remote moved on" (pull, then push again) from a refusal pulling can't fix.
            self.fetch_origin(creds)?;
            let state = self.sync_state()?;
            return if state.behind > 0 {
                Ok(PushOutcome::Rejected { state })
            } else {
                Err(Error::Git(format!("the remote refused the push: {reason}")))
            };
        }
        self.set_upstream(&branch)?;
        Ok(PushOutcome::Pushed {
            state: self.sync_state()?,
        })
    }

    /// Commits everything, then pushes unless the remote has commits this device hasn't pulled.
    pub fn commit_and_push(
        &self,
        message: &str,
        author: &Author,
        creds: Option<&Creds>,
    ) -> Result<CommitPushOutcome> {
        self.origin()?;
        let committed = self.commit_all(message, author)?.is_some();
        if !committed && self.ahead_behind(&self.branch()?)?.0 == 0 {
            return Ok(CommitPushOutcome::NothingToCommit {
                state: self.sync_state()?,
            });
        }
        self.fetch_origin(creds)?;
        let state = self.sync_state()?;
        if state.behind > 0 {
            return Ok(CommitPushOutcome::NeedsPull { state });
        }
        Ok(match self.push(creds)? {
            PushOutcome::Pushed { state } => CommitPushOutcome::Pushed { state },
            PushOutcome::Rejected { state } => CommitPushOutcome::NeedsPull { state },
            PushOutcome::NothingToPush { state } => CommitPushOutcome::NothingToCommit { state },
        })
    }

    /// Connects to the repository at `url`. An empty remote receives the local history; otherwise the
    /// local branch takes the remote's default branch name and pulls it, even if the histories are
    /// unrelated. If connecting fails, the previous remote setting is restored, so a failed connect
    /// never looks connected.
    pub fn connect(
        &self,
        url: &str,
        creds: Option<&Creds>,
        author: &Author,
    ) -> Result<PullOutcome> {
        let previous = self.remote_url();
        self.set_remote(url)?;
        let outcome = self.join_remote(creds, author);
        if outcome.is_err() {
            match previous {
                Some(url) => self.set_remote(&url)?,
                None => self.remove_remote()?,
            }
        }
        outcome
    }

    fn join_remote(&self, creds: Option<&Creds>, author: &Author) -> Result<PullOutcome> {
        let remote = self.fetch_origin(creds)?;
        let local = self.branch()?;
        let shared = remote
            .default
            .iter()
            .map(String::as_str)
            .chain(DEFAULT_BRANCHES)
            .chain([local.as_str()])
            .find(|name| remote.names.iter().any(|n| n == name))
            .map(str::to_owned);
        match shared {
            Some(branch) => {
                self.adopt_branch(&local, &branch)?;
                self.set_upstream(&branch)?;
                self.pull_fetched(author)
            }
            None => {
                if head_commit(&self.git)?.is_some() {
                    self.push(creds)?;
                }
                self.unchanged()
            }
        }
    }

    fn origin(&self) -> Result<Remote<'_>> {
        self.git.find_remote(REMOTE).map_err(|e| match e.code() {
            ErrorCode::NotFound => Error::NotConnected,
            _ => e.into(),
        })
    }

    fn fetch_origin(&self, creds: Option<&Creds>) -> Result<RemoteBranches> {
        let mut remote = self.origin()?;
        let auth = Auth::new(creds);
        let mut options = FetchOptions::new();
        options
            .remote_callbacks(auth.callbacks())
            .proxy_options(proxy())
            .prune(FetchPrune::On)
            .download_tags(AutotagOption::None);
        remote
            .fetch(
                &[FETCH_REFSPEC],
                Some(&mut options),
                Some("mdgitjournal: fetch"),
            )
            .map_err(|e| auth.error(e))?;
        self.record_fetch()?;
        // libgit2 keeps the advertised refs after the fetch disconnects.
        let branch = |name: &str| name.strip_prefix("refs/heads/").map(str::to_owned);
        let names = remote
            .list()?
            .iter()
            .filter_map(|h| branch(h.name()))
            .collect();
        let default = remote
            .default_branch()
            .ok()
            .and_then(|name| name.as_str().ok().and_then(branch));
        Ok(RemoteBranches { default, names })
    }

    fn pull_fetched(&self, author: &Author) -> Result<PullOutcome> {
        let branch = self.branch()?;
        let Some(remote) = self.remote_branch(&branch)? else {
            return self.unchanged();
        };
        let remote = self.git.find_commit(remote)?;
        let local = head_commit(&self.git)?;
        let stamp = chrono::Local::now().format("%H%M").to_string();
        let mut conflicts = Vec::new();
        let mut rebased = 0;
        let new_head = match &local {
            None => remote,
            Some(local)
                if local.id() == remote.id()
                    || self.git.graph_descendant_of(local.id(), remote.id())? =>
            {
                return self.unchanged();
            }
            Some(local) if self.git.graph_descendant_of(remote.id(), local.id())? => remote,
            Some(local) => {
                let (head, count) =
                    self.rebase(local, &remote, &branch, author, &stamp, &mut conflicts)?;
                rebased = count;
                head
            }
        };

        // Uncommitted work: merge the working tree onto the new head, with the old head as base.
        let snapshot = Snapshot::take(&self.git)?;
        let repo = &snapshot.repo;
        let base = match &local {
            Some(local) => repo.find_tree(local.tree_id())?,
            None => empty_tree(repo)?,
        };
        let ours = repo.find_tree(snapshot.tree)?;
        let theirs = repo.find_tree(new_head.tree_id())?;
        let workdir = workdir(repo)?;
        let merge = merge_keep_both(repo, &base, &ours, &theirs, &stamp, |p| exists(workdir, p))?;
        conflicts.extend(merge.conflicts);
        let mut target = files(&merge.index);

        // A safety net under the merge: a file may leave the working tree only if git still has
        // its exact content, at the base or somewhere in the result. A file kept this way is
        // reported, so it never sits there unnoticed (e.g. renamed remotely, edited here).
        let base_files = tree_files(&base)?;
        let renamed = renames(repo, &base, &theirs)?;
        for (path, entry) in &snapshot.files {
            let kept = target.contains_key(path)
                || base_files.get(path) == Some(entry)
                || target.values().any(|(id, _)| *id == entry.0);
            if !kept {
                target.insert(path.clone(), *entry);
                let shown = lossy(renamed.get(path).unwrap_or(path));
                // Replaces the merge's "deleted here" note for the renamed file: it wasn't.
                conflicts.retain(|c| c.path != shown || c.copy.is_some());
                conflicts.push(Conflict {
                    path: shown,
                    copy: Some(lossy(path)),
                });
            }
        }

        let theirs_files = tree_files(&theirs)?;
        let merged = target
            .iter()
            .filter(|&(path, entry)| {
                let ours = snapshot.files.get(path);
                let theirs = theirs_files.get(path);
                ours.is_some_and(|o| o != entry) && theirs.is_some_and(|t| t != entry)
            })
            .map(|(path, _)| lossy(path))
            .collect();

        // Files first: if this fails partway, HEAD still names the old commit, and the next pull
        // merges again from there.
        let updated = apply(&snapshot, &target, &stamp, &mut conflicts)?;
        self.git.reference(
            &format!("refs/heads/{branch}"),
            new_head.id(),
            true,
            "mdgitjournal: pull",
        )?;
        let mut index = self.git.index()?;
        index.read(false)?;
        index.read_tree(&new_head.tree()?)?;
        index.write()?;
        Ok(PullOutcome {
            state: self.sync_state()?,
            updated,
            merged,
            conflicts,
            rebased,
        })
    }

    /// Replays the local commits missing from `remote` on top of it, oldest first, keeping their
    /// messages and authors; commits whose changes are already upstream are dropped. If the local
    /// side has merge commits, one merge commit is made instead. Returns the new head and the number
    /// of commits replayed.
    fn rebase<'r>(
        &'r self,
        local: &Commit<'r>,
        remote: &Commit<'r>,
        branch: &str,
        author: &Author,
        stamp: &str,
        conflicts: &mut Vec<Conflict>,
    ) -> Result<(Commit<'r>, u32)> {
        let repo = &self.git;
        let committer = signature(author)?;
        let mut walk = repo.revwalk()?;
        walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;
        walk.push(local.id())?;
        walk.hide(remote.id())?;
        let commits = walk
            .map(|id| repo.find_commit(id?))
            .collect::<Result<Vec<_>, _>>()?;
        let untaken = |_: &[u8]| false;

        if commits.iter().any(|commit| commit.parent_count() > 1) {
            let base = match repo.merge_base(local.id(), remote.id()) {
                Ok(id) => repo.find_commit(id)?.tree()?,
                Err(e) if e.code() == ErrorCode::NotFound => empty_tree(repo)?,
                Err(e) => return Err(e.into()),
            };
            let mut merge =
                merge_keep_both(repo, &base, &local.tree()?, &remote.tree()?, stamp, untaken)?;
            conflicts.append(&mut merge.conflicts);
            let tree = repo.find_tree(merge.index.write_tree_to(repo)?)?;
            let message = format!("Merge remote-tracking branch '{REMOTE}/{branch}'\n");
            let id = repo.commit(
                None,
                &committer,
                &committer,
                &message,
                &tree,
                &[local, remote],
            )?;
            return Ok((repo.find_commit(id)?, 0));
        }

        let mut onto = remote.clone();
        let mut replayed = 0;
        for commit in &commits {
            let base = match commit.parents().next() {
                Some(parent) => parent.tree()?,
                None => empty_tree(repo)?,
            };
            let mut merge =
                merge_keep_both(repo, &base, &commit.tree()?, &onto.tree()?, stamp, untaken)?;
            conflicts.append(&mut merge.conflicts);
            let tree = merge.index.write_tree_to(repo)?;
            if tree == onto.tree_id() {
                continue;
            }
            let message = String::from_utf8_lossy(commit.message_raw_bytes());
            let id = repo.commit(
                None,
                &commit.author(),
                &committer,
                &message,
                &repo.find_tree(tree)?,
                &[&onto],
            )?;
            onto = repo.find_commit(id)?;
            replayed += 1;
        }
        Ok((onto, replayed))
    }

    /// Renames the local branch to the remote's, so every device works on the same branch.
    fn adopt_branch(&self, local: &str, remote: &str) -> Result<()> {
        if local == remote {
            return Ok(());
        }
        if self.git.find_branch(remote, BranchType::Local).is_ok() {
            return Err(Error::Git(format!(
                "cannot switch to the remote's branch \"{remote}\": a local branch with that name \
                 already exists"
            )));
        }
        match head_commit(&self.git)? {
            None => self.git.set_head(&format!("refs/heads/{remote}"))?,
            Some(_) => {
                self.git
                    .find_branch(local, BranchType::Local)?
                    .rename(remote, false)?;
            }
        }
        Ok(())
    }

    fn unchanged(&self) -> Result<PullOutcome> {
        Ok(PullOutcome {
            state: self.sync_state()?,
            updated: Vec::new(),
            merged: Vec::new(),
            conflicts: Vec::new(),
            rebased: 0,
        })
    }
}

/// Old path -> new path for every file renamed between two trees.
fn renames(repo: &Repository, old: &Tree<'_>, new: &Tree<'_>) -> Result<HashMap<Vec<u8>, Vec<u8>>> {
    let mut diff = repo.diff_tree_to_tree(Some(old), Some(new), None)?;
    diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
    Ok(diff
        .deltas()
        .filter(|delta| delta.status() == Delta::Renamed)
        .filter_map(|delta| {
            let from = delta.old_file().path_bytes()?.to_vec();
            let to = delta.new_file().path_bytes()?.to_vec();
            Some((from, to))
        })
        .collect())
}

/// Hands credentials to libgit2 (in memory only), only for URLs under `Creds::url_prefix`, and
/// gives up after a few rejected attempts.
struct Auth<'a> {
    creds: Option<&'a Creds>,
    attempts: Cell<u32>,
    failure: RefCell<Option<String>>,
}

impl<'a> Auth<'a> {
    fn new(creds: Option<&'a Creds>) -> Self {
        Auth {
            creds,
            attempts: Cell::new(0),
            failure: RefCell::new(None),
        }
    }

    fn callbacks(&'a self) -> RemoteCallbacks<'a> {
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(move |url, _, allowed| {
            let failure = match self.creds {
                None => "the remote repository requires signing in".to_owned(),
                Some(creds) if !url.starts_with(&creds.url_prefix) => format!(
                    "the token is only sent to {}, not to {} (check remote.origin.pushurl and \
                     url.*.insteadOf in the git config)",
                    creds.url_prefix,
                    origin(url)
                ),
                Some(_) if !allowed.contains(CredentialType::USER_PASS_PLAINTEXT) => {
                    "the remote asked for a sign-in method other than a token".to_owned()
                }
                Some(_) if self.attempts.get() >= MAX_AUTH_ATTEMPTS => {
                    "the remote did not accept the token; sign in again".to_owned()
                }
                Some(creds) => {
                    self.attempts.set(self.attempts.get() + 1);
                    return Cred::userpass_plaintext(&creds.username, &creds.password);
                }
            };
            let error = git2::Error::from_str(&failure);
            self.failure.replace(Some(failure));
            Err(error)
        });
        callbacks
    }

    fn error(&self, e: git2::Error) -> Error {
        match self.failure.take() {
            Some(failure) => Error::Auth(failure),
            None => e.into(),
        }
    }
}

/// "https://user@host:8080/x.git" -> "https://host:8080", for messages.
fn origin(url: &str) -> String {
    let url = without_userinfo(url);
    let path_start = url.find("://").map_or(0, |i| i + 3);
    match url[path_start..].find('/') {
        Some(i) => url[..path_start + i].to_owned(),
        None => url,
    }
}

/// Honours `http.proxy` and the usual proxy environment variables, like git.
fn proxy() -> ProxyOptions<'static> {
    let mut proxy = ProxyOptions::new();
    proxy.auto();
    proxy
}

#[cfg(test)]
mod tests {
    use super::{origin, Creds};

    #[test]
    fn debug_output_hides_the_token() {
        let creds = Creds {
            username: "x-access-token".into(),
            password: "ghs_secret".into(),
            url_prefix: "https://github.com/".into(),
        };
        let shown = format!("{creds:?}");
        assert!(shown.contains("x-access-token"));
        assert!(!shown.contains("ghs_secret"));
    }

    #[test]
    fn origins_drop_userinfo_and_path() {
        assert_eq!(
            origin("https://u:p@example.com:8080/a/b.git"),
            "https://example.com:8080"
        );
        assert_eq!(origin("http://127.0.0.1:1"), "http://127.0.0.1:1");
    }
}
