//! One level of Undo for applied pulls. The record is `.git/mdgitjournal/undo.json` (plus a copy
//! of the index from before the pull, `undo-index`), and `refs/mdgitjournal/undo` names a commit
//! holding the files the pull replaced, byte for byte as they were on disk, so `git gc` keeps
//! them. The ref is never pushed.
//!
//! The record is written before the pull touches a file. A record whose branch never moved is a
//! pull that stopped part way (the app quit during it); the next operation puts it back.

use std::collections::{BTreeMap, BTreeSet};

use git2::{ErrorCode, Index, Oid, Signature};
use serde::{Deserialize, Serialize};

use crate::pull::{Analysis, SyncSummary};
use crate::repo::{head_commit, lossy};
use crate::worktree::{self, build_tree, files, scratch, tree_files, Files, Plan, Snapshot};
use crate::{Error, Repo, Result};

const RECORD: &str = "undo.json";
const INDEX_COPY: &str = "undo-index";
const UNDO_REF: &str = "refs/mdgitjournal/undo";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    version: u32,
    branch: String,
    /// The branch before the pull; `None` if it was unborn.
    before: Option<String>,
    after: String,
    remote: String,
    /// Every path the pull wrote or removed.
    files: BTreeMap<String, RecordFile>,
    summary: SyncSummary,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordFile {
    /// The file as git saw it (after its filters), to recognise it.
    before: Option<String>,
    before_mode: Option<u32>,
    /// Its exact bytes on disk, which Undo writes back (kept in the undo ref's tree).
    before_raw: Option<String>,
    after: Option<String>,
}

impl RecordFile {
    fn raw(&self) -> Result<Option<(Oid, u32)>> {
        match (self.before_raw.as_deref(), self.before_mode) {
            (Some(id), Some(mode)) => Ok(Some((Oid::from_str(id)?, mode))),
            _ => Ok(None),
        }
    }
}

/// The undo record a new pull replaces, held while the pull runs so a pull that stops part way
/// can put it back.
pub(crate) struct Previous {
    record: Option<Vec<u8>>,
    index: Option<Vec<u8>>,
    reference: Option<Oid>,
}

impl Repo {
    /// The pull `undo_last_sync` would reverse; `None` once the branch has moved on from it.
    pub fn last_sync(&self) -> Result<Option<SyncSummary>> {
        let Some(record) = self.undo_record()? else {
            return Ok(None);
        };
        let tip = head_commit(&self.git)?.map(|commit| commit.id().to_string());
        let current = self.branch_name()?.as_deref() == Some(record.branch.as_str())
            && tip.as_deref() == Some(record.after.as_str());
        Ok(current.then_some(record.summary))
    }

    /// Puts back exactly what was there before the last pull: the branch, the index and every
    /// file the pull touched. Refuses, changing nothing, if that would lose something: a file
    /// edited since, something staged since, a commit since, or a pull that has been pushed.
    pub fn undo_last_sync(&self) -> Result<Vec<String>> {
        self.locked()?;
        let record = self
            .undo_record()?
            .ok_or_else(|| Error::Invalid("There is no pull to undo.".into()))?;
        let oid = |id: &str| Oid::from_str(id).map_err(Error::from);
        let after = oid(&record.after)?;
        let remote = oid(&record.remote)?;
        let before = record.before.as_deref().map(oid).transpose()?;

        let branch = self.branch()?;
        if branch != record.branch {
            return Err(Error::UndoRefused(format!(
                "You've switched from {} to {branch} since the pull, so it can't be undone.",
                record.branch
            )));
        }
        if head_commit(&self.git)?.map(|commit| commit.id()) != Some(after) {
            return Err(Error::UndoRefused(
                "You've committed since the pull, so it can't be undone.".into(),
            ));
        }
        if after != remote {
            if let Some(origin) = self.remote_branch(&branch)? {
                if origin == after || self.git.graph_descendant_of(origin, after)? {
                    return Err(Error::UndoRefused(
                        "The pull has been pushed since, so it can't be undone.".into(),
                    ));
                }
            }
        }

        let edited = self.edited_since(&record)?;
        if !edited.is_empty() {
            return Err(Error::UndoRefused(format!(
                "You've changed {} since the pull; Undo would overwrite {}.",
                list(&edited),
                if edited.len() == 1 { "it" } else { "them" }
            )));
        }
        let staged = self.staged_since(before, after)?;
        if !staged.is_empty() {
            return Err(Error::UndoRefused(format!(
                "You've staged {} with git since the pull; Undo would unstage {}.",
                list(&staged),
                if staged.len() == 1 { "it" } else { "them" }
            )));
        }

        self.restore_files(&record)?;
        self.restore_index(before)?;
        let name = format!("refs/heads/{branch}");
        match before {
            Some(before) => {
                self.git.reference_matching(
                    &name,
                    before,
                    true,
                    after,
                    "mdgitjournal: undo pull",
                )?;
            }
            None => self.git.find_reference(&name)?.delete()?,
        }
        self.clear_undo()?;
        Ok(record.files.into_keys().collect())
    }

