//! The working tree as git sees it: an in-memory snapshot, and writing a merge result back to disk.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use git2::{FileMode, Index, IndexAddOption, Oid, Repository};

use crate::merge::copy_path;
use crate::repo::{lossy, workdir};
use crate::{Conflict, Error, Result};

/// Repository path -> (blob id, file mode).
pub(crate) type Files = BTreeMap<Vec<u8>, (Oid, u32)>;

/// Written in the git directory and renamed into place, so an interrupted write never leaves a
/// stray file in the journal.
const TEMP_FILE: &str = "mdgitjournal-write.tmp";
const MEMPACK_PRIORITY: i32 = 1000;
/// Submodules are recorded, never written; git leaves them alone on checkout too.
const GITLINK: u32 = 0o160000;
/// Symbolic links are never written: a link from the remote could point the journal anywhere.
const LINK: u32 = 0o120000;
/// The merge stage bits of an index entry's flags; 0 for a resolved entry.
pub(crate) const STAGE_MASK: u16 = 0x3000;

/// The working tree (tracked and untracked files, minus ignored ones) as a tree.
pub(crate) struct Snapshot {
    /// A separate handle whose new objects stay in memory: snapshots never write to the real index
    /// or object store. Merges with the snapshot must run on this handle.
    pub repo: Repository,
    pub tree: Oid,
    pub files: Files,
}

impl Snapshot {
    pub fn take(git: &Repository) -> Result<Snapshot> {
        let repo = Repository::open(workdir(git)?)?;
        repo.odb()?.add_new_mempack_backend(MEMPACK_PRIORITY)?;
        // Starts from the on-disk index so its stat cache spares rehashing unchanged files; the
        // index is never written back.
        let mut index = repo.index()?;
        index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
        index.update_all(["*"], None)?;
        let tree = index.write_tree()?;
        let files = files(&index);
        Ok(Snapshot { repo, tree, files })
    }
}

/// Stage-0 entries of `index`.
pub(crate) fn files(index: &Index) -> Files {
    index
        .iter()
        .filter(|entry| entry.flags & STAGE_MASK == 0)
        .map(|entry| (entry.path, (entry.id, entry.mode)))
        .collect()
}

/// Makes the working tree match `target`, given that it matched `snapshot.files` when the snapshot
/// was taken. Only paths that differ are touched. A file whose content changed since the snapshot is
/// never overwritten or removed: it moves to a conflict copy first, or stays in place.
/// Returns every path written, moved or removed.
pub(crate) fn apply(
    snapshot: &Snapshot,
    target: &Files,
    stamp: &str,
    conflicts: &mut Vec<Conflict>,
) -> Result<Vec<String>> {
    let repo = &snapshot.repo;
    let current = &snapshot.files;
    let workdir = workdir(repo)?;

    // Every path is checked before the first change, so an unsafe path or a folder in the way
    // aborts with the working tree untouched.
    no_case_collisions(target.keys())?;
    let mut writes = Vec::new();
    for (path, &(id, mode)) in target {
        if mode != GITLINK && current.get(path) != Some(&(id, mode)) {
            if mode == LINK {
                return Err(Error::Git(format!(
                    "the remote has a symbolic link at \"{}\"; mdgitjournal does not write links. \
                     Remove it from the repository with git and pull again",
                    lossy(path)
                )));
            }
            let full = checked_path(workdir, path)?;
            not_a_folder(&full)?;
            writes.push((path, full, id, mode));
        }
    }
    let mut removals = Vec::new();
    for (path, &(id, mode)) in current {
        if mode != GITLINK && !target.contains_key(path) {
            let full = checked_path(workdir, path)?;
            not_a_folder(&full)?;
            removals.push((path, full, id));
        }
    }

    // New paths first: conflict copies must be on disk before the files they preserve change.
    writes.sort_by_key(|(path, ..)| current.contains_key(*path));
    let mut touched = Vec::new();
    let mut written = HashSet::new();
    for (path, full, id, mode) in writes {
        if differs_on_disk(repo, &full, current.get(path).map(|&(id, _)| id))? {
            let copy = copy_path(path, stamp, |p| {
                target.contains_key(p) || current.contains_key(p) || exists(workdir, p)
            });
            checked_path(workdir, path)?;
            make_parents(workdir, &copy)?;
            fs::rename(&full, workdir.join(OsStr::from_bytes(&copy)))?;
            touched.push(lossy(&copy));
            conflicts.push(Conflict {
                path: lossy(path),
                copy: Some(lossy(&copy)),
            });
        }
        make_parents(workdir, path)?;
        write_blob(repo, &full, id, mode)?;
        written.insert(file_id(&fs::symlink_metadata(&full)?));
        touched.push(lossy(path));
    }
    for (path, full, id) in removals {
        let Some(meta) = metadata(&full)? else {
            continue;
        };
        // On a case-insensitive file system the old name can reach a file just written under
        // another spelling.
        if !written.contains(&file_id(&meta)) && !differs_on_disk(repo, &full, Some(id))? {
            checked_path(workdir, path)?;
            remove_file(workdir, &full)?;
            touched.push(lossy(path));
        }
    }

    touched.sort();
    touched.dedup();
    Ok(touched)
}

