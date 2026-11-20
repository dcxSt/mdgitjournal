//! Resolve with git: a standard `git merge` left in progress (conflict markers, `MERGE_HEAD`) for
//! the user to finish with their own tools, and an Abort that puts everything back.
//!
//! A merge the app starts is recorded in `.git/mdgitjournal/merge.json`, with a copy of the index
//! from before it (`merge-index`), so Abort restores the index byte for byte and can take back the
//! commit of uncommitted edits made to start the merge.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use git2::build::CheckoutBuilder;
use git2::{ErrorCode, Index, MergeOptions, Oid, RepositoryState};
use serde::{Deserialize, Serialize};

use crate::pull::PullPlan;
use crate::repo::{head_commit, head_tree_id, lossy, now_ms, read_if_exists, REMOTE};
use crate::worktree::{self, entry, files, tree_files, Files, Snapshot, GITLINK};
use crate::{Author, Error, Repo, Result};

const RECORD: &str = "merge.json";
const INDEX_COPY: &str = "merge-index";
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeState {
    /// Paths with conflict entries in the index, sorted.
    pub conflicted: Vec<String>,
    /// The app started it (Resolve with git), rather than the git command line.
    pub started_by_app: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    version: u32,
    /// HEAD before the merge (and before `precommit`); `None` if the branch was unborn.
    head: Option<String>,
    /// The commit of uncommitted edits made so the merge could start.
    precommit: Option<String>,
    merge_head: String,
    started_at: i64,
}

impl Repo {
    /// Starts `git merge origin/<branch>` for the review `id`, leaving it in progress. Uncommitted
    /// edits are committed first, with `message` (required then), so git can merge them and Abort
    /// can make them uncommitted again.
    pub fn start_external_merge(
        &self,
        id: &str,
        message: Option<&str>,
        author: &Author,
    ) -> Result<()> {
        self.locked()?;
        let snapshot = Snapshot::take(&self.git)?;
        let analysis = self.fresh(&snapshot, id)?;
        let dirty = snapshot.tree != head_tree_id(&self.git)?;
        let message = message.filter(|m| !m.trim().is_empty());
        if dirty && message.is_none() {
            return Err(Error::EmptyMessage);
        }
        if !dirty && analysis.plan == PullPlan::FastForward {
            return Err(Error::Invalid(
                "There is nothing to merge: Pull brings in GitHub's commits as they are.".into(),
            ));
        }
        let (branch, local, remote) = (analysis.branch.clone(), analysis.local, analysis.remote);

        let index = self.index_bytes()?;
        match &index {
            Some(bytes) => self.write_state_file(INDEX_COPY, bytes)?,
            None => self.remove_state_file(INDEX_COPY)?,
        }
        let precommit = match message {
            Some(message) if dirty => self.commit_snapshot(&snapshot, message, None, author)?,
            _ => None,
        };
        drop(analysis);
        drop(snapshot);
        if let Err(error) = self.merge_remote(&branch) {
            // Back to before: the edits uncommitted again, the index as it was.
            let restored = match precommit {
                Some(precommit) => self
                    .move_branch_back(&branch, local, precommit)
                    .and_then(|()| self.replace_index(index.as_deref())),
                None => Ok(()),
            };
            self.remove_state_file(INDEX_COPY)?;
            restored?;
            return Err(error);
        }

        let head = precommit
            .or(local)
            .ok_or_else(|| Error::Git("the merge has no commit to start from".into()))?;
        let git_dir = self.git.path();
        fs::write(
            git_dir.join("MERGE_MSG"),
            format!("Merge remote-tracking branch '{REMOTE}/{branch}'\n"),
        )?;
        fs::write(git_dir.join("ORIG_HEAD"), format!("{head}\n"))?;
        let record = Record {
            version: VERSION,
            head: local.map(|id| id.to_string()),
            precommit: precommit.map(|id| id.to_string()),
            merge_head: remote.to_string(),
            started_at: now_ms(),
        };
        let json = serde_json::to_vec_pretty(&record)
            .map_err(|e| Error::Io(format!("cannot write the merge record: {e}")))?;
        self.write_state_file(RECORD, &json)?;
        self.clear_undo()
    }

    /// `Some` while a git merge is in progress, whoever started it. Never writes.
    pub fn merge_state(&self) -> Result<Option<MergeState>> {
        if self.git.state() != RepositoryState::Merge {
            return Ok(None);
        }
        let index = Index::open(&self.git.path().join("index"))?;
        let mut conflicted = BTreeSet::new();
        for conflict in index.conflicts()? {
            let conflict = conflict?;
            if let Some(entry) = conflict.our.or(conflict.their).or(conflict.ancestor) {
                conflicted.insert(lossy(&entry.path));
            }
        }
        Ok(Some(MergeState {
            conflicted: conflicted.into_iter().collect(),
            started_by_app: self.merge_record()?.is_some(),
        }))
    }

