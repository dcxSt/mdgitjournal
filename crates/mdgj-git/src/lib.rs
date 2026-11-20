//! libgit2 wrapper for mdgitjournal: init, status, diff, commit, fetch, pull (preview, review,
//! apply, undo), Resolve with git, push, connect.
//!
//! The rule everything here implements: a file changed on only one side flows automatically; a
//! file changed on both sides waits for the user. Nothing in this crate writes conflict markers
//! into the journal, except `Repo::start_external_merge`, which exists to do exactly that, the way
//! `git merge` does.

mod diff;
mod diff3;
mod error;
mod external;
mod pull;
mod repo;
mod sync;
mod undo;
mod worktree;

pub use diff::{DiffHunk, DiffLine, FileDiff, LineKind};
pub use diff3::{MergeChunk, MergeSuggestion};
pub use error::{Error, Result};
pub use external::MergeState;
pub use pull::{
    Choice, CommitInfo, Overlap, OverlapKind, PullOutcome, PullPlan, PullPreview, Resolution,
    Resolved, ResolvedFile, SyncSummary,
};
pub use repo::{Author, ChangeKind, FileChange, Repo, SyncState};
pub use sync::{CommitPushOutcome, Creds, PushOutcome};
