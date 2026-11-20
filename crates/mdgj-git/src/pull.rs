//! Pull: what GitHub's commits would change, applied at once when each file changed on one side
//! only, and after the user's choices when a file changed on both. Every applied pull can be
//! undone.
//!
//! Names follow docs/architecture.md: `L` is HEAD, `R` is origin/<branch>, `B` their merge-base,
//! `W` the working tree; "mine" is W (unpushed commits and uncommitted edits alike), "GitHub's" is R.
//! Renames are followed as git follows them, so an entry retitled on one device and edited on the
//! other is one file changed on both sides, not a deletion and a new file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use chrono::TimeZone;
use git2::{ErrorCode, Index, MergeOptions, Oid, Repository, Signature, Sort};
use serde::{Deserialize, Serialize};

use crate::diff3::{self, MergeSuggestion};
use crate::repo::{changes, empty_tree, head_commit, lossy, now_ms, signature, RawChange};
use crate::worktree::{
    self, build_tree, entry, exists, files, regular, tree_files, Files, Snapshot, GITLINK,
};
use crate::{Author, ChangeKind, Error, FileChange, Repo, Result, SyncState};

/// Commits listed in a preview or summary, per side; the counts are complete.
const LISTED: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullPlan {
    /// No unpushed commits: the branch moves to GitHub's.
    FastForward,
    /// Unpushed commits are replayed after GitHub's, keeping history a line.
    Rebase,
    /// A merge commit with parents `[L, R]`; always shown to the user first.
    Merge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub id: String,
    /// The full message, trailing whitespace trimmed.
    pub message: String,
    pub author: String,
    /// Author time, unix ms.
    pub time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullPreview {
    /// Fingerprint of branch, HEAD, origin's branch and the working tree; `apply_pull` checks it.
    pub id: String,
    pub branch: String,
    pub plan: PullPlan,
    /// No file changed on both sides and no merge commit is needed: Pull applies it without asking.
    pub automatic: bool,
    /// GitHub's new commits, newest first, at most 50.
    pub incoming: Vec<CommitInfo>,
    pub incoming_count: u32,
    /// Unpushed commits, newest first, at most 50.
    pub local: Vec<CommitInfo>,
    pub local_count: u32,
    /// How the files changed on GitHub only will change the working tree.
    pub auto: Vec<FileChange>,
    /// Files changed on both sides, sorted by path.
    pub overlaps: Vec<Overlap>,
    /// What Resolve with git would do (empty without overlaps).
    pub git_merge: GitMerge,
}

/// What Resolve with git would do with a review.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitMerge {
    /// Files git leaves with conflict markers, found by running the same merge in memory.
    pub conflicted: Vec<String>,
    /// Files whose uncommitted changes are committed first, because the merge writes them.
    pub uncommitted: Vec<String>,
}

