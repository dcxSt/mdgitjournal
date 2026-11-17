//! Three-way merges that never lose text. A conflict keeps the remote version at its path and moves
//! the local version to a conflict copy beside it; a file deleted on one side and changed on the
//! other keeps the changed version.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use git2::{Index, IndexEntry, MergeOptions, Repository, Tree};

use crate::repo::lossy;
use crate::worktree::{files, Files, STAGE_MASK};
use crate::{Conflict, Result};

pub(crate) struct Merge {
    /// The result, free of conflicts.
    pub index: Index,
    pub conflicts: Vec<Conflict>,
}

/// Merges `local` and `remote`, which both descend from `base`. Merged objects are written through
/// `repo`. Conflict copies avoid every path in the merge and any path `taken` reports.
pub(crate) fn merge_keep_both(
    repo: &Repository,
    base: &Tree<'_>,
    local: &Tree<'_>,
    remote: &Tree<'_>,
    stamp: &str,
    taken: impl Fn(&[u8]) -> bool,
) -> Result<Merge> {
    let mut index = repo.merge_trees(base, local, remote, Some(&MergeOptions::new()))?;
    let found = index.conflicts()?.collect::<Result<Vec<_>, _>>()?;
    let mut occupied: BTreeSet<Vec<u8>> = index.iter().map(|entry| entry.path).collect();
    let conflicted: BTreeSet<&[u8]> = found
        .iter()
        .flat_map(|c| [&c.ancestor, &c.our, &c.their])
        .flatten()
        .map(|entry| &entry.path[..])
        .collect();
    for path in conflicted {
        index.conflict_remove(Path::new(OsStr::from_bytes(path)))?;
    }

    let mut conflicts = Vec::new();
    for conflict in found {
        match (conflict.our, conflict.their) {
            (Some(ours), Some(theirs)) => {
                let path = lossy(&theirs.path);
                let same = ours.id == theirs.id;
                add(&mut index, theirs)?;
                if !same {
                    let copy = copy_path(&ours.path, stamp, |p| occupied.contains(p) || taken(p));
                    occupied.insert(copy.clone());
                    conflicts.push(Conflict {
                        path,
                        copy: Some(lossy(&copy)),
                    });
                    add(&mut index, IndexEntry { path: copy, ..ours })?;
                }
            }
            // Deleted remotely, changed locally: the local file simply stays.
            (Some(ours), None) => add(&mut index, ours)?,
            (None, Some(theirs)) => {
                conflicts.push(Conflict {
                    path: lossy(&theirs.path),
                    copy: None,
                });
                add(&mut index, theirs)?;
            }
            (None, None) => {}
        }
    }
    Ok(Merge { index, conflicts })
}

fn add(index: &mut Index, mut entry: IndexEntry) -> Result<()> {
    entry.flags &= !STAGE_MASK;
    Ok(index.add(&entry)?)
}

/// The stage-0 files of a tree.
pub(crate) fn tree_files(tree: &Tree<'_>) -> Result<Files> {
    let mut index = Index::new()?;
    index.read_tree(tree)?;
    Ok(files(&index))
}

/// `dir/name.md` -> `dir/name.conflict-<stamp>.md`, else `…-<stamp>-2.md`, `-3` … whichever is
/// free first. Names without an extension get the suffix at the end.
pub(crate) fn copy_path(path: &[u8], stamp: &str, taken: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let name_start = path.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    let (dir, name) = path.split_at(name_start);
    let (stem, ext) = match name.iter().rposition(|&b| b == b'.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, &b""[..]),
    };
    let mut n = 1;
    loop {
        let suffix = if n == 1 {
            format!(".conflict-{stamp}")
        } else {
            format!(".conflict-{stamp}-{n}")
        };
        let candidate = [dir, stem, suffix.as_bytes(), ext].concat();
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::copy_path;

    fn copy(path: &str, taken: &[&str]) -> String {
        let taken = |p: &[u8]| taken.iter().any(|t| t.as_bytes() == p);
        String::from_utf8(copy_path(path.as_bytes(), "1402", taken)).unwrap()
    }

    #[test]
    fn conflict_copies_sit_next_to_the_original() {
        assert_eq!(
            copy("2026-11-16-1402-run.md", &[]),
            "2026-11-16-1402-run.conflict-1402.md"
        );
        assert_eq!(copy("assets/a-1.jpg", &[]), "assets/a-1.conflict-1402.jpg");
        assert_eq!(copy("notes", &[]), "notes.conflict-1402");
        assert_eq!(copy(".gitignore", &[]), ".gitignore.conflict-1402");
        assert_eq!(copy("a.b/c", &[]), "a.b/c.conflict-1402");
    }

    #[test]
    fn conflict_copies_never_reuse_a_name() {
        assert_eq!(
            copy("x.md", &["x.conflict-1402.md", "x.conflict-1402-2.md"]),
            "x.conflict-1402-3.md"
        );
    }
}
