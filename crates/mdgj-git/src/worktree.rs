//! The working tree as git sees it (an in-memory snapshot), and the one careful way a pull replaces
//! or removes files in it.
//!
//! Files written from the repository get the bytes `git checkout` would write (the smudge filters
//! of `.gitattributes` and `core.autocrlf` applied); files put back as they were (Undo, a pull
//! rolled back) get their exact earlier bytes, which are kept as they were on disk.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use git2::build::CheckoutBuilder;
use git2::{
    AttrCheckFlags, AttrValue, Index, IndexAddOption, IndexEntry, IndexTime, ObjectType, Oid,
    Repository, Tree,
};

use crate::repo::{lossy, workdir};
use crate::{Error, Result};

/// Repository path -> (blob id, file mode).
pub(crate) type Files = BTreeMap<Vec<u8>, (Oid, u32)>;

/// Written in the git directory and renamed into place, so an interrupted write never leaves a
/// stray file in the journal.
const TEMP_FILE: &str = "mdgitjournal-write.tmp";
/// Where a blob is checked out to learn the bytes git would write for it, inside the git directory.
const CHECKOUT_DIR: &str = "mdgitjournal-checkout.tmp";
/// Attributes that make git change a file's bytes on checkout.
const FILTER_ATTRIBUTES: [&str; 6] = [
    "text",
    "eol",
    "crlf",
    "filter",
    "ident",
    "working-tree-encoding",
];
const MEMPACK_PRIORITY: i32 = 1000;
pub(crate) const BLOB: u32 = 0o100644;
pub(crate) const EXECUTABLE: u32 = 0o100755;
/// Symbolic links: a pull never writes one from the remote, as it could point the journal anywhere.
pub(crate) const LINK: u32 = 0o120000;
/// Submodules are recorded, never written; git leaves them alone on checkout too.
pub(crate) const GITLINK: u32 = 0o160000;
/// The merge stage bits of an index entry's flags; 0 for a resolved entry.
const STAGE_MASK: u16 = 0x3000;

/// The working tree (tracked and untracked files, minus ignored ones) as a tree.
pub(crate) struct Snapshot {
    /// A separate handle whose new objects stay in memory: snapshots, previews and the review's
    /// trial merges never write to the real index or object store.
    pub repo: Repository,
    pub tree: Oid,
    pub files: Files,
}