/// How a file changed on both sides: what happened here, then on GitHub. A rename may come with
/// changes to the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapKind {
    BothModified,
    BothAdded,
    /// Modified here, deleted on GitHub.
    ModifiedDeleted,
    /// Deleted here, modified on GitHub.
    DeletedModified,
    /// Modified here, renamed on GitHub.
    ModifiedRenamed,
    /// Renamed here, modified on GitHub.
    RenamedModified,
    /// Renamed here and on GitHub, to different names.
    BothRenamed,
    /// Renamed here, deleted on GitHub.
    RenamedDeleted,
    /// Deleted here, renamed on GitHub.
    DeletedRenamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Mine,
    Theirs,
    Both,
    Merged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlap {
    /// The file's name after a merge (for a rename on one side, the new name), and the review's
    /// key for it.
    pub path: String,
    pub kind: OverlapKind,
    /// A present side isn't UTF-8 text (or holds a NUL byte, or isn't a regular file).
    pub binary: bool,
    /// Where mine is; `None` if deleted here.
    pub mine_path: Option<String>,
    /// Where GitHub's is; `None` if deleted on GitHub.
    pub theirs_path: Option<String>,
    /// The merge commit records this file's resolution: both sides' commits changed it (plan
    /// `merge` only). Otherwise the resolution stays as an uncommitted change.
    pub committed: bool,
    /// An opaque fingerprint of the three versions, so a fresh review can tell the files that
    /// didn't change since the last one.
    pub versions: String,
    /// The resolutions allowed, in the order the review offers them.
    pub choices: Vec<Choice>,
    /// Where Keep both saves GitHub's version; `Some` iff `both` is allowed.
    pub copy: Option<String>,
    /// The prepared merge; `Some` iff `merged` is allowed.
    pub suggestion: Option<MergeSuggestion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum Resolution {
    Mine,
    Theirs,
    Both,
    /// Exactly these bytes, with mine's file mode.
    Merged {
        text: String,
    },
}

impl Resolution {
    pub fn choice(&self) -> Choice {
        match self {
            Resolution::Mine => Choice::Mine,
            Resolution::Theirs => Choice::Theirs,
            Resolution::Both => Choice::Both,
            Resolution::Merged { .. } => Choice::Merged,
        }
    }
}

/// `{ "path": "...", "choice": "merged", "text": "..." }` from the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub path: String,
    #[serde(flatten)]
    pub resolution: Resolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFile {
    pub path: String,
    pub kind: OverlapKind,
    pub choice: Choice,
    pub copy: Option<String>,
}

/// What an applied pull did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSummary {
    /// Unix ms.
    pub at: i64,
    pub plan: PullPlan,
    pub incoming: Vec<CommitInfo>,
    pub incoming_count: u32,
    /// Unpushed commits placed after GitHub's (plan `rebase`), else 0.
    pub rebased: u32,
    /// The merge commit (plan `merge`).
    pub merge_commit: Option<String>,
    /// How the working tree changed, Keep both copies included.
    pub files: Vec<FileChange>,
    pub resolved: Vec<ResolvedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PullOutcome {
    UpToDate {
        state: SyncState,
    },
    Applied {
        state: SyncState,
        summary: SyncSummary,
    },
    /// Nothing was written: the user decides.
    Review {
        state: SyncState,
        preview: PullPreview,
    },
}

impl PullOutcome {
    /// The sync state after the pull.
    pub fn state(&self) -> &SyncState {
        match self {
            PullOutcome::UpToDate { state }
            | PullOutcome::Applied { state, .. }
            | PullOutcome::Review { state, .. } => state,
        }
    }
}

impl Repo {
    /// Fetches, then applies GitHub's commits at once if the preview is automatic; otherwise
    /// returns the review, with nothing written.
    pub fn pull(&self, creds: Option<&crate::Creds>, author: &Author) -> Result<PullOutcome> {
        self.locked()?;
        self.fetch_origin(creds)?;
        self.pull_fetched(author)
    }

    /// What a pull would do, from local data; `None` when up to date. Writes nothing: no ref,
    /// index, file or object.
    pub fn preview_pull(&self) -> Result<Option<PullPreview>> {
        self.locked()?;
        let snapshot = Snapshot::take(&self.git)?;
        Analysis::new(self, &snapshot)?
            .map(|analysis| analysis.preview())
            .transpose()
    }

    /// Applies the review `id` with one resolution per overlap. `message` is the merge commit's,
    /// required for plan `merge` and ignored otherwise. `Error::StaleReview` (nothing written) if
    /// the journal, HEAD or origin's branch changed since the review was computed.
    pub fn apply_pull(
        &self,
        id: &str,
        resolutions: &[Resolved],
        message: Option<&str>,
        author: &Author,
    ) -> Result<SyncSummary> {
        self.locked()?;
        let snapshot = Snapshot::take(&self.git)?;
        let analysis = self.fresh(&snapshot, id)?;
        self.apply(&analysis, resolutions, message, author)
    }

    pub(crate) fn pull_fetched(&self, author: &Author) -> Result<PullOutcome> {
        let snapshot = Snapshot::take(&self.git)?;
        let Some(analysis) = Analysis::new(self, &snapshot)? else {
            return Ok(PullOutcome::UpToDate {
                state: self.sync_state()?,
            });
        };
        if analysis.automatic() {
            let summary = self.apply(&analysis, &[], None, author)?;
            Ok(PullOutcome::Applied {
                state: self.sync_state()?,
                summary,
            })
        } else {
            let preview = analysis.preview()?;
            Ok(PullOutcome::Review {
                state: self.sync_state()?,
                preview,
            })
        }
    }

    /// The analysis of the journal as it is now, if it is still the review `id`.
    pub(crate) fn fresh<'s>(&self, snapshot: &'s Snapshot, id: &str) -> Result<Analysis<'s>> {
        match Analysis::new(self, snapshot)? {
            Some(analysis) if analysis.id == id => Ok(analysis),
            _ => Err(Error::StaleReview),
        }
    }

    fn apply(
        &self,
        analysis: &Analysis<'_>,
        resolutions: &[Resolved],
        message: Option<&str>,
        author: &Author,
    ) -> Result<SyncSummary> {
        let picks = analysis.resolve(resolutions)?;
        let message = match (analysis.plan, message) {
            (PullPlan::Merge, Some(message)) if !message.trim().is_empty() => message,
            (PullPlan::Merge, _) => return Err(Error::EmptyMessage),
            _ => "",
        };
        let snapshot = analysis.snapshot;
        let target = analysis.target(&picks)?;
        let plan = worktree::plan(snapshot, &target)?;
        let committer = signature(author)?;

        // The new head's objects are written now; the branch only moves once the files are done.
        let (head, rebased, merge_commit) = match analysis.plan {
            PullPlan::FastForward => (analysis.remote, 0, None),
            PullPlan::Rebase => {
                let replayed = replay(
                    &self.git,
                    &analysis.unpushed,
                    analysis.remote,
                    Some(&committer),
                )?
                .ok_or_else(|| {
                    Error::Git("an unpushed commit no longer replays onto the remote".into())
                })?;
                (replayed.head, replayed.count, None)
            }
            PullPlan::Merge => {
                let id = self.merge_commit(analysis, &target, &picks, message, &committer)?;
                (id, 0, Some(id.to_string()))
            }
        };

        let (incoming, incoming_count) =
            listed(&snapshot.repo, Some(analysis.remote), analysis.local)?;
        let target_tree = build_tree(&snapshot.repo, &target)?;
        let files = changes(&snapshot.repo, snapshot.tree, target_tree)?
            .iter()
            .map(RawChange::public)
            .collect();
        let resolved = analysis
            .overlaps
            .iter()
            .zip(&picks)
            .map(|(overlap, pick)| ResolvedFile {
                path: lossy(&overlap.path),
                kind: overlap.kind,
                choice: pick.choice(),
                copy: match pick {
                    Resolution::Both => overlap.copy.as_deref().map(lossy),
                    _ => None,
                },
            })
            .collect();
        let summary = SyncSummary {
            at: now_ms(),
            plan: analysis.plan,
            incoming,
            incoming_count,
            rebased,
            merge_commit,
            files,
            resolved,
        };

        // Undo's record comes first, so every file the pull touches is covered by it. Anything
        // failing from here on puts back exactly what was there: the files still as this pull
        // left them, the index, and the previous undo record.
        let previous = self.undo_backup()?;
        let index = self.index_bytes()?;
        if let Err(error) = self.record_pull(snapshot, analysis, head, &plan, &summary, &committer)
        {
            return Err(also(error, self.undo_restore(previous)));
        }
        let done = match worktree::execute(&snapshot.repo, &plan) {
            Ok(done) => done,
            Err(failed) => {
                let restored = worktree::rollback(&snapshot.repo, &plan, &failed.done)
                    .and_then(|()| self.undo_restore(previous));
                return Err(also(failed.error, restored));
            }
        };
        if let Err(error) = self.move_branch(analysis, head) {
            let restored = worktree::rollback(&snapshot.repo, &plan, &done)
                .and_then(|()| self.replace_index(index.as_deref()))
                .and_then(|()| self.undo_restore(previous));
            return Err(also(error, restored));
        }
        Ok(summary)
    }

    /// The index follows the new head where it followed the old one (stat data kept where
    /// unchanged), as `git merge` updates it; an entry staged with git stays staged (the analysis
    /// refused the pull if it changes that path). Then the branch moves, only if it is still where
    /// the analysis found it.
    fn move_branch(&self, analysis: &Analysis<'_>, head: Oid) -> Result<()> {
        let mut index = self.git.index()?;
        index.read(true)?;
        let staged = files(&index);
        let new = tree_files(&self.git.find_commit(head)?.tree()?)?;
        let paths: BTreeSet<&Vec<u8>> = staged
            .keys()
            .chain(analysis.ours.keys())
            .chain(new.keys())
            .collect();
        let changed: Vec<&Vec<u8>> = paths
            .into_iter()
            .filter(|path| {
                let now = staged.get(*path);
                now == analysis.ours.get(*path) && now != new.get(*path)
            })
            .collect();
        // Removals first: with core.ignorecase, a removal can match an entry just added under
        // another case.
        for path in changed.iter().filter(|path| !new.contains_key(**path)) {
            index.remove_path(Path::new(std::ffi::OsStr::from_bytes(path)))?;
        }
        for path in changed {
            if let Some(&(id, mode)) = new.get(path) {
                index.add(&entry(path, id, mode))?;
            }
        }
        index.write()?;
        let name = format!("refs/heads/{}", analysis.branch);
        let log = format!(
            "mdgitjournal: pull ({})",
            match analysis.plan {
                PullPlan::FastForward => "fast-forward",
                PullPlan::Rebase => "rebase",
                PullPlan::Merge => "merge",
            }
        );
        let moved = match analysis.local {
            Some(local) => self.git.reference_matching(&name, head, true, local, &log),
            None => self.git.reference(&name, head, false, &log),
        };
        match moved {
            Ok(_) => Ok(()),
            Err(e) if matches!(e.code(), ErrorCode::Modified | ErrorCode::Exists) => {
                Err(Error::StaleReview)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// A commit with parents `[L, R]` of what was committed on each side: per path, the side
    /// that changed it, and for files both sides' commits changed differently, the user's
    /// resolution (with Keep both's copy). Uncommitted edits stay uncommitted.
    fn merge_commit(
        &self,
        analysis: &Analysis<'_>,
        target: &Files,
        picks: &[&Resolution],
        message: &str,
        signature: &Signature<'_>,
    ) -> Result<Oid> {
        let files = analysis.merge_files(target, picks)?;
        for &(id, mode) in files.values() {
            if mode != GITLINK {
                self.persist(analysis.snapshot, id)?;
            }
        }
        let tree = self.git.find_tree(build_tree(&self.git, &files)?)?;
        let local = analysis
            .local
            .ok_or_else(|| Error::Git("a merge needs local commits".into()))?;
        let local = self.git.find_commit(local)?;
        let remote = self.git.find_commit(analysis.remote)?;
        let message = git2::message_prettify(message, None)?;
        Ok(self.git.commit(
            None,
            signature,
            signature,
            &message,
            &tree,
            &[&local, &remote],
        )?)
    }
}

/// `error`, unless putting things back after it failed as well.
fn also(error: Error, restored: Result<()>) -> Error {
    match restored {
        Ok(()) => error,
        Err(also) => Error::Io(format!("{error}; putting things back failed too: {also}")),
    }
}

/// A file changed on both sides, as found: one path, or the names a rename went between.
struct Found {
    /// See [`Overlap::path`].
    path: Vec<u8>,
    kind: OverlapKind,
    binary: bool,
    choices: Vec<Choice>,
    copy: Option<Vec<u8>>,
    /// Where each side's version is; `None` where that side has none.
    base_path: Option<Vec<u8>>,
    mine_path: Option<Vec<u8>>,
    theirs_path: Option<Vec<u8>>,
    /// The base, mine and theirs paths, sorted, without repeats: every path the overlap decides.
    paths: Vec<Vec<u8>>,
    committed: bool,
}

/// Everything a preview and an apply need, computed through the snapshot's handle, so nothing is
/// stored.
pub(crate) struct Analysis<'s> {
    pub snapshot: &'s Snapshot,
    pub id: String,
    pub branch: String,
    pub local: Option<Oid>,
    pub remote: Oid,
    pub plan: PullPlan,
    base_tree: Oid,
    remote_tree: Oid,
    base: Files,
    ours: Files,
    theirs: Files,
    /// B..L, oldest first.
    unpushed: Vec<Oid>,
    /// Paths GitHub changed since B.
    remote_changed: BTreeSet<Vec<u8>>,
    /// Every path an overlap decides.
    covered: BTreeSet<Vec<u8>>,
    overlaps: Vec<Found>,
}

impl<'s> Analysis<'s> {
    /// `None` when there is nothing to pull.
    pub fn new(repo: &Repo, snapshot: &'s Snapshot) -> Result<Option<Analysis<'s>>> {
        let git = &snapshot.repo;
        let branch = repo.branch()?;
        let Some(remote) = repo.remote_branch(&branch)? else {
            return Ok(None);
        };
        let local = head_commit(git)?.map(|commit| commit.id());
        let fast_forward = match local {
            None => true,
            Some(local) if local == remote || git.graph_descendant_of(local, remote)? => {
                return Ok(None)
            }
            Some(local) => git.graph_descendant_of(remote, local)?,
        };
        let base = match local {
            Some(local) if fast_forward => Some(local),
            Some(local) => match git.merge_base(local, remote) {
                Ok(base) => Some(base),
                Err(e) if e.code() == ErrorCode::NotFound => None,
                Err(e) => return Err(e.into()),
            },
            None => None,
        };
        let tree_of = |commit: Option<Oid>| -> Result<Oid> {
            match commit {
                Some(id) => Ok(git.find_commit(id)?.tree_id()),
                None => Ok(empty_tree(git)?.id()),
            }
        };
        let base_tree = tree_of(base)?;
        let remote_tree = tree_of(Some(remote))?;
        let base_files = tree_files(&git.find_tree(base_tree)?)?;
        let ours = tree_files(&git.find_tree(tree_of(local)?)?)?;
        let theirs = tree_files(&git.find_tree(remote_tree)?)?;
        let work = &snapshot.files;

        let remote_changed: BTreeSet<Vec<u8>> = base_files
            .keys()
            .chain(theirs.keys())
            .filter(|p| base_files.get(*p) != theirs.get(*p))
            .cloned()
            .collect();

        let stamp = stamp(git.find_commit(remote)?.time().seconds());
        let workdir = crate::repo::workdir(git)?;
        let mut names = Names::new(workdir, [work, &ours, &base_files, &theirs]);
        let sides = Sides {
            git,
            base: &base_files,
            work,
            theirs: &theirs,
        };
        let mut overlaps = Vec::new();
        let mut covered = BTreeSet::new();

        // Files renamed on one side and changed on the other, or renamed differently on both.
        let mine_renames = renames(git, base_tree, snapshot.tree)?;
        let their_renames = renames(git, base_tree, remote_tree)?;
        let renamed: BTreeSet<&Vec<u8>> = mine_renames.keys().chain(their_renames.keys()).collect();
        for from in renamed {
            let mine = mine_renames
                .get(from)
                .or_else(|| work.contains_key(from).then_some(from));
            let gh = their_renames
                .get(from)
                .or_else(|| theirs.contains_key(from).then_some(from));
            // A new name the other side has a file of its own at is left to the per-path rules.
            if mine_renames
                .get(from)
                .is_some_and(|to| theirs.contains_key(to) && gh != Some(to))
                || their_renames
                    .get(from)
                    .is_some_and(|to| work.contains_key(to) && mine != Some(to))
            {
                continue;
            }
            let b = base_files.get(from);
            let w = mine.and_then(|p| work.get(p));
            let r = gh.and_then(|p| theirs.get(p));
            let changed_here = mine != Some(from) || w != b;
            let changed_there = gh != Some(from) || r != b;
            if !changed_here || !changed_there || (mine == gh && w == r) {
                continue;
            }
            let (kind, path) = match (mine, gh) {
                (Some(m), Some(g)) if m == g => (OverlapKind::BothModified, m),
                (Some(m), Some(g)) if m == from => (OverlapKind::ModifiedRenamed, g),
                (Some(m), Some(g)) if g == from => (OverlapKind::RenamedModified, m),
                (Some(m), Some(_)) => (OverlapKind::BothRenamed, m),
                (Some(m), None) => (OverlapKind::RenamedDeleted, m),
                (None, Some(g)) => (OverlapKind::DeletedRenamed, g),
                (None, None) => continue,
            };
            let found = sides.found(
                kind,
                path.clone(),
                [Some(from.clone()), mine.cloned(), gh.cloned()],
                &mut names,
                &stamp,
            )?;
            covered.extend(found.paths.iter().cloned());
            overlaps.push(found);
        }

        // Every other file, path by path.
        for path in &remote_changed {
            if covered.contains(path) {
                continue;
            }
            let (b, w, r) = (base_files.get(path), work.get(path), theirs.get(path));
            if b == w || w == r {
                continue;
            }
            let kind = match (b, w, r) {
                (None, ..) => OverlapKind::BothAdded,
                (_, _, None) => OverlapKind::ModifiedDeleted,
                (_, None, _) => OverlapKind::DeletedModified,
                _ => OverlapKind::BothModified,
            };
            let at = |entry: Option<&(Oid, u32)>| entry.map(|_| path.clone());
            let found = sides.found(
                kind,
                path.clone(),
                [at(b), at(w), at(r)],
                &mut names,
                &stamp,
            )?;
            overlaps.push(found);
        }
        for found in &overlaps {
            covered.extend(found.paths.iter().cloned());
        }
        overlaps.sort_by(|a, b| a.path.cmp(&b.path));

        // Something staged with git stays staged, so the pull mustn't change it.
        let staged = match git.path().join("index") {
            path if path.exists() => files(&Index::open(&path)?),
            _ => Files::new(),
        };
        let indexed: BTreeSet<&Vec<u8>> = staged.keys().chain(ours.keys()).collect();
        for path in indexed {
            let now = staged.get(path);
            if now != ours.get(path)
                && (covered.contains(path)
                    || (remote_changed.contains(path) && now != theirs.get(path)))
            {
                let path = lossy(path);
                return Err(Error::Git(format!(
                    "\"{path}\" is staged with git, and GitHub changed it; commit it, or unstage \
                     it with git restore --staged \"{path}\", and pull again"
                )));
            }
        }

        let mut unpushed = Vec::new();
        let plan = match local {
            Some(local) if !fast_forward => {
                let mut walk = git.revwalk()?;
                walk.set_sorting(Sort::TOPOLOGICAL | Sort::REVERSE)?;
                walk.push(local)?;
                walk.hide(remote)?;
                unpushed = walk.collect::<Result<Vec<_>, _>>()?;
                let mut merges = false;
                for &id in &unpushed {
                    merges = merges || git.find_commit(id)?.parent_count() > 1;
                }
                // A file changed on both sides is resolved once, in a merge commit; replaying it
                // commit by commit would mean resolving it again for each.
                if merges || !overlaps.is_empty() {
                    PullPlan::Merge
                } else {
                    // Replayed in memory; it must give exactly what each side changed.
                    let expected =
                        build_tree(git, &history_of(&base_files, &ours, &theirs, &Files::new()))?;
                    match replay(git, &unpushed, remote, None)? {
                        Some(replayed) if replayed.tree == expected => PullPlan::Rebase,
                        _ => PullPlan::Merge,
                    }
                }
            }
            _ => PullPlan::FastForward,
        };
        if plan == PullPlan::Merge {
            for found in &mut overlaps {
                let side = |files: &Files| -> Vec<Option<(Oid, u32)>> {
                    found.paths.iter().map(|p| files.get(p).copied()).collect()
                };
                let (b, l, r) = (side(&base_files), side(&ours), side(&theirs));
                found.committed = l != b && l != r;
            }
        }

        let id = format!(
            "{branch}:{}:{remote}:{}",
            local.map_or_else(|| "0".to_owned(), |l| l.to_string()),
            snapshot.tree
        );
        let analysis = Analysis {
            snapshot,
            id,
            branch,
            local,
            remote,
            plan,
            base_tree,
            remote_tree,
            base: base_files,
            ours,
            theirs,
            unpushed,
            remote_changed,
            covered,
            overlaps,
        };
        // Anything the pull could write must be writable: refused here, there's no review.
        worktree::plan(snapshot, &analysis.widest())?;
        Ok(Some(analysis))
    }

    pub fn automatic(&self) -> bool {
        self.overlaps.is_empty() && self.plan != PullPlan::Merge
    }

    /// The working tree with every one-sided change from GitHub applied.
    fn with_auto(&self) -> Files {
        let mut files = self.snapshot.files.clone();
        for path in &self.remote_changed {
            if !self.covered.contains(path) {
                set(&mut files, path, self.theirs.get(path));
            }
        }
        files
    }

    /// Every version the pull might write: one-sided changes, GitHub's side of each overlap, and
    /// the Keep both copies.
    fn widest(&self) -> Files {
        let mut files = self.with_auto();
        for found in &self.overlaps {
            for path in &found.paths {
                if let Some(&theirs) = self.theirs.get(path) {
                    files.insert(path.clone(), theirs);
                }
            }
            if let Some((copy, theirs)) = self.copy_of(found) {
                files.insert(copy.clone(), theirs);
            }
        }
        files
    }

    /// Keep both's copy of GitHub's version: where it goes, and what.
    fn copy_of<'f>(&self, found: &'f Found) -> Option<(&'f Vec<u8>, (Oid, u32))> {
        let theirs = found
            .theirs_path
            .as_ref()
            .and_then(|p| self.theirs.get(p))?;
        Some((found.copy.as_ref()?, *theirs))
    }

    /// The uncommitted changes a merge of GitHub's commits writes over: changes from L to W at
    /// (or renamed from) a path GitHub changed. Resolve with git commits them first.
    pub fn merged_over(&self) -> Result<Vec<RawChange>> {
        let git = &self.snapshot.repo;
        let head = match self.local {
            Some(id) => git.find_commit(id)?.tree_id(),
            None => empty_tree(git)?.id(),
        };
        let touched =
            |path: &Vec<u8>| self.remote_changed.contains(path) || self.covered.contains(path);
        Ok(changes(git, head, self.snapshot.tree)?
            .into_iter()
            .filter(|change| touched(&change.path) || change.old_path.as_ref().is_some_and(touched))
            .collect())
    }

    /// What Resolve with git would do: commit `merged_over` first, then merge, here in memory.
    fn git_merge(&self) -> Result<GitMerge> {
        if self.overlaps.is_empty() {
            return Ok(GitMerge::default());
        }
        let git = &self.snapshot.repo;
        let over = self.merged_over()?;
        let mut ours = self.ours.clone();
        for change in &over {
            if let Some(old) = &change.old_path {
                ours.remove(old);
            }
            match self.snapshot.files.get(&change.path) {
                Some(&entry) if change.kind != ChangeKind::Deleted => {
                    ours.insert(change.path.clone(), entry);
                }
                _ => {
                    ours.remove(&change.path);
                }
            }
        }
        let ours = git.find_tree(build_tree(git, &ours)?)?;
        let index = git.merge_trees(
            &git.find_tree(self.base_tree)?,
            &ours,
            &git.find_tree(self.remote_tree)?,
            Some(&MergeOptions::new()),
        )?;
        let mut conflicted = BTreeSet::new();
        for conflict in index.conflicts()? {
            let conflict = conflict?;
            if let Some(entry) = conflict.our.or(conflict.their).or(conflict.ancestor) {
                conflicted.insert(lossy(&entry.path));
            }
        }
        Ok(GitMerge {
            conflicted: conflicted.into_iter().collect(),
            uncommitted: over.iter().map(|change| change.public().path).collect(),
        })
    }

    /// The merge commit's files: per path, the side whose commits changed it; for a file both
    /// sides' commits changed differently, the user's resolution of those commits' versions.
    fn merge_files(&self, target: &Files, picks: &[&Resolution]) -> Result<Files> {
        let mut files = history_of(&self.base, &self.ours, &self.theirs, target);
        for (found, pick) in self.overlaps.iter().zip(picks) {
            for path in &found.paths {
                files.remove(path);
            }
            let take = |files: &mut Files, from: &Files| {
                for path in &found.paths {
                    if let Some(&entry) = from.get(path) {
                        files.insert(path.clone(), entry);
                    }
                }
            };
            if !found.committed {
                // Only one side's commits changed it (or both the same way); any resolution of
                // uncommitted edits stays uncommitted.
                let unchanged = found
                    .paths
                    .iter()
                    .all(|path| self.ours.get(path) == self.base.get(path));
                take(
                    &mut files,
                    if unchanged { &self.theirs } else { &self.ours },
                );
                continue;
            }
            match pick {
                Resolution::Mine => take(&mut files, &self.ours),
                Resolution::Theirs => take(&mut files, &self.theirs),
                Resolution::Both => {
                    take(&mut files, &self.ours);
                    if let Some((copy, theirs)) = self.copy_of(found) {
                        files.insert(copy.clone(), theirs);
                    }
                }
                Resolution::Merged { text } => {
                    let entry = self.committed_merge(found, text, target)?;
                    files.insert(found.path.clone(), entry);
                }
            }
        }
        Ok(files)
    }

    /// The merge commit's version of a file merged in the review: the merged text without the
    /// uncommitted edits it was made from, where those edits don't touch the merged lines (they
    /// stay uncommitted); where they do, the merged text as it is.
    fn committed_merge(&self, found: &Found, merged: &str, target: &Files) -> Result<(Oid, u32)> {
        let resolved = *target
            .get(&found.path)
            .ok_or_else(|| Error::Git("a merged file is missing from the result".into()))?;
        let git = &self.snapshot.repo;
        let at = |files: &Files| found.mine_path.as_ref().and_then(|p| files.get(p)).copied();
        let (Some((work, work_mode)), Some((local, local_mode))) =
            (at(&self.snapshot.files), at(&self.ours))
        else {
            return Ok(resolved);
        };
        if work == local {
            return Ok(resolved);
        }
        let (Some(work), Some(local)) =
            (text(git, work, work_mode)?, text(git, local, local_mode)?)
        else {
            return Ok(resolved);
        };
        match diff3::suggest(&work, &local, merged)?.text() {
            Some(committed) => Ok((git.blob(committed.as_bytes())?, resolved.1)),
            None => Ok(resolved),
        }
    }

    pub fn preview(&self) -> Result<PullPreview> {
        let git = &self.snapshot.repo;
        let (incoming, incoming_count) = listed(git, Some(self.remote), self.local)?;
        let (local, local_count) = listed(git, self.local, Some(self.remote))?;
        let auto_tree = build_tree(git, &self.with_auto())?;
        let auto = changes(git, self.snapshot.tree, auto_tree)?
            .iter()
            .map(RawChange::public)
            .collect();
        let overlaps = self
            .overlaps
            .iter()
            .map(|found| self.describe(found))
            .collect::<Result<_>>()?;
        Ok(PullPreview {
            id: self.id.clone(),
            branch: self.branch.clone(),
            plan: self.plan,
            automatic: self.automatic(),
            incoming,
            incoming_count,
            local,
            local_count,
            auto,
            overlaps,
            git_merge: self.git_merge()?,
        })
    }

    fn describe(&self, found: &Found) -> Result<Overlap> {
        let git = &self.snapshot.repo;
        let text_of = |files: &Files, path: &Option<Vec<u8>>| -> Result<String> {
            match path.as_ref().and_then(|p| files.get(p)) {
                Some(&(id, mode)) => Ok(text(git, id, mode)?.unwrap_or_default()),
                None => Ok(String::new()),
            }
        };
        let suggestion = if found.choices.contains(&Choice::Merged) {
            Some(diff3::suggest(
                &text_of(&self.base, &found.base_path)?,
                &text_of(&self.snapshot.files, &found.mine_path)?,
                &text_of(&self.theirs, &found.theirs_path)?,
            )?)
        } else {
            None
        };
        let id = |files: &Files, path: &Option<Vec<u8>>| match path
            .as_ref()
            .and_then(|p| files.get(p))
        {
            Some((id, mode)) => format!("{id}-{mode:o}"),
            None => "0".to_owned(),
        };
        Ok(Overlap {
            path: lossy(&found.path),
            kind: found.kind,
            binary: found.binary,
            mine_path: found.mine_path.as_deref().map(lossy),
            theirs_path: found.theirs_path.as_deref().map(lossy),
            committed: found.committed,
            versions: format!(
                "{}:{}:{}",
                id(&self.base, &found.base_path),
                id(&self.snapshot.files, &found.mine_path),
                id(&self.theirs, &found.theirs_path)
            ),
            choices: found.choices.clone(),
            copy: found.copy.as_deref().map(lossy),
            suggestion,
        })
    }

    /// One resolution per overlap, in overlap order, each one the overlap allows.
    fn resolve<'r>(&self, resolutions: &'r [Resolved]) -> Result<Vec<&'r Resolution>> {
        let mut by_path: HashMap<&str, &Resolution> = HashMap::new();
        for resolved in resolutions {
            let path = resolved.path.as_str();
            if !self.overlaps.iter().any(|found| lossy(&found.path) == path) {
                return Err(Error::Invalid(format!(
                    "\"{path}\" isn't changed on both sides, so it has nothing to resolve."
                )));
            }
            if by_path.insert(path, &resolved.resolution).is_some() {
                return Err(Error::Invalid(format!(
                    "\"{path}\" has more than one resolution."
                )));
            }
        }
        self.overlaps
            .iter()
            .map(|found| {
                let path = lossy(&found.path);
                let pick = by_path.get(path.as_str()).ok_or_else(|| {
                    Error::Invalid(format!("Choose what to keep for \"{path}\"."))
                })?;
                if !found.choices.contains(&pick.choice()) {
                    let choice = match pick.choice() {
                        Choice::Mine => "Keep mine",
                        Choice::Theirs => "Keep GitHub's",
                        Choice::Both => "Keep both",
                        Choice::Merged => "Merge",
                    };
                    return Err(Error::Invalid(format!(
                        "{choice} isn't possible for \"{path}\"."
                    )));
                }
                Ok(*pick)
            })
            .collect()
    }

    /// The working tree after the pull: mine where GitHub changed nothing, GitHub's where only it
    /// did, and the user's resolution where both did. Merged texts are stored in the snapshot.
    fn target(&self, picks: &[&Resolution]) -> Result<Files> {
        let mut files = self.with_auto();
        for (found, pick) in self.overlaps.iter().zip(picks) {
            match pick {
                // Mine is what is on disk, under whatever name it has here.
                Resolution::Mine => {}
                Resolution::Theirs => {
                    for path in &found.paths {
                        set(&mut files, path, self.theirs.get(path));
                    }
                }
                Resolution::Both => {
                    if let Some((copy, theirs)) = self.copy_of(found) {
                        files.insert(copy.clone(), theirs);
                    }
                }
                Resolution::Merged { text } => {
                    let mode = found
                        .mine_path
                        .as_ref()
                        .and_then(|p| self.snapshot.files.get(p))
                        .map_or(worktree::BLOB, |e| e.1);
                    for path in &found.paths {
                        files.remove(path);
                    }
                    let id = self.snapshot.repo.blob(text.as_bytes())?;
                    files.insert(found.path.clone(), (id, mode));
                }
            }
        }
        Ok(files)
    }
}

