//! Pull: what GitHub's commits would change, applied at once when each file changed on one side
//! only, and after the user's choices when a file changed on both. Every applied pull can be
//! undone.
//!
//! Names follow docs/architecture.md: `L` is HEAD, `R` is origin/<branch>, `B` their merge-base,
//! `W` the working tree; "mine" is W (unpushed commits and uncommitted edits alike), "GitHub's" is R.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use chrono::TimeZone;
use git2::{ErrorCode, MergeOptions, Oid, Repository, Signature, Sort};
use serde::{Deserialize, Serialize};

use crate::diff3::{self, MergeSuggestion};
use crate::repo::{changes, empty_tree, head_commit, lossy, now_ms, signature, RawChange};
use crate::worktree::{self, build_tree, exists, regular, tree_files, Files, Snapshot, GITLINK};
use crate::{Author, Error, FileChange, Repo, Result, SyncState};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapKind {
    BothModified,
    BothAdded,
    /// Modified here, deleted on GitHub.
    ModifiedDeleted,
    /// Deleted here, modified on GitHub.
    DeletedModified,
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
    pub path: String,
    pub kind: OverlapKind,
    /// A present side isn't UTF-8 text (or holds a NUL byte, or isn't a regular file).
    pub binary: bool,
    /// Full texts; `None` if that side has no file, or the file is binary.
    pub base: Option<String>,
    pub mine: Option<String>,
    pub theirs: Option<String>,
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

    /// The index becomes the new head's tree (stat data kept where unchanged), then the branch
    /// moves, only if it is still where the analysis found it.
    fn move_branch(&self, analysis: &Analysis<'_>, head: Oid) -> Result<()> {
        let mut index = self.git.index()?;
        index.read(false)?;
        index.read_tree(&self.git.find_commit(head)?.tree()?)?;
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

    /// A commit with parents `[L, R]`: per path, the side that changed it, and for paths both
    /// sides' commits changed differently, the user's resolution (with Keep both's copy).
    fn merge_commit(
        &self,
        analysis: &Analysis<'_>,
        target: &Files,
        picks: &[&Resolution],
        message: &str,
        signature: &Signature<'_>,
    ) -> Result<Oid> {
        let mut files = analysis.history(target);
        for (overlap, pick) in analysis.overlaps.iter().zip(picks) {
            if let (Resolution::Both, Some(copy)) = (pick, &overlap.copy) {
                if analysis.committed.contains(&overlap.path) {
                    if let Some(&entry) = target.get(copy) {
                        files.insert(copy.clone(), entry);
                    }
                }
            }
        }
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

/// A file changed on both sides, as found.
struct Found {
    path: Vec<u8>,
    kind: OverlapKind,
    binary: bool,
    choices: Vec<Choice>,
    copy: Option<Vec<u8>>,
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
    base: Files,
    ours: Files,
    theirs: Files,
    /// B..L, oldest first.
    unpushed: Vec<Oid>,
    /// Paths GitHub changed since B.
    remote_changed: BTreeSet<Vec<u8>>,
    /// Paths both sides' commits changed differently.
    committed: BTreeSet<Vec<u8>>,
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
        let files_of = |commit: Option<Oid>| -> Result<Files> {
            match commit {
                Some(id) => tree_files(&git.find_commit(id)?.tree()?),
                None => Ok(Files::new()),
            }
        };
        let base_files = files_of(base)?;
        let ours = files_of(local)?;
        let theirs = files_of(Some(remote))?;
        let work = &snapshot.files;

        let remote_changed: BTreeSet<Vec<u8>> = base_files
            .keys()
            .chain(theirs.keys())
            .filter(|p| base_files.get(*p) != theirs.get(*p))
            .cloned()
            .collect();
        let committed: BTreeSet<Vec<u8>> = remote_changed
            .iter()
            .filter(|p| base_files.get(*p) != ours.get(*p) && ours.get(*p) != theirs.get(*p))
            .cloned()
            .collect();

        let stamp = stamp(git.find_commit(remote)?.time().seconds());
        let workdir = crate::repo::workdir(git)?;
        let mut names = Names::new(workdir, [work, &ours, &base_files, &theirs]);
        let mut overlaps = Vec::new();
        for path in &remote_changed {
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
            let mut binary = false;
            for &(id, mode) in [b, w, r].into_iter().flatten() {
                binary = binary || text(git, id, mode)?.is_none();
            }
            let choices = match kind {
                OverlapKind::ModifiedDeleted | OverlapKind::DeletedModified => {
                    vec![Choice::Mine, Choice::Theirs]
                }
                _ if binary => vec![Choice::Mine, Choice::Theirs, Choice::Both],
                _ => vec![Choice::Mine, Choice::Theirs, Choice::Both, Choice::Merged],
            };
            let copy = choices
                .contains(&Choice::Both)
                .then(|| names.copy(path, &stamp));
            overlaps.push(Found {
                path: path.clone(),
                kind,
                binary,
                choices,
                copy,
            });
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
                if merges || !committed.is_empty() {
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
            base: base_files,
            ours,
            theirs,
            unpushed,
            remote_changed,
            committed,
            overlaps,
        };
        // Anything the pull could write must be writable: refused here, there's no review.
        worktree::plan(snapshot, &analysis.widest())?;
        Ok(Some(analysis))
    }

    pub fn automatic(&self) -> bool {
        self.overlaps.is_empty() && self.plan != PullPlan::Merge
    }

    fn is_overlap(&self, path: &[u8]) -> bool {
        self.overlaps
            .binary_search_by(|found| found.path.as_slice().cmp(path))
            .is_ok()
    }

    /// The working tree with every one-sided change from GitHub applied.
    fn with_auto(&self) -> Files {
        let mut files = self.snapshot.files.clone();
        for path in &self.remote_changed {
            if !self.is_overlap(path) {
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
            if let Some(&theirs) = self.theirs.get(&found.path) {
                files.insert(found.path.clone(), theirs);
                if let Some(copy) = &found.copy {
                    files.insert(copy.clone(), theirs);
                }
            }
        }
        files
    }

    /// Per path, the side whose commits changed it; for paths both changed differently, the
    /// target's version.
    fn history(&self, target: &Files) -> Files {
        history_of(&self.base, &self.ours, &self.theirs, target)
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
        })
    }

    fn describe(&self, found: &Found) -> Result<Overlap> {
        let git = &self.snapshot.repo;
        let path = &found.path;
        let text_of = |entry: Option<&(Oid, u32)>| -> Result<Option<String>> {
            match entry {
                Some(&(id, mode)) if !found.binary => text(git, id, mode),
                _ => Ok(None),
            }
        };
        let base = text_of(self.base.get(path))?;
        let mine = text_of(self.snapshot.files.get(path))?;
        let theirs = text_of(self.theirs.get(path))?;
        let suggestion = if found.choices.contains(&Choice::Merged) {
            Some(diff3::suggest(
                base.as_deref().unwrap_or(""),
                mine.as_deref().unwrap_or(""),
                theirs.as_deref().unwrap_or(""),
            )?)
        } else {
            None
        };
        Ok(Overlap {
            path: lossy(path),
            kind: found.kind,
            binary: found.binary,
            base,
            mine,
            theirs,
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
            let path = &found.path;
            let theirs = self.theirs.get(path);
            match pick {
                Resolution::Mine => {}
                Resolution::Theirs => set(&mut files, path, theirs),
                Resolution::Both => {
                    if let (Some(copy), Some(&theirs)) = (&found.copy, theirs) {
                        files.insert(copy.clone(), theirs);
                    }
                }
                Resolution::Merged { text } => {
                    let mode = self
                        .snapshot
                        .files
                        .get(path)
                        .map_or(worktree::BLOB, |e| e.1);
                    let id = self.snapshot.repo.blob(text.as_bytes())?;
                    files.insert(path.clone(), (id, mode));
                }
            }
        }
        Ok(files)
    }
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
    let mut options = MergeOptions::new();
    // Paths, as the overlap analysis sees them: renames are not followed.
    options.find_renames(false);
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

/// "hhmm" of a unix time, in this device's time zone.
fn stamp(seconds: i64) -> String {
    match chrono::Local.timestamp_opt(seconds, 0).earliest() {
        Some(time) => time.format("%H%M").to_string(),
        None => "0000".to_owned(),
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
        String::from_utf8(copy_name(path.as_bytes(), "1530", n)).unwrap()
    }

    #[test]
    fn keep_both_copies_sit_next_to_the_original() {
        assert_eq!(
            copy("2026-11-16-1402-run.md", 1),
            "2026-11-16-1402-run.from-github-1530.md"
        );
        assert_eq!(
            copy("assets/x-5f3a09c2.jpg", 1),
            "assets/x-5f3a09c2.from-github-1530.jpg"
        );
        assert_eq!(copy("notes", 1), "notes.from-github-1530");
        assert_eq!(copy(".gitignore", 1), ".gitignore.from-github-1530");
        assert_eq!(copy("a.b/c", 1), "a.b/c.from-github-1530");
        assert_eq!(copy("x.md", 3), "x.from-github-1530-3.md");
    }

    #[test]
    fn keep_both_names_are_free_whatever_the_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("x.from-github-1530-2.md"),
            "ignored, on disk",
        )
        .unwrap();
        let id = Oid::ZERO_SHA1;
        let tree = Files::from([(b"X.FROM-GITHUB-1530.md".to_vec(), (id, worktree::BLOB))]);
        let empty = Files::new();
        let mut names = Names::new(dir.path(), [&tree, &empty, &empty, &empty]);
        assert_eq!(names.copy(b"x.md", "1530"), b"x.from-github-1530-3.md");
        assert_eq!(names.copy(b"x.md", "1530"), b"x.from-github-1530-4.md");
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
            base: Some("a\n".into()),
            mine: Some("b\n".into()),
            theirs: None,
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
                "base": "a\n", "mine": "b\n", "theirs": null,
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
