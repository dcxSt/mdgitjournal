//! A journal directory: list, read, atomic write, create, rename, delete, import images.

use std::cmp::Reverse;
use std::fs::{self, File, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::entries::{format_filename, parse_filename, slugify, word_count};
use crate::images::{compress_image, ImageOpts, ImportedImage};
use crate::{atomic, Error, Result};

/// The only subfolder of a journal.
const ASSETS: &str = "assets";

/// A journal: a flat directory of `.md` entries plus `assets/` (the git working tree).
#[derive(Debug, Clone)]
pub struct Journal {
    root: PathBuf,
}

/// An entry as the sidebar shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryMeta {
    pub file: String,
    /// `"2026-11-16"`; `None` for files that don't match the entry pattern.
    pub date: Option<String>,
    /// `"14:02"`; `None` for files that don't match the entry pattern.
    pub time: Option<String>,
    /// The slug, or the file stem for files that don't match the entry pattern.
    pub title: String,
    /// Modification time in Unix milliseconds.
    pub mtime: i64,
    pub size: u64,
    /// `None` if the file isn't readable UTF-8 text.
    pub words: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryContent {
    pub file: String,
    pub content: String,
    /// Hex SHA-256 of the content, as [`Journal::write`] returns it.
    pub hash: String,
}

impl Journal {
    /// Opens the journal at `root`, creating the directory if it is missing.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|err| {
            Error::io(
                format_args!("could not open the journal folder {}", root.display()),
                err,
            )
        })?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The top-level `.md` files: dated entries newest first, then other files by name.
    /// Dotfiles, directories and symlinks are skipped.
    pub fn list(&self) -> Result<Vec<EntryMeta>> {
        let mut entries = Vec::new();
        for file in self.files()? {
            match self.meta(&file) {
                Ok(meta) => entries.push(meta),
                // Removed since the directory was read.
                Err(Error::NotFound(_)) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(entries)
    }

    /// Reads an entry. Fails with [`Error::Invalid`] if it isn't UTF-8, rather than altering it.
    pub fn read(&self, file: &str) -> Result<EntryContent> {
        validate(file)?;
        let bytes = fs::read(self.root.join(file)).map_err(|err| Error::file("read", file, err))?;
        let hash = hash(&bytes);
        let content = String::from_utf8(bytes).map_err(|_| {
            Error::Invalid(format!(
                "{file} is not UTF-8 text, so it can't be edited here"
            ))
        })?;
        Ok(EntryContent {
            file: file.to_owned(),
            content,
            hash,
        })
    }

    /// Atomically replaces (or creates) an entry with exactly `content`; returns its hash.
    pub fn write(&self, file: &str, content: &str) -> Result<String> {
        validate(file)?;
        atomic::write(&self.root.join(file), content.as_bytes())
            .map_err(|err| Error::io(format_args!("could not save {file}"), err))?;
        Ok(hash(content.as_bytes()))
    }

    /// Creates an empty entry named after `title` at `now` (to the minute). If the name is
    /// taken, `-2`, `-3`, … is appended to the slug.
    pub fn create(&self, title: &str, now: NaiveDateTime) -> Result<EntryMeta> {
        let slug = slugify(title);
        let mut n = 1;
        loop {
            let file = format_filename(now.date(), now.time(), &numbered(&slug, n));
            match File::create_new(self.root.join(&file)) {
                Ok(_) => return self.meta(&file),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => n += 1,
                Err(err) => return Err(Error::io(format_args!("could not create {file}"), err)),
            }
        }
    }

    /// Changes an entry's slug to match `new_title`, keeping its date and time. If the name is
    /// taken, `-2`, `-3`, … is appended; the entry's current name never counts as taken, so
    /// renaming to the same title is a no-op.
    pub fn rename(&self, file: &str, new_title: &str) -> Result<EntryMeta> {
        validate(file)?;
        let entry = parse_filename(file).ok_or_else(|| {
            Error::Invalid(format!(
                "{file} has no date and time in its name, so it can't be renamed"
            ))
        })?;
        let from = self.root.join(file);
        let id = fs::symlink_metadata(&from)
            .map(|meta| file_id(&meta))
            .map_err(|err| Error::file("rename", file, err))?;
        let slug = slugify(new_title);
        let mut n = 1;
        loop {
            let target = format_filename(entry.date, entry.time, &numbered(&slug, n));
            if target == file {
                return self.meta(file);
            }
            let to = self.root.join(&target);
            let taken = match fs::symlink_metadata(&to) {
                // A case-insensitive file system finds the entry itself if only the case changes.
                Ok(meta) => file_id(&meta) != id,
                Err(err) if err.kind() == io::ErrorKind::NotFound => false,
                Err(err) => return Err(Error::io(format_args!("could not rename {file}"), err)),
            };
            if !taken {
                fs::rename(&from, &to).map_err(|err| Error::file("rename", file, err))?;
                return self.meta(&target);
            }
            n += 1;
        }
    }

    pub fn delete(&self, file: &str) -> Result<()> {
        validate(file)?;
        fs::remove_file(self.root.join(file)).map_err(|err| Error::file("delete", file, err))
    }

    /// Compresses an image (see [`compress_image`]) and saves it as
    /// `assets/<entry-stem>-N.jpg` with the first free N, starting at 1.
    pub fn import_image(
        &self,
        entry_file: &str,
        bytes: &[u8],
        opts: ImageOpts,
    ) -> Result<ImportedImage> {
        validate(entry_file)?;
        let (jpeg, width, height) = compress_image(bytes, opts)?;
        let dir = self.root.join(ASSETS);
        let stem = stem(entry_file);
        let name = fs::create_dir_all(&dir)
            .and_then(|()| atomic::write_new(&dir, &jpeg, |n| format!("{stem}-{n}.jpg")))
            .map_err(|err| Error::io("could not save the image", err))?;
        Ok(ImportedImage {
            path: format!("{ASSETS}/{name}"),
            bytes: jpeg.len() as u64,
            width,
            height,
        })
    }

    /// Valid top-level `.md` file names in [`Journal::list`] order.
    pub(crate) fn files(&self) -> Result<Vec<String>> {
        let list_error = |err| Error::io("could not list the journal folder", err);
        let mut files = Vec::new();
        for dirent in fs::read_dir(&self.root).map_err(list_error)? {
            let dirent = dirent.map_err(list_error)?;
            let Ok(name) = dirent.file_name().into_string() else {
                continue;
            };
            if is_valid_file(&name) && dirent.file_type().is_ok_and(|t| t.is_file()) {
                files.push(name);
            }
        }
        // Dated entries newest first (`Reverse` also puts `Some` before `None`), then by name.
        files.sort_by_cached_key(|name| {
            let entry = parse_filename(name).map(|e| (e.date, e.time, e.slug));
            (Reverse(entry), name.clone())
        });
        Ok(files)
    }

    fn meta(&self, file: &str) -> Result<EntryMeta> {
        let path = self.root.join(file);
        let meta = fs::metadata(&path).map_err(|err| Error::file("read", file, err))?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let entry = parse_filename(file);
        Ok(EntryMeta {
            file: file.to_owned(),
            date: entry
                .as_ref()
                .map(|e| e.date.format("%Y-%m-%d").to_string()),
            time: entry.as_ref().map(|e| e.time.format("%H:%M").to_string()),
            title: title(file),
            mtime,
            size: meta.len(),
            words: fs::read_to_string(&path).ok().map(|text| word_count(&text)),
        })
    }
}

/// What the UI shows for a file: the slug of an entry, or the stem of any other file.
pub(crate) fn title(file: &str) -> String {
    parse_filename(file).map_or_else(|| stem(file).to_owned(), |e| e.slug)
}

/// `file` without `.md`; only for names that passed [`is_valid_file`].
pub(crate) fn stem(file: &str) -> &str {
    &file[..file.len() - ".md".len()]
}

/// A bare `.md` file name in the journal root: no path separators, no `..`, not hidden.
fn is_valid_file(name: &str) -> bool {
    name.len() > ".md".len()
        && name.ends_with(".md")
        && !name.starts_with('.')
        && !name.contains("..")
        && !name.contains(['/', '\\', '\0'])
}

fn validate(file: &str) -> Result<()> {
    if is_valid_file(file) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "{file:?} is not a valid entry file name"
        )))
    }
}