/// The three versions an overlap is made of, to describe one.
struct Sides<'a> {
    git: &'a Repository,
    base: &'a Files,
    work: &'a Files,
    theirs: &'a Files,
}

impl Sides<'_> {
    /// An overlap of `kind` whose versions are at `[base, mine, theirs]`.
    fn found(
        &self,
        kind: OverlapKind,
        path: Vec<u8>,
        [base_path, mine_path, theirs_path]: [Option<Vec<u8>>; 3],
        names: &mut Names<'_>,
        stamp: &str,
    ) -> Result<Found> {
        let at = |files: &Files, path: &Option<Vec<u8>>| {
            path.as_ref().and_then(|p| files.get(p)).copied()
        };
        let mut binary = false;
        for (id, mode) in [
            at(self.base, &base_path),
            at(self.work, &mine_path),
            at(self.theirs, &theirs_path),
        ]
        .into_iter()
        .flatten()
        {
            binary = binary || text(self.git, id, mode)?.is_none();
        }
        let choices = match kind {
            OverlapKind::ModifiedDeleted
            | OverlapKind::DeletedModified
            | OverlapKind::RenamedDeleted
            | OverlapKind::DeletedRenamed => vec![Choice::Mine, Choice::Theirs],
            _ if binary => vec![Choice::Mine, Choice::Theirs, Choice::Both],
            _ => vec![Choice::Mine, Choice::Theirs, Choice::Both, Choice::Merged],
        };
        // Keep both: after a rename, the two versions already have names of their own.
        let copy = choices.contains(&Choice::Both).then(|| match kind {
            OverlapKind::ModifiedRenamed
            | OverlapKind::RenamedModified
            | OverlapKind::BothRenamed => theirs_path.clone().unwrap_or_default(),
            _ => names.copy(&path, stamp),
        });
        let mut paths: Vec<Vec<u8>> = [&base_path, &mine_path, &theirs_path]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();
        Ok(Found {
            path,
            kind,
            binary,
            choices,
            copy,
            base_path,
            mine_path,
            theirs_path,
            paths,
            committed: false,
        })
    }
}