    /// A pull that stopped part way (the app quit after writing files, before moving the branch)
    /// is put back: the files it wrote and the index, as if it had never run. Run before every
    /// locked operation. Refuses, changing nothing, if a file it wrote has been edited since.
    pub(crate) fn recover_interrupted_pull(&self) -> Result<()> {
        let Some(record) = self.undo_record()? else {
            return Ok(());
        };
        let tip = head_commit(&self.git)?.map(|commit| commit.id().to_string());
        let interrupted = self.branch_name()?.as_deref() == Some(record.branch.as_str())
            && tip == record.before
            && record.before.as_deref() != Some(record.after.as_str());
        if !interrupted {
            return Ok(());
        }
        let edited = self.edited_since(&record)?;
        if !edited.is_empty() {
            return Err(Error::Git(format!(
                "a pull stopped part way, and {} changed since; the versions from before the \
                 pull are in refs/mdgitjournal/undo. Sort it out with git, then delete \
                 .git/mdgitjournal/undo.json",
                list(&edited)
            )));
        }
        let before = record.before.as_deref().map(Oid::from_str).transpose()?;
        self.restore_files(&record)?;
        self.restore_index(before)?;
        self.clear_undo()
    }

    /// The recorded files now neither as the pull left them nor as they were before it.
    fn edited_since<'r>(&self, record: &'r Record) -> Result<Vec<&'r str>> {
        let hasher = scratch(&self.git)?;
        let mut edited = Vec::new();
        for (path, file) in &record.files {
            let was = file.before.as_deref().map(Oid::from_str).transpose()?;
            let left = file.after.as_deref().map(Oid::from_str).transpose()?;
            let now = worktree::on_disk(&hasher, path.as_bytes())?;
            if now != left && now != was {
                edited.push(path.as_str());
            }
        }
        Ok(edited)
    }

    /// Index entries changed since a pull from `before` to `after`: where the index differs from
    /// what the pull left (the new head's entry where the old index followed the old head, else
    /// the entry staged with git, which the pull kept).
    fn staged_since(&self, before: Option<Oid>, after: Oid) -> Result<Vec<String>> {
        let open = |path: std::path::PathBuf| -> Result<Files> {
            match path.exists() {
                true => Ok(files(&Index::open(&path)?)),
                false => Ok(Files::new()),
            }
        };
        let old = open(self.state_file(INDEX_COPY))?;
        let now = open(self.git.path().join("index"))?;
        let ours = match before {
            Some(id) => tree_files(&self.git.find_commit(id)?.tree()?)?,
            None => Files::new(),
        };
        let new = tree_files(&self.git.find_commit(after)?.tree()?)?;
        let paths: BTreeSet<&Vec<u8>> = old
            .keys()
            .chain(now.keys())
            .chain(ours.keys())
            .chain(new.keys())
            .collect();
        Ok(paths
            .into_iter()
            .filter(|path| {
                let left = match old.get(*path) == ours.get(*path) {
                    true => new.get(*path),
                    false => old.get(*path),
                };
                now.get(*path) != left
            })
            .map(|path| lossy(path))
            .collect())
    }

    /// Every recorded file back to its exact bytes from before the pull, all or nothing: if one
    /// can't be written, those already put back return to how they were, and nothing changed.
    fn restore_files(&self, record: &Record) -> Result<()> {
        let hasher = scratch(&self.git)?;
        let mut restore = Vec::new();
        for (path, file) in &record.files {
            restore.push((
                path.as_str(),
                file.raw()?,
                file.before.as_deref().map(Oid::from_str).transpose()?,
            ));
        }
        // Removals first, and each file compared again right before it is written: on a
        // case-insensitive file system a removed name can be the same file as a restored one.
        restore.sort_by_key(|(_, raw, _)| raw.is_some());
        let mut done: Vec<(&str, Option<(Oid, u32)>)> = Vec::new();
        for &(path, raw, was) in &restore {
            if raw.is_some() && worktree::on_disk(&hasher, path.as_bytes())? == was {
                // Already as it was (to git), so its bytes are the user's own.
                continue;
            }
            let written = worktree::capture_path(&hasher, path.as_bytes()).and_then(|now| {
                worktree::restore(&self.git, path.as_bytes(), raw)?;
                Ok(now)
            });
            match written {
                Ok(now) => done.push((path, now)),
                Err(error) => {
                    let mut back = Ok(());
                    for &(path, now) in done.iter().rev() {
                        back = back.and(worktree::restore(&hasher, path.as_bytes(), now));
                    }
                    return Err(match back {
                        Ok(()) => Error::Io(format!(
                            "Undo couldn't write \"{path}\" ({error}); nothing was changed."
                        )),
                        Err(also) => Error::Io(format!(
                            "Undo couldn't write \"{path}\" ({error}), and putting the files back \
                             failed too: {also}"
                        )),
                    });
                }
            }
        }
        Ok(())
    }

    /// The index from before the pull, byte for byte (or the old head's tree without a copy).
    fn restore_index(&self, before: Option<Oid>) -> Result<()> {
        match (self.read_state_file(INDEX_COPY)?, before) {
            (Some(bytes), _) => self.replace_index(Some(&bytes)),
            (None, Some(before)) => {
                let mut index = self.git.index()?;
                index.read_tree(&self.git.find_commit(before)?.tree()?)?;
                Ok(index.write()?)
            }
            (None, None) => self.replace_index(None),
        }
    }

    /// Forgets the undo record (after a commit, push, Resolve with git or disconnect).
    pub(crate) fn clear_undo(&self) -> Result<()> {
        self.remove_state_file(RECORD)?;
        self.remove_state_file(INDEX_COPY)?;
        match self.git.find_reference(UNDO_REF) {
            Ok(mut reference) => Ok(reference.delete()?),
            Err(e) if e.code() == ErrorCode::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub(crate) fn undo_backup(&self) -> Result<Previous> {
        Ok(Previous {
            record: self.read_state_file(RECORD)?,
            index: self.read_state_file(INDEX_COPY)?,
            reference: match self.git.refname_to_id(UNDO_REF) {
                Ok(id) => Some(id),
                Err(e) if e.code() == ErrorCode::NotFound => None,
                Err(e) => return Err(e.into()),
            },
        })
    }

    pub(crate) fn undo_restore(&self, previous: Previous) -> Result<()> {
        self.clear_undo()?;
        if let Some(id) = previous.reference {
            self.git
                .reference(UNDO_REF, id, true, "mdgitjournal: keep the previous undo")?;
        }
        if let Some(index) = previous.index {
            self.write_state_file(INDEX_COPY, &index)?;
        }
        if let Some(record) = previous.record {
            self.write_state_file(RECORD, &record)?;
        }
        Ok(())
    }

    /// Replaces the undo record with one for the pull about to run: what every path in `plan`
    /// holds now, the index as it is, and the branch before and after.
    pub(crate) fn record_pull(
        &self,
        snapshot: &Snapshot,
        analysis: &Analysis<'_>,
        after: Oid,
        plan: &Plan,
        summary: &SyncSummary,
        signature: &Signature<'_>,
    ) -> Result<()> {
        self.clear_undo()?;
        let mut kept = Files::new();
        let mut files = BTreeMap::new();
        for step in &plan.steps {
            // The exact bytes, for files; a submodule is never written, so it has none.
            let raw = step.before.and(step.raw);
            if let Some((id, mode)) = raw {
                self.persist(snapshot, id)?;
                kept.insert(step.path.clone(), (id, mode));
            }
            let path = String::from_utf8(step.path.clone())
                .map_err(|_| Error::Git("a file name is not UTF-8".into()))?;
            files.insert(
                path,
                RecordFile {
                    before: step.before.map(|(id, _)| id.to_string()),
                    before_mode: raw.map(|(_, mode)| mode),
                    before_raw: raw.map(|(id, _)| id.to_string()),
                    after: step.after.map(|(id, _)| id.to_string()),
                },
            );
        }
        let tree = self.git.find_tree(build_tree(&self.git, &kept)?)?;
        let parent = analysis
            .local
            .map(|id| self.git.find_commit(id))
            .transpose()?;
        let parents: Vec<_> = parent.iter().collect();
        let keeper = self.git.commit(
            None,
            signature,
            signature,
            "mdgitjournal: before pull (kept for Undo)\n",
            &tree,
            &parents,
        )?;
        self.git.reference(
            UNDO_REF,
            keeper,
            true,
            "mdgitjournal: keep the files a pull replaces",
        )?;
        if let Some(index) = self.index_bytes()? {
            self.write_state_file(INDEX_COPY, &index)?;
        }
        let record = Record {
            version: VERSION,
            branch: analysis.branch.clone(),
            before: analysis.local.map(|id| id.to_string()),
            after: after.to_string(),
            remote: analysis.remote.to_string(),
            files,
            summary: summary.clone(),
        };
        let json = serde_json::to_vec_pretty(&record)
            .map_err(|e| Error::Io(format!("cannot write the undo record: {e}")))?;
        self.write_state_file(RECORD, &json)
    }

    /// The record, if there is one this version understands.
    fn undo_record(&self) -> Result<Option<Record>> {
        let Some(bytes) = self.read_state_file(RECORD)? else {
            return Ok(None);
        };
        Ok(serde_json::from_slice::<Record>(&bytes)
            .ok()
            .filter(|record| record.version == VERSION))
    }
}

/// "a", "a and b", "a, b and c".
pub(crate) fn list(paths: &[impl AsRef<str>]) -> String {
    let quoted: Vec<String> = paths
        .iter()
        .map(|p| format!("\"{}\"", p.as_ref()))
        .collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::list;

    #[test]
    fn file_lists_read_naturally() {
        assert_eq!(list(&["a.md"]), "\"a.md\"");
        assert_eq!(list(&["a.md", "b.md"]), "\"a.md\" and \"b.md\"");
        assert_eq!(
            list(&["a.md", "b.md", "c.md"]),
            "\"a.md\", \"b.md\" and \"c.md\""
        );
    }
}