/// Device and inode: equal for two names of the same file.
fn file_id(meta: &Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

fn numbered(slug: &str, n: u32) -> String {
    if n == 1 {
        slug.to_owned()
    } else {
        format!("{slug}-{n}")
    }
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_file_names() {
        for name in [
            "a.md",
            "2026-11-16-1402-run.md",
            "My notes.md",
            "é.md",
            "a.b.md",
        ] {
            assert!(is_valid_file(name), "{name:?}");
        }
        for name in [
            "",
            ".md",
            "md",
            "a.txt",
            "a.md/",
            "a.MD",
            ".hidden.md",
            "../a.md",
            "..md",
            "a..md",
            "dir/a.md",
            "/a.md",
            "dir\\a.md",
            "a\0.md",
            "assets",
        ] {
            assert!(!is_valid_file(name), "{name:?}");
        }
    }

    #[test]
    fn titles() {
        assert_eq!(title("2026-11-16-1402-morning-run-2.md"), "morning-run-2");
        assert_eq!(title("Ideas.md"), "Ideas");
        assert_eq!(title("2026-11-16-1402-.md"), "2026-11-16-1402-");
    }

    #[test]
    fn numbered_slugs() {
        assert_eq!(numbered("run", 1), "run");
        assert_eq!(numbered("run", 2), "run-2");
        assert_eq!(numbered("day-2", 3), "day-2-3");
    }

    #[test]
    fn hash_is_hex_sha256() {
        assert_eq!(
            hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