/// Whether a file exists at `full` with content other than `expected` (`None`: no file expected),
/// i.e. whether replacing or removing it could lose something.
fn differs_on_disk(repo: &Repository, full: &Path, expected: Option<Oid>) -> Result<bool> {
    match metadata(full)? {
        None => Ok(false),
        // Hashed through the snapshot handle, so filters match the snapshot and nothing is stored.
        Some(_) => Ok(expected != Some(repo.blob_path(full)?)),
    }
}

fn metadata(full: &Path) -> Result<Option<fs::Metadata>> {
    match full.symlink_metadata() {
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn not_a_folder(full: &Path) -> Result<()> {
    match metadata(full)? {
        Some(meta) if meta.is_dir() => Err(Error::Io(format!(
            "{} is a folder; move it out of the way and try again",
            full.display()
        ))),
        _ => Ok(()),
    }
}

fn file_id(meta: &fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

pub(crate) fn exists(workdir: &Path, path: &[u8]) -> bool {
    workdir
        .join(OsStr::from_bytes(path))
        .symlink_metadata()
        .is_ok()
}

/// `workdir`/`path`, refusing paths that could escape the working tree or reach into .git: `..`,
/// `.git`, and parents that are symlinks or files.
fn checked_path(workdir: &Path, path: &[u8]) -> Result<PathBuf> {
    let unsafe_path = || Error::Git(format!("refusing to write unsafe path \"{}\"", lossy(path)));
    let parts: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
    let bad = |part: &&[u8]| {
        part.is_empty() || *part == b"." || *part == b".." || part.eq_ignore_ascii_case(b".git")
    };
    let Some((name, parents)) = parts.split_last().filter(|_| !parts.iter().any(bad)) else {
        return Err(unsafe_path());
    };
    let mut full = workdir.to_path_buf();
    for part in parents {
        full.push(OsStr::from_bytes(part));
        match metadata(&full)? {
            Some(meta) if !meta.is_dir() => {
                return Err(Error::Io(format!(
                    "cannot write \"{}\": {} is not a folder",
                    lossy(path),
                    full.display()
                )))
            }
            _ => {}
        }
    }
    full.push(OsStr::from_bytes(name));
    Ok(full)
}

/// Creates the missing folders above `path`, checking each one as it goes, so a folder never
/// resolves through a symlink, even one that appeared after planning.
fn make_parents(workdir: &Path, path: &[u8]) -> Result<()> {
    let mut full = workdir.to_path_buf();
    let parts: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        full.push(OsStr::from_bytes(part));
        match fs::create_dir(&full) {
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e.into()),
            _ => {}
        }
        if !full.symlink_metadata()?.is_dir() {
            return Err(Error::Io(format!(
                "cannot write \"{}\": {} is not a folder",
                lossy(path),
                full.display()
            )));
        }
    }
    Ok(())
}

/// Refuses paths that name the same file on a case-insensitive file system (APFS's default), such
/// as `A` and `a/x.md`: writing one could go through the other.
fn no_case_collisions<'a>(paths: impl Iterator<Item = &'a Vec<u8>>) -> Result<()> {
    let mut seen: HashMap<String, &[u8]> = HashMap::new();
    for path in paths {
        let ends = path
            .iter()
            .enumerate()
            .filter(|&(_, &b)| b == b'/')
            .map(|(i, _)| i);
        for end in ends.chain([path.len()]) {
            let prefix = &path[..end];
            let folded = String::from_utf8_lossy(prefix).to_lowercase();
            match seen.get(folded.as_str()) {
                Some(other) if *other != prefix => {
                    return Err(Error::Git(format!(
                        "the remote has both \"{}\" and \"{}\", which are the same name on this \
                         device; rename one of them with git and pull again",
                        lossy(other),
                        lossy(prefix)
                    )))
                }
                Some(_) => {}
                None => {
                    seen.insert(folded, prefix);
                }
            }
        }
    }
    Ok(())
}

