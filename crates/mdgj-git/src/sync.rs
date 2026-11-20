//! Remote operations: fetch, push, commit and push, connect.

use std::cell::{Cell, RefCell};
use std::fmt;

use git2::{
    AutotagOption, BranchType, Cred, CredentialType, ErrorCode, FetchOptions, FetchPrune,
    ProxyOptions, PushOptions, Remote, RemoteCallbacks,
};

use crate::repo::{head_commit, without_userinfo, REMOTE};
use crate::{Author, Error, PullOutcome, Repo, Result, SyncState};

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
pub(crate) struct RemoteBranches {
    default: Option<String>,
    names: Vec<String>,
}

impl Repo {
    /// Updates `refs/remotes/origin/*` and returns the new sync state. Works during a merge.
    pub fn fetch(&self, creds: Option<&Creds>) -> Result<SyncState> {
        self.fetch_origin(creds)?;
        self.sync_state()
    }

    /// Pushes the current branch (only `refs/heads/<branch>`). Never forces.
    pub fn push(&self, creds: Option<&Creds>) -> Result<PushOutcome> {
        let mut remote = self.origin()?;
        self.locked()?;
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
        // A pushed pull is no longer the user's to undo.
        self.clear_undo()?;
        Ok(PushOutcome::Pushed {
            state: self.sync_state()?,
        })
    }

    /// Commits the chosen `paths` (see `commit`), then pushes unless the remote has commits this
    /// device hasn't pulled. Commits already waiting are pushed too.
    pub fn commit_and_push(
        &self,
        message: &str,
        paths: &[String],
        author: &Author,
        creds: Option<&Creds>,
    ) -> Result<CommitPushOutcome> {
        self.origin()?;
        let committed = self.commit(message, paths, author)?.is_some();
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

    /// Connects to the repository at `url`. An empty remote receives the local history; otherwise
    /// the local branch takes the remote's default branch name and pulls it by the usual rules,
    /// even if the histories are unrelated: at once if automatic, else as a review (still
    /// connected). If connecting fails, the previous remote setting is restored, so a failed
    /// connect never looks connected.
    pub fn connect(
        &self,
        url: &str,
        creds: Option<&Creds>,
        author: &Author,
    ) -> Result<PullOutcome> {
        self.locked()?;
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
                Ok(PullOutcome::UpToDate {
                    state: self.sync_state()?,
                })
            }
        }
    }

    fn origin(&self) -> Result<Remote<'_>> {
        self.git.find_remote(REMOTE).map_err(|e| match e.code() {
            ErrorCode::NotFound => Error::NotConnected,
            _ => e.into(),
        })
    }

    pub(crate) fn fetch_origin(&self, creds: Option<&Creds>) -> Result<RemoteBranches> {
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