/// The base paths renamed between two trees, `old -> new`, as git's rename detection pairs them.
fn renames(git: &Repository, old: Oid, new: Oid) -> Result<HashMap<Vec<u8>, Vec<u8>>> {
    Ok(changes(git, old, new)?
        .into_iter()
        .filter(|change| change.kind == ChangeKind::Renamed)
        .filter_map(|change| Some((change.old_path?, change.path)))
        .collect())
}

/// Per path: GitHub's if the local commits didn't change it, the local commits' if GitHub didn't
/// (or both made the same change), else `target`'s.
fn history_of(base: &Files, ours: &Files, theirs: &Files, target: &Files) -> Files {
    let paths: BTreeSet<&Vec<u8>> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    let mut files = Files::new();
    for path in paths {
        let (b, l, r) = (base.get(path), ours.get(path), theirs.get(path));
        let pick = if l == b {
            r
        } else if r == b || l == r {
            l
        } else {
            target.get(path)
        };
        if let Some(&entry) = pick {
            files.insert(path.clone(), entry);
        }
    }
    files
}

fn set(files: &mut Files, path: &[u8], entry: Option<&(Oid, u32)>) {
    match entry {
        Some(&entry) => {
            files.insert(path.to_vec(), entry);
        }
        None => {
            files.remove(path);
        }
    }
}

