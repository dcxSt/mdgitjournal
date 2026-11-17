//! libgit2 wrapper: init, status, commit, fetch, pull (merge + keep-both), push, connect.

mod error;
mod merge;
mod repo;
mod sync;
mod worktree;

pub use error::{Error, Result};
pub use repo::{Author, ChangeKind, FileChange, Repo, SyncState};
pub use sync::{CommitPushOutcome, Conflict, Creds, PullOutcome, PushOutcome};