impl Snapshot {
    pub fn take(git: &Repository) -> Result<Snapshot> {
        let repo = scratch(git)?;
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

/// A second handle on the repository whose new objects (hashes of files, trial merges) stay in
/// memory.
pub(crate) fn scratch(git: &Repository) -> Result<Repository> {
    let repo = Repository::open(workdir(git)?)?;
    repo.odb()?.add_new_mempack_backend(MEMPACK_PRIORITY)?;
    Ok(repo)
}

/// Stage-0 entries of `index`.
pub(crate) fn files(index: &Index) -> Files {
    index
        .iter()
        .filter(|entry| entry.flags & STAGE_MASK == 0)
        .map(|entry| (entry.path, (entry.id, entry.mode)))
        .collect()
}

/// The files of a tree.
pub(crate) fn tree_files(tree: &Tree<'_>) -> Result<Files> {
    let mut index = Index::new()?;
    index.read_tree(tree)?;
    Ok(files(&index))
}

/// Writes the tree holding `files` (only the trees; the blobs must already exist) through `repo`.
pub(crate) fn build_tree(repo: &Repository, files: &Files) -> Result<Oid> {
    let mut index = Index::new()?;
    for (path, &(id, mode)) in files {
        index.add(&entry(path, id, mode))?;
    }
    Ok(index.write_tree_to(repo)?)
}

/// An index entry with no file-system data, so git rehashes the file before trusting it.
pub(crate) fn entry(path: &[u8], id: Oid, mode: u32) -> IndexEntry {
    IndexEntry {
        ctime: IndexTime::new(0, 0),
        mtime: IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode,
        uid: 0,
        gid: 0,
        file_size: 0,
        id,
        flags: 0,
        flags_extended: 0,
        path: path.to_vec(),
    }
}

pub(crate) fn regular(mode: u32) -> bool {
    mode == BLOB || mode == EXECUTABLE
}

/// One file a pull replaces or removes.
pub(crate) struct Step {
    pub path: Vec<u8>,
    full: PathBuf,
    /// The working tree's version (`None`: no file), as recorded for Undo.
    pub before: Option<(Oid, u32)>,
    /// The new version (`None`: removed).
    pub after: Option<(Oid, u32)>,
    /// What must be on disk right before the step. Usually `before`; for a new name that the file
    /// system folds onto an existing file (case-only renames on APFS), that file.
    expected: Option<(Oid, u32)>,
    /// The exact bytes on disk when planned (a blob in the snapshot's handle), which a rollback or
    /// Undo writes back; `before` is what git's filters make of them.
    pub raw: Option<(Oid, u32)>,
}

/// Every change that makes the working tree match a target, checked before anything is written.
pub(crate) struct Plan {
    pub steps: Vec<Step>,
}

/// Plans the changes from the snapshot's working tree to `target`. Refuses, with nothing written,
/// anything git would need to sort out: links from the remote, names that differ only in case, a
/// folder (or a symlinked or plain file) where a folder or file must go, an ignored file in the
/// way, paths that could leave the working tree, and names that aren't UTF-8.
pub(crate) fn plan(snapshot: &Snapshot, target: &Files) -> Result<Plan> {
    let current = &snapshot.files;
    let workdir = workdir(&snapshot.repo)?;
    no_case_collisions(target.keys())?;

    let mut removals = Vec::new();
    for (path, &entry) in current {
        if entry.1 != GITLINK && !target.contains_key(path) {
            let full = checked(workdir, path)?;
            removals.push(Step {
                raw: capture(&snapshot.repo, &full)?,
                full,
                path: path.clone(),
                before: Some(entry),
                after: None,
                expected: Some(entry),
            });
        }
    }
    let mut steps = Vec::new();
    for (path, &entry) in target {
        let before = current.get(path).copied();
        if entry.1 == GITLINK || before == Some(entry) {
            continue;
        }
        if entry.1 == LINK {
            return Err(Error::Git(format!(
                "the remote has a symbolic link at \"{}\"; mdgitjournal does not write links. \
                 Remove it from the repository with git and pull again",
                lossy(path)
            )));
        }
        let full = checked(workdir, path)?;
        let expected = match (before, metadata(&full)?) {
            (Some(_), _) | (None, None) => before,
            (None, Some(meta)) => {
                let alias = removals.iter().find(|removal| {
                    metadata(&removal.full)
                        .ok()
                        .flatten()
                        .is_some_and(|m| file_id(&m) == file_id(&meta))
                });
                match alias {
                    Some(removal) => removal.before,
                    None => {
                        return Err(Error::Git(format!(
                            "\"{}\" is on this device but ignored by git, and the remote has a \
                             file with that name; move it out of the way and try again",
                            lossy(path)
                        )))
                    }
                }
            }
        };
        steps.push(Step {
            path: path.clone(),
            raw: match expected {
                Some(_) => capture(&snapshot.repo, &full)?,
                None => None,
            },
            full,
            before,
            after: Some(entry),
            expected,
        });
    }
    steps.extend(removals);
    Ok(Plan { steps })
}

/// What happened to each step that ran.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Done {
    Wrote,
    Removed,
    /// A removal of a name that, on this file system, is the file just written under another.
    Skipped,
}

pub(crate) struct Failed {
    pub error: Error,
    pub done: Vec<Done>,
}

/// Runs the plan. Each file is compared with what the plan expects immediately before it is
/// replaced or removed; one that changed in the meantime stops everything with
/// `Error::StaleReview`, and `rollback` puts back what was done. `repo` must be the snapshot's
/// handle, which holds the new versions.
pub(crate) fn execute(repo: &Repository, plan: &Plan) -> Result<Vec<Done>, Failed> {
    let mut done = Vec::new();
    let mut written = HashSet::new();
    for step in &plan.steps {
        match run(repo, step, &mut written) {
            Ok(outcome) => done.push(outcome),
            Err(error) => return Err(Failed { error, done }),
        }
    }
    Ok(done)
}

fn run(repo: &Repository, step: &Step, written: &mut HashSet<(u64, u64)>) -> Result<Done> {
    let workdir = workdir(repo)?;
    let meta = metadata(&step.full)?;
    if step.after.is_none() && meta.as_ref().is_some_and(|m| written.contains(&file_id(m))) {
        return Ok(Done::Skipped);
    }
    // The folders on the way are checked again right before the change.
    checked_path(workdir, &step.path)?;
    let is_expected = |full: &Path| -> Result<bool> {
        let meta = metadata(full)?;
        Ok(hash(repo, full, meta.as_ref())? == step.expected.map(|(id, _)| id))
    };
    match step.after {
        Some((id, mode)) => {
            make_parents(workdir, &step.path)?;
            // The new version is on disk (and synced) before the old one is checked, so the check
            // and the replacement happen back to back.
            let bytes = checkout_bytes(repo, &step.path, id, mode)?;
            let temp = stage(repo, &step.full, &bytes, mode)?;
            if !is_expected(&step.full)? {
                fs::remove_file(&temp)?;
                return Err(Error::StaleReview);
            }
            fs::rename(&temp, &step.full)?;
            written.insert(file_id(&fs::symlink_metadata(&step.full)?));
            Ok(Done::Wrote)
        }
        None => {
            if !is_expected(&step.full)? {
                return Err(Error::StaleReview);
            }
            remove_file(workdir, &step.full)?;
            Ok(Done::Removed)
        }
    }
}

/// Undoes the steps that ran, newest first, each only if its file is still as the step left it:
/// the file's exact earlier bytes come back.
pub(crate) fn rollback(repo: &Repository, plan: &Plan, done: &[Done]) -> Result<()> {
    for (step, outcome) in plan.steps.iter().zip(done).rev() {
        let meta = metadata(&step.full)?;
        let untouched = match outcome {
            Done::Wrote => hash(repo, &step.full, meta.as_ref())? == step.after.map(|(id, _)| id),
            Done::Removed => meta.is_none(),
            Done::Skipped => false,
        };
        if untouched {
            restore(repo, &step.path, step.raw)?;
        }
    }
    Ok(())
}

/// The file at `full` exactly as it is on disk (a link's target, for a link), stored as a blob
/// through `repo` (a scratch handle keeps it in memory) with its mode; `None` if there is none.
pub(crate) fn capture(repo: &Repository, full: &Path) -> Result<Option<(Oid, u32)>> {
    match metadata(full)? {
        None => Ok(None),
        Some(meta) if meta.is_dir() => Err(Error::Io(format!(
            "{} is a folder; move it out of the way and try again",
            full.display()
        ))),
        Some(meta) if meta.file_type().is_symlink() => {
            let target = fs::read_link(full)?;
            Ok(Some((repo.blob(target.as_os_str().as_bytes())?, LINK)))
        }
        Some(meta) => {
            let mode = if meta.mode() & 0o111 != 0 {
                EXECUTABLE
            } else {
                BLOB
            };
            Ok(Some((repo.blob(&fs::read(full)?)?, mode)))
        }
    }
}

/// [`capture`] of a repository path.
pub(crate) fn capture_path(repo: &Repository, path: &[u8]) -> Result<Option<(Oid, u32)>> {
    capture(repo, &checked_path(workdir(repo)?, path)?)
}

/// The blob id of the file at `path` as git would store it (hashed through `repo`, so use a
/// scratch handle to store nothing); `None` if there is no file, the zero id for a folder.
pub(crate) fn on_disk(repo: &Repository, path: &[u8]) -> Result<Option<Oid>> {
    let full = checked_path(workdir(repo)?, path)?;
    let meta = metadata(&full)?;
    hash(repo, &full, meta.as_ref())
}

fn hash(repo: &Repository, full: &Path, meta: Option<&fs::Metadata>) -> Result<Option<Oid>> {
    match meta {
        None => Ok(None),
        Some(meta) if meta.is_dir() => Ok(Some(Oid::ZERO_SHA1)),
        // git stores a link as its target.
        Some(meta) if meta.file_type().is_symlink() => {
            let target = fs::read_link(full)?;
            Ok(Some(Oid::hash_object(
                ObjectType::Blob,
                target.as_os_str().as_bytes(),
            )?))
        }
        // Filters apply as in the snapshot.
        Some(_) => Ok(Some(repo.blob_path(full)?)),
    }
}

/// Makes the file at `path` what `git checkout` makes of `entry` (blob from `repo`), or removes it
/// for `None`. Used where git would check a file out (Abort), so links are written as links;
/// submodules are left alone.
pub(crate) fn put(repo: &Repository, path: &[u8], entry: Option<(Oid, u32)>) -> Result<()> {
    write(repo, path, entry, true)
}

/// Makes the file at `path` hold exactly the bytes of `entry` (from [`capture`]), or removes it
/// for `None`: a user's own earlier version comes back byte for byte (Undo, rollback).
pub(crate) fn restore(repo: &Repository, path: &[u8], entry: Option<(Oid, u32)>) -> Result<()> {
    write(repo, path, entry, false)
}

fn write(repo: &Repository, path: &[u8], entry: Option<(Oid, u32)>, checkout: bool) -> Result<()> {
    let workdir = workdir(repo)?;
    let full = checked_path(workdir, path)?;
    match entry {
        Some((_, GITLINK)) => Ok(()),
        Some((id, mode)) => {
            not_a_folder(&full)?;
            make_parents(workdir, path)?;
            let bytes = match checkout {
                true => checkout_bytes(repo, path, id, mode)?,
                false => repo.find_blob(id)?.content().to_vec(),
            };
            let temp = stage(repo, &full, &bytes, mode)?;
            fs::rename(&temp, &full)?;
            Ok(())
        }
        None => match metadata(&full)? {
            Some(meta) if !meta.is_dir() => remove_file(workdir, &full),
            _ => Ok(()),
        },
    }
}

/// The bytes `git checkout` writes for the blob at `path`: the blob itself, unless an attribute
/// or `core.autocrlf` filters it, when libgit2 checks it out (into the git directory) to apply
/// the same filters. A link's bytes are its target.
fn checkout_bytes(repo: &Repository, path: &[u8], id: Oid, mode: u32) -> Result<Vec<u8>> {
    let blob = repo.find_blob(id)?;
    if !regular(mode) || !filtered(repo, path)? {
        return Ok(blob.content().to_vec());
    }
    let dir = repo.path().join(CHECKOUT_DIR);
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    fs::create_dir(&dir)?;
    let result = (|| -> Result<Vec<u8>> {
        let mut index = Index::new()?;
        index.add(&entry(path, id, mode))?;
        let mut checkout = CheckoutBuilder::new();
        checkout
            .force()
            .update_index(false)
            .target_dir(&dir)
            .disable_pathspec_match(true)
            .path(OsStr::from_bytes(path));
        repo.checkout_index(Some(&mut index), Some(&mut checkout))?;
        Ok(fs::read(dir.join(OsStr::from_bytes(path)))?)
    })();
    let _ = fs::remove_dir_all(&dir);
    result
}

/// Whether git might change the bytes of `path` on checkout.
fn filtered(repo: &Repository, path: &[u8]) -> Result<bool> {
    let relative = Path::new(OsStr::from_bytes(path));
    for name in FILTER_ATTRIBUTES {
        let value = repo.get_attr_bytes(relative, name, AttrCheckFlags::FILE_THEN_INDEX)?;
        if !matches!(AttrValue::from_bytes(value), AttrValue::Unspecified) {
            return Ok(true);
        }
    }
    Ok(repo.config()?.get_bool("core.autocrlf").unwrap_or(false))
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

/// A path the plan may write or remove: safe, UTF-8 (the undo record names it), not a folder.
fn checked(workdir: &Path, path: &[u8]) -> Result<PathBuf> {
    if std::str::from_utf8(path).is_err() {
        return Err(Error::Git(format!(
            "\"{}\" is not a UTF-8 file name; rename it with git and try again",
            lossy(path)
        )));
    }
    let full = checked_path(workdir, path)?;
    not_a_folder(&full)?;
    Ok(full)
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

/// Writes `bytes` for `full` to the temp file in the git directory, synced, ready to be renamed
/// into place (a link to `bytes`, for a link).
fn stage(repo: &Repository, full: &Path, bytes: &[u8], mode: u32) -> Result<PathBuf> {
    let temp = repo.path().join(TEMP_FILE);
    match fs::remove_file(&temp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    let permissions = match mode {
        BLOB => 0o666,
        EXECUTABLE => 0o777,
        LINK => {
            std::os::unix::fs::symlink(OsStr::from_bytes(bytes), &temp)?;
            return Ok(temp);
        }
        _ => {
            return Err(Error::Git(format!(
                "{} has an unsupported file type (mode {mode:o})",
                full.display()
            )))
        }
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(permissions)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(temp)
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

    fn apply(snapshot: &Snapshot, target: &Files) -> Result<()> {
        let plan = plan(snapshot, target)?;
        match execute(&snapshot.repo, &plan) {
            Ok(_) => Ok(()),
            Err(failed) => {
                rollback(&snapshot.repo, &plan, &failed.done).unwrap();
                Err(failed.error)
            }
        }
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
    fn a_file_edited_after_the_snapshot_stops_everything_and_nothing_is_lost() {
        let (dir, repo) = setup();
        let path = |name: &str| dir.path().join(name);
        fs::write(path("a.md"), "before\n").unwrap();
        fs::write(path("b.md"), "before\n").unwrap();
        let snapshot = Snapshot::take(&repo).unwrap();
        let remote = snapshot.repo.blob(b"remote\n").unwrap();
        // The target adds new.md, replaces a.md and drops b.md; b.md changes after the snapshot.
        let target = Files::from([
            (b"a.md".to_vec(), (remote, BLOB)),
            (b"new.md".to_vec(), (remote, BLOB)),
        ]);
        fs::write(path("b.md"), "typed during the pull\n").unwrap();
        assert_eq!(apply(&snapshot, &target), Err(Error::StaleReview));
        assert_eq!(fs::read_to_string(path("a.md")).unwrap(), "before\n");
        assert_eq!(
            fs::read_to_string(path("b.md")).unwrap(),
            "typed during the pull\n"
        );
        assert!(!path("new.md").exists());

        // The same with the first file to be replaced.
        fs::write(path("a.md"), "typed again\n").unwrap();
        assert_eq!(apply(&snapshot, &target), Err(Error::StaleReview));
        assert_eq!(fs::read_to_string(path("a.md")).unwrap(), "typed again\n");
        assert!(!path("new.md").exists());

        // A file that appeared where a new one goes.
        let snapshot = Snapshot::take(&repo).unwrap();
        let remote = snapshot.repo.blob(b"remote\n").unwrap();
        let mut target = snapshot.files.clone();
        target.insert(b"c.md".to_vec(), (remote, BLOB));
        let plan = plan(&snapshot, &target).unwrap();
        fs::write(path("c.md"), "mine\n").unwrap();
        assert!(matches!(
            execute(&snapshot.repo, &plan),
            Err(Failed {
                error: Error::StaleReview,
                ..
            })
        ));
        assert_eq!(fs::read_to_string(path("c.md")).unwrap(), "mine\n");
    }

    #[test]
    fn plans_write_only_what_differs() {
        let (dir, repo) = setup();
        fs::write(dir.path().join("same.md"), "same\n").unwrap();
        fs::write(dir.path().join("gone.md"), "gone\n").unwrap();
        let snapshot = Snapshot::take(&repo).unwrap();
        let new = snapshot.repo.blob(b"new\n").unwrap();
        let mut target = snapshot.files.clone();
        target.remove(b"gone.md".as_slice());
        target.insert(b"dir/new.md".to_vec(), (new, BLOB));
        let plan = plan(&snapshot, &target).unwrap();
        let paths: Vec<_> = plan.steps.iter().map(|s| lossy(&s.path)).collect();
        assert_eq!(paths, ["dir/new.md", "gone.md"]);
        let done = execute(&snapshot.repo, &plan).ok().unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("dir/new.md")).unwrap(),
            "new\n"
        );
        assert!(!dir.path().join("gone.md").exists());
        rollback(&snapshot.repo, &plan, &done).unwrap();
        assert!(!dir.path().join("dir").exists(), "emptied folders go too");
        assert_eq!(
            fs::read_to_string(dir.path().join("gone.md")).unwrap(),
            "gone\n"
        );
    }

    #[test]
    fn ignored_files_in_the_way_are_refused() {
        let (dir, repo) = setup();
        fs::write(dir.path().join(".gitignore"), "*.local\n").unwrap();
        fs::write(dir.path().join("notes.local"), "private\n").unwrap();
        let snapshot = Snapshot::take(&repo).unwrap();
        let blob = snapshot.repo.blob(b"remote\n").unwrap();
        let mut target = snapshot.files.clone();
        target.insert(b"notes.local".to_vec(), (blob, BLOB));
        let result = plan(&snapshot, &target).map(|_| ());
        assert!(
            matches!(&result, Err(Error::Git(m)) if m.contains("ignored")),
            "{result:?}"
        );
    }

    #[test]
    fn unsafe_paths_are_refused_before_anything_is_written() {
        let (dir, repo) = setup();
        let snapshot = Snapshot::take(&repo).unwrap();
        let blob = snapshot.repo.blob(b"x").unwrap();
        for bad in [
            &b"../escape.md"[..],
            b".git/config",
            b"a//b.md",
            b"./a.md",
            b"\xff.md",
        ] {
            let target = Files::from([
                (b"aaa-first.md".to_vec(), (blob, BLOB)),
                (bad.to_vec(), (blob, BLOB)),
            ]);
            assert!(apply(&snapshot, &target).is_err());
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
            (b"aaa-first.md".to_vec(), (blob, BLOB)),
            (b"link".to_vec(), (link, LINK)),
        ]);
        let result = apply(&snapshot, &target);
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
        let target = Files::from([(b"a/x.md".to_vec(), (blob, BLOB))]);
        assert!(apply(&snapshot, &target).is_err());
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
            (b"A".to_vec(), (blob, BLOB)),
            (b"a/x.md".to_vec(), (blob, BLOB)),
            (b"aaa-first.md".to_vec(), (blob, BLOB)),
        ]);
        assert!(apply(&snapshot, &target).is_err());
        assert!(!dir.path().join("aaa-first.md").exists());
        assert!(!dir.path().join("A").exists());
    }

    #[test]
    fn put_restores_links_and_files() {
        let (dir, repo) = setup();
        let target = repo.blob(b"elsewhere").unwrap();
        let text = repo.blob(b"text\n").unwrap();
        put(&repo, b"sub/link", Some((target, LINK))).unwrap();
        put(&repo, b"sub/file.md", Some((text, BLOB))).unwrap();
        let link = dir.path().join("sub/link");
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("elsewhere"));
        assert_eq!(on_disk(&repo, b"sub/link").unwrap(), Some(target));
        assert_eq!(on_disk(&repo, b"sub/file.md").unwrap(), Some(text));
        assert_eq!(on_disk(&repo, b"sub").unwrap(), Some(Oid::ZERO_SHA1));
        put(&repo, b"sub/link", None).unwrap();
        put(&repo, b"sub/file.md", None).unwrap();
        assert!(!dir.path().join("sub").exists());
        assert_eq!(on_disk(&repo, b"sub/file.md").unwrap(), None);
    }
}