/// The blob's text, if it is a regular file holding UTF-8 without NUL bytes.
pub(crate) fn text(repo: &Repository, id: Oid, mode: u32) -> Result<Option<String>> {
    if !regular(mode) {
        return Ok(None);
    }
    let blob = repo.find_blob(id)?;
    Ok(std::str::from_utf8(blob.content())
        .ok()
        .filter(|text| !text.contains('\0'))
        .map(str::to_owned))
}

struct Replayed {
    /// The last replayed commit (`onto` when only trees were computed).
    head: Oid,
    tree: Oid,
    count: u32,
}

/// Replays `commits` onto `onto`, oldest first: as commits with their original author and message
/// and `committer`, or with `None`, as trees only. `None` if one doesn't apply cleanly. Commits
/// that become empty are dropped.
fn replay(
    git: &Repository,
    commits: &[Oid],
    onto: Oid,
    committer: Option<&Signature<'_>>,
) -> Result<Option<Replayed>> {
    // Renames are followed, as git and the overlap analysis follow them.
    let options = MergeOptions::new();
    let mut head = git.find_commit(onto)?;
    let mut tree = head.tree()?;
    let mut count = 0;
    for &id in commits {
        let commit = git.find_commit(id)?;
        let parent = match commit.parents().next() {
            Some(parent) => parent.tree()?,
            None => empty_tree(git)?,
        };
        let mut index = git.merge_trees(&parent, &tree, &commit.tree()?, Some(&options))?;
        if index.has_conflicts() {
            return Ok(None);
        }
        let next = index.write_tree_to(git)?;
        if next == tree.id() {
            continue;
        }
        tree = git.find_tree(next)?;
        count += 1;
        if let Some(committer) = committer {
            let message = String::from_utf8_lossy(commit.message_raw_bytes());
            let id = git.commit(None, &commit.author(), committer, &message, &tree, &[&head])?;
            head = git.find_commit(id)?;
        }
    }
    Ok(Some(Replayed {
        head: head.id(),
        tree: tree.id(),
        count,
    }))
}