/// Atomically replaces `full` with the blob's bytes, exactly as stored.
fn write_blob(repo: &Repository, full: &Path, id: Oid, mode: u32) -> Result<()> {
    let blob = repo.find_blob(id)?;
    let temp = repo.path().join(TEMP_FILE);
    match fs::remove_file(&temp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let permissions = if mode == u32::from(FileMode::Blob) {
        0o666
    } else if mode == u32::from(FileMode::BlobExecutable) {
        0o777
    } else {
        return Err(Error::Git(format!(
            "{} has an unsupported file type (mode {mode:o})",
            full.display()
        )));
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(permissions)
        .open(&temp)?;
    file.write_all(blob.content())?;
    file.sync_all()?;
    fs::rename(&temp, full)?;
    Ok(())
}

/// Removes a file, then any folders it leaves empty (as git does).
fn remove_file(workdir: &Path, full: &Path) -> Result<()> {
    fs::remove_file(full)?;
    let mut dir = full.parent();
    while let Some(d) = dir.filter(|d| *d != workdir) {
        if fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        (dir, repo)
    }

    #[test]
    fn snapshot_stores_nothing() {
        let (dir, repo) = setup();
        fs::write(dir.path().join("a.md"), "untracked text\n").unwrap();
        let snapshot = Snapshot::take(&repo).unwrap();
        let (id, _) = snapshot.files[&b"a.md".to_vec()];
        assert!(snapshot.repo.find_blob(id).is_ok());
        assert!(Repository::open(dir.path()).unwrap().find_blob(id).is_err());
        assert!(!dir.path().join(".git/index").exists());
    }

    #[test]
    fn files_edited_after_the_snapshot_are_kept() {
        let (dir, repo) = setup();
        let path = |name: &str| dir.path().join(name);
        fs::write(path("a.md"), "before\n").unwrap();
        fs::write(path("b.md"), "before\n").unwrap();
        let snapshot = Snapshot::take(&repo).unwrap();
        fs::write(path("a.md"), "typed during the pull\n").unwrap();
        fs::write(path("b.md"), "typed during the pull\n").unwrap();

        // The target replaces a.md and drops b.md.
        let remote = snapshot.repo.blob(b"remote\n").unwrap();
        let target = Files::from([(b"a.md".to_vec(), (remote, u32::from(FileMode::Blob)))]);
        let mut conflicts = Vec::new();
        let touched = apply(&snapshot, &target, "0930", &mut conflicts).unwrap();

        assert_eq!(fs::read_to_string(path("a.md")).unwrap(), "remote\n");
        assert_eq!(
            fs::read_to_string(path("a.conflict-0930.md")).unwrap(),
            "typed during the pull\n"
        );
        assert_eq!(
            fs::read_to_string(path("b.md")).unwrap(),
            "typed during the pull\n"
        );
        assert_eq!(touched, ["a.conflict-0930.md", "a.md"]);
        assert_eq!(
            conflicts,
            [Conflict {
                path: "a.md".into(),
                copy: Some("a.conflict-0930.md".into())
            }]
        );
    }

    #[test]
    fn unsafe_paths_are_refused_before_anything_is_written() {
        let (dir, repo) = setup();
        let snapshot = Snapshot::take(&repo).unwrap();
        let blob = snapshot.repo.blob(b"x").unwrap();
        for bad in [&b"../escape.md"[..], b".git/config", b"a//b.md", b"./a.md"] {
            let target = Files::from([
                (b"aaa-first.md".to_vec(), (blob, u32::from(FileMode::Blob))),
                (bad.to_vec(), (blob, u32::from(FileMode::Blob))),
            ]);
            assert!(apply(&snapshot, &target, "0000", &mut Vec::new()).is_err());
            assert!(!dir.path().join("aaa-first.md").exists());
        }
    }

    #[test]
    fn remote_symlinks_are_refused_before_anything_is_written() {
        let (dir, repo) = setup();
        let snapshot = Snapshot::take(&repo).unwrap();
        let link = snapshot.repo.blob(b"../outside").unwrap();
        let blob = snapshot.repo.blob(b"x").unwrap();
        let target = Files::from([
            (b"aaa-first.md".to_vec(), (blob, u32::from(FileMode::Blob))),
            (b"link".to_vec(), (link, LINK)),
        ]);
        let result = apply(&snapshot, &target, "0000", &mut Vec::new());
        assert!(
            matches!(&result, Err(Error::Git(m)) if m.contains("symbolic link")),
            "{result:?}"
        );
        assert!(!dir.path().join("aaa-first.md").exists());
        assert!(dir.path().join("link").symlink_metadata().is_err());
    }

    #[test]
    fn writes_never_go_through_a_symlinked_folder() {
        let (dir, repo) = setup();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("a")).unwrap();
        let snapshot = Snapshot::take(&repo).unwrap();
        let blob = snapshot.repo.blob(b"x").unwrap();
        let target = Files::from([(b"a/x.md".to_vec(), (blob, u32::from(FileMode::Blob)))]);
        assert!(apply(&snapshot, &target, "0000", &mut Vec::new()).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);

        // A link that appears after planning is caught when the folder is made.
        fs::remove_file(dir.path().join("a")).unwrap();
        let snapshot = Snapshot::take(&repo).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("a")).unwrap();
        assert!(make_parents(dir.path(), b"a/x.md").is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        drop(snapshot);
    }

    #[test]
    fn names_that_differ_only_in_case_are_refused() {
        let (dir, repo) = setup();
        let snapshot = Snapshot::take(&repo).unwrap();
        let blob = snapshot.repo.blob(b"x").unwrap();
        let target = Files::from([
            (b"A".to_vec(), (blob, u32::from(FileMode::Blob))),
            (b"a/x.md".to_vec(), (blob, u32::from(FileMode::Blob))),
            (b"aaa-first.md".to_vec(), (blob, u32::from(FileMode::Blob))),
        ]);
        assert!(apply(&snapshot, &target, "0000", &mut Vec::new()).is_err());
        assert!(!dir.path().join("aaa-first.md").exists());
        assert!(!dir.path().join("A").exists());
    }
}