    /// `git merge --abort`: every file the merge changed (or that conflicts) goes back to HEAD's
    /// version, discarding what was done to it since; other files are left alone. For a merge
    /// the app started, the index is restored exactly and the commit of uncommitted edits is
    /// taken back, so they are uncommitted again. Returns the paths put back.
    pub fn abort_merge(&self) -> Result<Vec<String>> {
        if self.git.state() != RepositoryState::Merge {
            return Err(Error::Invalid("No merge is in progress.".into()));
        }
        let record = self.merge_record()?;
        let head = head_commit(&self.git)?;
        let head_id = head.as_ref().map(|commit| commit.id());
        let head_files = match &head {
            Some(commit) => tree_files(&commit.tree()?)?,
            None => Files::new(),
        };
        let mut merged = Vec::new();
        for id in self.merge_heads()? {
            merged.push(tree_files(&self.git.find_commit(id)?.tree()?)?);
        }

        let index = Index::open(&self.git.path().join("index"))?;
        let staged = files(&index);
        let mut reset: BTreeSet<Vec<u8>> = BTreeSet::new();
        for conflict in index.conflicts()? {
            let conflict = conflict?;
            for entry in [conflict.ancestor, conflict.our, conflict.their]
                .into_iter()
                .flatten()
            {
                reset.insert(entry.path);
            }
        }
        // Only paths the merge could have changed: HEAD and the merged commit differ there.
        for path in staged.keys().chain(head_files.keys()) {
            let ours = head_files.get(path);
            if staged.get(path) != ours && merged.iter().any(|m| m.get(path) != ours) {
                reset.insert(path.clone());
            }
        }

        for path in &reset {
            let ours = head_files.get(path).copied();
            if ours.is_some_and(|(_, mode)| mode == GITLINK) {
                continue;
            }
            worktree::put(&self.git, path, ours)?;
        }

        let precommit = record
            .as_ref()
            .and_then(|r| r.precommit.as_deref())
            .and_then(|id| Oid::from_str(id).ok())
            .filter(|id| Some(*id) == head_id);
        let saved = match &record {
            Some(_) => self.read_state_file(INDEX_COPY)?,
            None => None,
        };
        match (saved, precommit) {
            (Some(bytes), _) => self.replace_index(Some(&bytes))?,
            (None, Some(_)) => {
                let before = record.as_ref().and_then(|r| r.head.as_deref());
                match before.map(Oid::from_str).transpose()? {
                    Some(before) => {
                        let mut index = self.git.index()?;
                        index.read_tree(&self.git.find_commit(before)?.tree()?)?;
                        index.write()?;
                    }
                    None => self.replace_index(None)?,
                }
            }
            (None, None) => {
                let mut index = self.git.index()?;
                index.read(true)?;
                for path in &reset {
                    let relative = Path::new(OsStr::from_bytes(path));
                    match index.remove_path(relative) {
                        Err(e) if e.code() != ErrorCode::NotFound => return Err(e.into()),
                        _ => {}
                    }
                    if let Some(&(id, mode)) = head_files.get(path) {
                        index.add(&entry(path, id, mode))?;
                    }
                }
                index.write()?;
            }
        }
        if let Some(precommit) = precommit {
            let branch = self.branch()?;
            let before = record
                .as_ref()
                .and_then(|r| r.head.as_deref())
                .map(Oid::from_str)
                .transpose()?;
            self.move_branch_back(&branch, before, precommit)?;
        }
        self.git.cleanup_state()?;
        self.forget_merge()?;
        Ok(reset.iter().map(|path| lossy(path)).collect())
    }

    /// Deletes the app's merge record (a merge finished or aborted with git leaves one behind).
    pub(crate) fn forget_merge(&self) -> Result<()> {
        self.remove_state_file(RECORD)?;
        self.remove_state_file(INDEX_COPY)
    }

    fn merge_remote(&self, branch: &str) -> Result<()> {
        let reference = self
            .git
            .find_reference(&format!("refs/remotes/{REMOTE}/{branch}"))?;
        let remote = self.git.reference_to_annotated_commit(&reference)?;
        let label = format!("{REMOTE}/{branch}");
        let mut checkout = CheckoutBuilder::new();
        // All or nothing: if any file is in the way (an ignored one, say), nothing is written.
        checkout
            .safe()
            .overwrite_ignored(false)
            .conflict_style_merge(true)
            .our_label("HEAD")
            .their_label(&label);
        self.git.merge(
            &[&remote],
            Some(&mut MergeOptions::new()),
            Some(&mut checkout),
        )?;
        Ok(())
    }

    /// Sets the branch back from `from` to `to`, or makes it unborn again for `None`.
    fn move_branch_back(&self, branch: &str, to: Option<Oid>, from: Oid) -> Result<()> {
        let name = format!("refs/heads/{branch}");
        match to {
            Some(to) => {
                self.git
                    .reference_matching(&name, to, true, from, "mdgitjournal: abort merge")?;
            }
            None => self.git.find_reference(&name)?.delete()?,
        }
        Ok(())
    }

    fn merge_heads(&self) -> Result<Vec<Oid>> {
        let Some(bytes) = read_if_exists(&self.git.path().join("MERGE_HEAD"))? else {
            return Ok(Vec::new());
        };
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| Ok(Oid::from_str(line.trim())?))
            .collect()
    }

    /// The app's record of the merge in progress, if it describes this merge.
    fn merge_record(&self) -> Result<Option<Record>> {
        let Some(bytes) = self.read_state_file(RECORD)? else {
            return Ok(None);
        };
        let Ok(record) = serde_json::from_slice::<Record>(&bytes) else {
            return Ok(None);
        };
        let head = head_commit(&self.git)?.map(|commit| commit.id().to_string());
        let current = record.version == VERSION
            && self.merge_heads()?.first().map(Oid::to_string).as_ref() == Some(&record.merge_head)
            && head.is_some()
            && head.as_ref() == record.precommit.as_ref().or(record.head.as_ref());
        Ok(current.then_some(record))
    }
}