/// The commits reachable from `tip` but not from `hide`, newest first: at most 50, and how many.
fn listed(git: &Repository, tip: Option<Oid>, hide: Option<Oid>) -> Result<(Vec<CommitInfo>, u32)> {
    let Some(tip) = tip else {
        return Ok((Vec::new(), 0));
    };
    let mut walk = git.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    walk.push(tip)?;
    if let Some(hide) = hide {
        walk.hide(hide)?;
    }
    let mut list = Vec::new();
    let mut count = 0u32;
    for id in walk {
        let id = id?;
        count = count.saturating_add(1);
        if list.len() < LISTED {
            let commit = git.find_commit(id)?;
            let author = commit.author();
            list.push(CommitInfo {
                id: id.to_string(),
                message: String::from_utf8_lossy(commit.message_raw_bytes())
                    .trim_end()
                    .to_owned(),
                author: String::from_utf8_lossy(author.name_bytes()).into_owned(),
                time: author.when().seconds() * 1000,
            });
        }
    }
    Ok((list, count))
}

/// "20h28" of a unix time (not "2028", which reads as a year in a journal), in this device's time zone.
fn stamp(seconds: i64) -> String {
    match chrono::Local.timestamp_opt(seconds, 0).earliest() {
        Some(time) => time.format("%Hh%M").to_string(),
        None => "00h00".to_owned(),
    }
}

/// Free names for Keep both copies: not used in any of the trees, by another copy, or on disk
/// (ignored files included), compared without case, as APFS does.
struct Names<'a> {
    workdir: &'a Path,
    taken: HashSet<String>,
}

impl<'a> Names<'a> {
    fn new(workdir: &'a Path, trees: [&Files; 4]) -> Names<'a> {
        let taken = trees
            .iter()
            .flat_map(|files| files.keys())
            .map(|path| fold(path))
            .collect();
        Names { workdir, taken }
    }

    fn copy(&mut self, path: &[u8], stamp: &str) -> Vec<u8> {
        let mut n = 1;
        loop {
            let candidate = copy_name(path, stamp, n);
            if !self.taken.contains(&fold(&candidate)) && !exists(self.workdir, &candidate) {
                self.taken.insert(fold(&candidate));
                return candidate;
            }
            n += 1;
        }
    }
}

fn fold(path: &[u8]) -> String {
    String::from_utf8_lossy(path).to_lowercase()
}

/// `dir/name.md` -> `dir/name.from-github-<stamp>.md`, the `n`th time `…-<stamp>-<n>.md`. Names
/// without an extension get the suffix at the end.
pub(crate) fn copy_name(path: &[u8], stamp: &str, n: u32) -> Vec<u8> {
    let name_start = path.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    let (dir, name) = path.split_at(name_start);
    let (stem, ext) = match name.iter().rposition(|&b| b == b'.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, &b""[..]),
    };
    let suffix = match n {
        1 => format!(".from-github-{stamp}"),
        n => format!(".from-github-{stamp}-{n}"),
    };
    [dir, stem, suffix.as_bytes(), ext].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(path: &str, n: u32) -> String {
        String::from_utf8(copy_name(path.as_bytes(), "15h30", n)).unwrap()
    }

    #[test]
    fn keep_both_copies_sit_next_to_the_original() {
        assert_eq!(
            copy("2026-11-16-1402-run.md", 1),
            "2026-11-16-1402-run.from-github-15h30.md"
        );
        assert_eq!(
            copy("assets/x-5f3a09c2.jpg", 1),
            "assets/x-5f3a09c2.from-github-15h30.jpg"
        );
        assert_eq!(copy("notes", 1), "notes.from-github-15h30");
        assert_eq!(copy(".gitignore", 1), ".gitignore.from-github-15h30");
        assert_eq!(copy("a.b/c", 1), "a.b/c.from-github-15h30");
        assert_eq!(copy("x.md", 3), "x.from-github-15h30-3.md");
    }

    #[test]
    fn keep_both_names_are_free_whatever_the_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("x.from-github-15h30-2.md"),
            "ignored, on disk",
        )
        .unwrap();
        let id = Oid::ZERO_SHA1;
        let tree = Files::from([(b"X.FROM-GITHUB-15H30.md".to_vec(), (id, worktree::BLOB))]);
        let empty = Files::new();
        let mut names = Names::new(dir.path(), [&tree, &empty, &empty, &empty]);
        assert_eq!(names.copy(b"x.md", "15h30"), b"x.from-github-15h30-3.md");
        assert_eq!(names.copy(b"x.md", "15h30"), b"x.from-github-15h30-4.md");
    }

    #[test]
    fn resolutions_come_from_the_ui_as_flat_objects() {
        let merged: Resolved =
            serde_json::from_str(r#"{ "path": "a.md", "choice": "merged", "text": "x\r\n" }"#)
                .unwrap();
        assert_eq!(
            merged,
            Resolved {
                path: "a.md".into(),
                resolution: Resolution::Merged {
                    text: "x\r\n".into()
                }
            }
        );
        let mine: Resolved =
            serde_json::from_str(r#"{ "path": "b.md", "choice": "mine" }"#).unwrap();
        assert_eq!(mine.resolution, Resolution::Mine);
        assert!(
            serde_json::from_str::<Resolved>(r#"{ "path": "b.md", "choice": "mine?" }"#).is_err()
        );
        assert_eq!(
            serde_json::to_value(&merged).unwrap(),
            serde_json::json!({ "path": "a.md", "choice": "merged", "text": "x\r\n" })
        );
    }

    #[test]
    fn previews_and_chunks_serialize_as_the_contract_says() {
        use crate::diff3::MergeChunk;
        let overlap = Overlap {
            path: "a.md".into(),
            kind: OverlapKind::ModifiedDeleted,
            binary: false,
            mine_path: Some("a.md".into()),
            theirs_path: None,
            committed: false,
            versions: "1:2:0".into(),
            choices: vec![Choice::Mine, Choice::Theirs],
            copy: None,
            suggestion: Some(MergeSuggestion {
                clean: false,
                chunks: vec![
                    MergeChunk::Same { text: "s\n".into() },
                    MergeChunk::Clash {
                        base: "b".into(),
                        mine: "m".into(),
                        theirs: "t".into(),
                    },
                ],
            }),
        };
        assert_eq!(
            serde_json::to_value(&overlap).unwrap(),
            serde_json::json!({
                "path": "a.md", "kind": "modified_deleted", "binary": false,
                "minePath": "a.md", "theirsPath": null, "committed": false, "versions": "1:2:0",
                "choices": ["mine", "theirs"], "copy": null,
                "suggestion": { "clean": false, "chunks": [
                    { "kind": "same", "text": "s\n" },
                    { "kind": "clash", "base": "b", "mine": "m", "theirs": "t" },
                ] },
            })
        );
        assert_eq!(
            serde_json::to_value(PullPlan::FastForward).unwrap(),
            serde_json::json!("fast_forward")
        );
    }
}
