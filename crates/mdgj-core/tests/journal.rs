use std::fs;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};
use mdgj_core::{Error, ImageOpts, Journal, SearchHit};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn journal() -> (TempDir, Journal) {
    let dir = tempfile::tempdir().unwrap();
    let journal = Journal::open(dir.path()).unwrap();
    (dir, journal)
}

fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, s)
        .unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn files(journal: &Journal) -> Vec<String> {
    journal
        .list()
        .unwrap()
        .into_iter()
        .map(|m| m.file)
        .collect()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The mode `File::create` gives under the current umask.
fn created_mode() -> u32 {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reference");
    fs::File::create(&path).unwrap();
    mode(&path)
}

const OPTS: ImageOpts = ImageOpts {
    max_edge: 1024,
    quality: 60,
};

#[test]
fn open_creates_missing_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("a/b/journal");
    let journal = Journal::open(&root).unwrap();
    assert!(root.is_dir());
    assert_eq!(journal.root(), root);
    assert!(journal.list().unwrap().is_empty());
}

#[test]
fn open_fails_readably_on_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("journal");
    fs::write(&file, "").unwrap();
    let err = Journal::open(&file).unwrap_err();
    assert!(matches!(err, Error::Io(_)));
    assert!(
        err.to_string()
            .starts_with("could not open the journal folder "),
        "{err}"
    );
}

#[test]
fn create_makes_an_empty_entry() {
    let (dir, journal) = journal();
    let meta = journal
        .create("Morning Run", at(2026, 11, 16, 14, 2, 59))
        .unwrap();
    assert_eq!(meta.file, "2026-11-16-1402-morning-run.md");
    assert_eq!(meta.date.as_deref(), Some("2026-11-16"));
    assert_eq!(meta.time.as_deref(), Some("14:02"));
    assert_eq!(meta.title, "morning-run");
    assert_eq!((meta.size, meta.words), (0, Some(0)));
    assert!(meta.mtime > 0);
    assert_eq!(fs::read(dir.path().join(&meta.file)).unwrap(), b"");
}

#[test]
fn create_dedupes_names() {
    let (_dir, journal) = journal();
    let now = at(2026, 11, 16, 14, 2, 0);
    let made: Vec<_> = (0..3)
        .map(|_| journal.create("Run", now).unwrap().file)
        .collect();
    assert_eq!(
        made,
        [
            "2026-11-16-1402-run.md",
            "2026-11-16-1402-run-2.md",
            "2026-11-16-1402-run-3.md"
        ]
    );
    // An untitled entry and an all-punctuation title share the fallback slug.
    assert_eq!(
        journal.create("", now).unwrap().file,
        "2026-11-16-1402-untitled.md"
    );
    assert_eq!(
        journal.create("?!", now).unwrap().file,
        "2026-11-16-1402-untitled-2.md"
    );
    // Same title a minute later is no collision.
    assert_eq!(
        journal
            .create("Run", at(2026, 11, 16, 14, 3, 0))
            .unwrap()
            .file,
        "2026-11-16-1403-run.md"
    );
}

#[test]
fn create_skips_names_taken_by_directories() {
    let (dir, journal) = journal();
    fs::create_dir(dir.path().join("2026-11-16-1402-run.md")).unwrap();
    assert_eq!(
        journal
            .create("run", at(2026, 11, 16, 14, 2, 0))
            .unwrap()
            .file,
        "2026-11-16-1402-run-2.md"
    );
}

#[test]
fn write_and_read_preserve_bytes_exactly() {
    let (_dir, journal) = journal();
    let file = journal
        .create("bytes", at(2026, 11, 16, 9, 0, 0))
        .unwrap()
        .file;
    for content in [
        "",
        "no trailing newline",
        "trailing newline\n",
        "crlf\r\nline two\r\n",
        "mixed\r\nendings\nand a lone\rcr",
        "  leading and trailing spaces  \n\n\n",
        "\u{feff}BOM, tabs\tand unicode: é 日本 🦀 \u{0}",
    ] {
        let hash = journal.write(&file, content).unwrap();
        let read = journal.read(&file).unwrap();
        assert_eq!(read.content, content);
        assert_eq!(read.hash, hash);
        assert_eq!(read.file, file);
        assert_eq!(
            fs::read(journal.root().join(&file)).unwrap(),
            content.as_bytes()
        );
    }
}

#[test]
fn hash_is_sha256_of_content() {
    let (_dir, journal) = journal();
    let hash = journal.write("abc.md", "abc").unwrap();
    assert_eq!(
        hash,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_ne!(journal.write("abc.md", "abd").unwrap(), hash);
}

#[test]
fn write_is_atomic_and_leaves_no_temp_files() {
    let (dir, journal) = journal();
    journal.write("a.md", "one").unwrap();
    journal.write("a.md", "two").unwrap();
    assert_eq!(names(dir.path()), ["a.md"]);
}

#[test]
fn write_keeps_file_permissions() {
    let (dir, journal) = journal();
    let path = dir.path().join("kept.md");
    fs::write(&path, "old").unwrap();
    // Exactly, even bits the usual 022 umask would clear.
    for kept in [0o640, 0o600, 0o666, 0o775] {
        fs::set_permissions(&path, fs::Permissions::from_mode(kept)).unwrap();
        journal.write("kept.md", "new").unwrap();
        assert_eq!(mode(&path), kept, "{kept:o}");
    }

    // New files get normal permissions, not the 0600 of a temp file.
    journal.write("fresh.md", "x").unwrap();
    assert_eq!(mode(&dir.path().join("fresh.md")), created_mode());
}

#[test]
fn read_errors() {
    let (dir, journal) = journal();
    assert!(matches!(journal.read("missing.md"), Err(Error::NotFound(f)) if f == "missing.md"));
    assert_eq!(
        journal.read("missing.md").unwrap_err().to_string(),
        "missing.md does not exist"
    );

    fs::write(dir.path().join("latin1.md"), b"caf\xe9").unwrap();
    let err = journal.read("latin1.md").unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "latin1.md is not UTF-8 text, so it can't be edited here"
    );
    // ... and the file is untouched.
    assert_eq!(fs::read(dir.path().join("latin1.md")).unwrap(), b"caf\xe9");
}

#[test]
fn file_arguments_are_validated() {
    let (dir, journal) = journal();
    fs::write(dir.path().join("..md"), "").unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub/x.md"), "secret").unwrap();
    let outside = dir.path().parent().unwrap().join("outside.md");
    let outside = outside.to_str().unwrap();
    for bad in [
        "",
        ".md",
        "x",
        "x.txt",
        "x.MD",
        ".hidden.md",
        "..md",
        "../x.md",
        "a..b.md",
        "sub/x.md",
        "/etc/passwd.md",
        "sub\\x.md",
        "x\0.md",
        outside,
    ] {
        let invalid = |r: Result<(), Error>| matches!(r, Err(Error::Invalid(_)));
        assert!(invalid(journal.read(bad).map(drop)), "read {bad:?}");
        assert!(invalid(journal.write(bad, "x").map(drop)), "write {bad:?}");
        assert!(invalid(journal.delete(bad)), "delete {bad:?}");
        assert!(
            invalid(journal.rename(bad, "y").map(drop)),
            "rename {bad:?}"
        );
        assert!(
            invalid(journal.import_image(bad, &[], OPTS).map(drop)),
            "import_image {bad:?}"
        );
    }
    assert_eq!(
        journal.read("../x.md").unwrap_err().to_string(),
        r#""../x.md" is not a valid entry file name"#
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("sub/x.md")).unwrap(),
        "secret"
    );
    assert!(!Path::new(outside).exists());
}

#[test]
fn list_orders_entries_newest_first_then_others_by_name() {
    let (dir, journal) = journal();
    let root = dir.path();
    for name in [
        "2026-11-16-1402-morning-run.md",
        "2026-11-16-1402-morning-run-2.md",
        "2026-11-16-1402-alpha.md",
        "2026-11-16-0900-early.md",
        "2026-11-15-2359-yesterday.md",
        "2025-12-31-2359-last-year.md",
        "2027-01-01-0000-future.md",
        "zeta.md",
        "Alpha.md",
        "notes.md",
        "2026-02-30-1200-not-a-date.md",
        "2026-11-16-1402-.md",
    ] {
        fs::write(root.join(name), "").unwrap();
    }
    // Not listed: other extensions, dotfiles, directories and their contents, symlinks.
    fs::write(root.join("readme.txt"), "").unwrap();
    fs::write(root.join(".draft.md"), "").unwrap();
    fs::write(root.join(".mdgj-tmp-abc"), "").unwrap();
    fs::create_dir(root.join("assets")).unwrap();
    fs::write(root.join("assets/2026-11-16-1402-in-assets.md"), "").unwrap();
    fs::create_dir(root.join("dir.md")).unwrap();
    std::os::unix::fs::symlink(root.join("notes.md"), root.join("link.md")).unwrap();

    assert_eq!(
        files(&journal),
        [
            "2027-01-01-0000-future.md",
            "2026-11-16-1402-morning-run-2.md",
            "2026-11-16-1402-morning-run.md",
            "2026-11-16-1402-alpha.md",
            "2026-11-16-0900-early.md",
            "2026-11-15-2359-yesterday.md",
            "2025-12-31-2359-last-year.md",
            "2026-02-30-1200-not-a-date.md",
            "2026-11-16-1402-.md",
            "Alpha.md",
            "notes.md",
            "zeta.md",
        ]
    );
}

#[test]
fn list_metadata() {
    let (dir, journal) = journal();
    fs::write(
        dir.path().join("2026-11-16-1402-run.md"),
        "# Run\n\n- [x] 5 km\n",
    )
    .unwrap();
    fs::write(dir.path().join("Ideas.md"), "one two").unwrap();
    fs::write(dir.path().join("binary.md"), b"\xff\xfe").unwrap();
    let list = journal.list().unwrap();

    let run = &list[0];
    assert_eq!(run.file, "2026-11-16-1402-run.md");
    assert_eq!(
        (run.date.as_deref(), run.time.as_deref()),
        (Some("2026-11-16"), Some("14:02"))
    );
    assert_eq!(run.title, "run");
    assert_eq!((run.size, run.words), (18, Some(3)));

    // Others sort by name, byte-wise: uppercase first.
    let ideas = &list[1];
    assert_eq!((ideas.date.as_deref(), ideas.time.as_deref()), (None, None));
    assert_eq!(
        (ideas.title.as_str(), ideas.size, ideas.words),
        ("Ideas", 7, Some(2))
    );

    let binary = &list[2];
    assert_eq!((binary.title.as_str(), binary.words), ("binary", None));
}

#[test]
fn list_is_fast_enough_for_thousands_of_entries() {
    let (dir, journal) = journal();
    let body = "Some words on a line.\n".repeat(50);
    for i in 0..3000 {
        let name = format!(
            "2026-{:02}-{:02}-{:02}{:02}-entry.md",
            1 + i % 12,
            1 + i % 28,
            i % 24,
            i % 60
        );
        fs::write(dir.path().join(name), &body).unwrap();
    }
    let started = std::time::Instant::now();
    let list = journal.list().unwrap();
    let hits = journal.search("line", 200).unwrap();
    let elapsed = started.elapsed();
    assert!(!list.is_empty() && hits.len() == 200);
    // Generous: debug builds on slow CI machines.
    assert!(elapsed.as_secs() < 10, "{elapsed:?}");
}

#[test]
fn rename_keeps_date_and_time() {
    let (dir, journal) = journal();
    let file = journal
        .create("Morning Run", at(2026, 11, 16, 14, 2, 0))
        .unwrap()
        .file;
    journal.write(&file, "keep me\r\n").unwrap();
    let meta = journal.rename(&file, "Evening Walk!").unwrap();
    assert_eq!(meta.file, "2026-11-16-1402-evening-walk.md");
    assert_eq!(
        (meta.date.as_deref(), meta.time.as_deref()),
        (Some("2026-11-16"), Some("14:02"))
    );
    assert_eq!(meta.title, "evening-walk");
    assert_eq!(names(dir.path()), ["2026-11-16-1402-evening-walk.md"]);
    assert_eq!(journal.read(&meta.file).unwrap().content, "keep me\r\n");
}

#[test]
fn rename_to_same_title_is_a_noop() {
    let (dir, journal) = journal();
    let file = journal
        .create("run", at(2026, 11, 16, 14, 2, 0))
        .unwrap()
        .file;
    assert_eq!(journal.rename(&file, "Run").unwrap().file, file);
    assert_eq!(journal.rename(&file, "  RUN!! ").unwrap().file, file);
    assert_eq!(names(dir.path()), [file]);
}

#[test]
fn rename_dedupes_without_clobbering() {
    let (dir, journal) = journal();
    let now = at(2026, 11, 16, 14, 2, 0);
    let run = journal.create("run", now).unwrap().file;
    let walk = journal.create("walk", now).unwrap().file;
    journal.write(&run, "run text").unwrap();
    journal.write(&walk, "walk text").unwrap();

    let renamed = journal.rename(&walk, "Run").unwrap().file;
    assert_eq!(renamed, "2026-11-16-1402-run-2.md");
    assert_eq!(journal.read(&run).unwrap().content, "run text");
    assert_eq!(journal.read(&renamed).unwrap().content, "walk text");

    // Renaming the duplicate to the title it already carries changes nothing.
    assert_eq!(journal.rename(&renamed, "run").unwrap().file, renamed);
    assert_eq!(
        names(dir.path()),
        ["2026-11-16-1402-run-2.md", "2026-11-16-1402-run.md"]
    );

    // Other entries with the same slug but a different time are no collision.
    let other = journal
        .create("swim", at(2026, 11, 17, 8, 0, 0))
        .unwrap()
        .file;
    assert_eq!(
        journal.rename(&other, "run").unwrap().file,
        "2026-11-17-0800-run.md"
    );
}

#[test]
fn rename_changing_only_case_is_no_collision() {
    // On a case-insensitive file system (macOS by default) the new name finds the entry itself.
    let (dir, journal) = journal();
    let file = "2026-11-16-1402-Run.md";
    fs::write(dir.path().join(file), "text").unwrap();
    let meta = journal.rename(file, "run").unwrap();
    assert_eq!(meta.file, "2026-11-16-1402-run.md");
    assert_eq!(names(dir.path()), ["2026-11-16-1402-run.md"]);
    assert_eq!(journal.read(&meta.file).unwrap().content, "text");
}

#[test]
fn rename_errors() {
    let (dir, journal) = journal();
    let err = journal.rename("2026-11-16-1402-gone.md", "x").unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");

    fs::write(dir.path().join("Ideas.md"), "").unwrap();
    let err = journal.rename("Ideas.md", "x").unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "Ideas.md has no date and time in its name, so it can't be renamed"
    );
    assert_eq!(names(dir.path()), ["Ideas.md"]);
}

#[test]
fn delete_removes_the_entry() {
    let (dir, journal) = journal();
    let file = journal
        .create("bye", at(2026, 11, 16, 14, 2, 0))
        .unwrap()
        .file;
    journal.delete(&file).unwrap();
    assert!(names(dir.path()).is_empty());
    assert!(matches!(journal.delete(&file), Err(Error::NotFound(_))));
}

fn hit(file: &str, title: &str, line: u32, snippet: &str, range: (u32, u32)) -> SearchHit {
    SearchHit {
        file: file.into(),
        title: title.into(),
        line,
        snippet: snippet.into(),
        match_start: range.0,
        match_end: range.1,
    }
}

#[test]
fn search_names_and_lines_newest_first() {
    let (dir, journal) = journal();
    fs::write(
        dir.path().join("2026-11-15-0800-coffee.md"),
        "Morning COFFEE\r\nno match\r\nmore coffee\r\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("2026-11-16-1402-coffee-run.md"),
        "ran to the café",
    )
    .unwrap();
    fs::write(dir.path().join("Coffee notes.md"), "beans").unwrap();
    fs::write(dir.path().join("2026-11-16-1500-tea.md"), "tea only").unwrap();

    assert_eq!(
        journal.search("Coffee", 200).unwrap(),
        [
            hit(
                "2026-11-16-1402-coffee-run.md",
                "coffee-run",
                0,
                "2026-11-16-1402-coffee-run",
                (16, 22)
            ),
            hit(
                "2026-11-15-0800-coffee.md",
                "coffee",
                0,
                "2026-11-15-0800-coffee",
                (16, 22)
            ),
            hit(
                "2026-11-15-0800-coffee.md",
                "coffee",
                1,
                "Morning COFFEE",
                (8, 14)
            ),
            hit(
                "2026-11-15-0800-coffee.md",
                "coffee",
                3,
                "more coffee",
                (5, 11)
            ),
            hit("Coffee notes.md", "Coffee notes", 0, "Coffee notes", (0, 6)),
        ]
    );
    assert_eq!(
        journal.search("CAFÉ", 200).unwrap(),
        [hit(
            "2026-11-16-1402-coffee-run.md",
            "coffee-run",
            1,
            "ran to the café",
            (11, 15)
        )]
    );
}

#[test]
fn search_limit_and_blank_queries() {
    let (dir, journal) = journal();
    fs::write(dir.path().join("2026-11-16-1402-a.md"), "x\nx\nx\n").unwrap();
    fs::write(dir.path().join("2026-11-15-1402-b.md"), "x\nx\n").unwrap();
    assert_eq!(journal.search("x", 200).unwrap().len(), 5);
    let two = journal.search("x", 2).unwrap();
    assert_eq!(
        two.iter()
            .map(|h| (h.file.as_str(), h.line))
            .collect::<Vec<_>>(),
        [("2026-11-16-1402-a.md", 1), ("2026-11-16-1402-a.md", 2)]
    );
    assert_eq!(journal.search("x", 4).unwrap().len(), 4);
    assert!(journal.search("x", 0).unwrap().is_empty());
    assert!(journal.search("", 200).unwrap().is_empty());
    assert!(journal.search(" \t", 200).unwrap().is_empty());
    assert!(journal.search("nothing", 200).unwrap().is_empty());
}

#[test]
fn search_multibyte_snippets() {
    let (dir, journal) = journal();
    let line = format!(
        "{}Ünïcødé{}",
        "日本語のテキスト、".repeat(10),
        "🦀".repeat(60)
    );
    fs::write(
        dir.path().join("2026-11-16-1402-multi.md"),
        format!("first\n{line}\n"),
    )
    .unwrap();
    let hits = journal.search("üNÏCØDÉ", 200).unwrap();
    assert_eq!(hits.len(), 1);
    let SearchHit {
        line: n,
        snippet,
        match_start,
        match_end,
        ..
    } = &hits[0];
    assert_eq!(*n, 2);
    let chars: Vec<char> = snippet.chars().collect();
    assert_eq!(chars.len(), 1 + 40 + 7 + 40 + 1);
    assert_eq!((*match_start, *match_end), (41, 48));
    assert_eq!(chars[41..48].iter().collect::<String>(), "Ünïcødé");
    assert_eq!(chars[48..88].iter().collect::<String>(), "🦀".repeat(40));
}

#[test]
fn search_skips_unreadable_content_but_matches_its_name() {
    let (dir, journal) = journal();
    fs::write(dir.path().join("2026-11-16-1402-blob.md"), b"blob \xff").unwrap();
    let hits = journal.search("blob", 200).unwrap();
    assert_eq!(hits.iter().map(|h| h.line).collect::<Vec<_>>(), [0]);
}

/// A 3000×2000 PNG with some texture; `seed` changes it.
fn png(seed: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(3000, 2000, |x, y| {
        let x = x + seed;
        image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8])
    });
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

/// `assets/<stem>-<the first 8 hex digits of the SHA-256 of jpeg>`, without `.jpg`.
fn content_name(stem: &str, jpeg: &[u8]) -> String {
    format!("assets/{stem}-{}", &hex::encode(Sha256::digest(jpeg))[..8])
}

#[test]
fn import_image_names_assets_by_content() {
    let (dir, journal) = journal();
    let entry = journal
        .create("Morning Run", at(2026, 11, 16, 14, 2, 0))
        .unwrap()
        .file;

    let first = journal.import_image(&entry, &png(0), OPTS).unwrap();
    let saved = fs::read(dir.path().join(&first.path)).unwrap();
    let base = content_name("2026-11-16-1402-morning-run", &saved);
    assert_eq!(first.path, format!("{base}.jpg"));
    assert_eq!((first.width, first.height), (1024, 683));
    assert_eq!(saved.len() as u64, first.bytes);
    assert_eq!(&saved[..2], [0xFF, 0xD8]);
    assert_eq!(mode(&dir.path().join(&first.path)), created_mode());

    // The same image again is the same file.
    assert_eq!(journal.import_image(&entry, &png(0), OPTS).unwrap(), first);
    assert_eq!(names(&dir.path().join("assets")).len(), 1);

    // A different image gets a different name.
    let other = journal.import_image(&entry, &png(1), OPTS).unwrap();
    assert_ne!(other.path, first.path);
    assert!(other
        .path
        .starts_with("assets/2026-11-16-1402-morning-run-"));

    // A different file under the name is never replaced: the next free number is used, and a
    // numbered file that holds the image is used as it is.
    fs::write(dir.path().join(&first.path), "keep").unwrap();
    let numbered = journal.import_image(&entry, &png(0), OPTS).unwrap();
    assert_eq!(numbered.path, format!("{base}-2.jpg"));
    assert_eq!(fs::read(dir.path().join(&numbered.path)).unwrap(), saved);
    assert_eq!(
        journal.import_image(&entry, &png(0), OPTS).unwrap(),
        numbered
    );
    assert_eq!(fs::read(dir.path().join(&first.path)).unwrap(), b"keep");

    // Assets are not entries, and no temp files are left behind.
    assert_eq!(files(&journal), [entry]);
    assert_eq!(names(&dir.path().join("assets")).len(), 3);
}

/// Two devices adding images to the same entry between syncs must not pick the same name, or
/// the pull sets one aside and the entry links to the other device's image.
#[test]
fn import_image_names_differ_across_devices() {
    let (dir_a, a) = journal();
    let (dir_b, b) = journal();
    let entry = "2026-11-16-1402-run.md";
    let from_a = a.import_image(entry, &png(0), OPTS).unwrap();
    let from_b = b.import_image(entry, &png(1), OPTS).unwrap();
    assert_ne!(from_a.path, from_b.path);
    assert!(!dir_b.path().join(&from_a.path).exists());
    assert!(!dir_a.path().join(&from_b.path).exists());

    // The same image added on both is the same file, which merges cleanly.
    let same = b.import_image(entry, &png(0), OPTS).unwrap();
    assert_eq!(same, from_a);
    assert_eq!(
        fs::read(dir_b.path().join(&same.path)).unwrap(),
        fs::read(dir_a.path().join(&from_a.path)).unwrap()
    );
}

#[test]
fn import_image_refuses_huge_images_without_writing() {
    let (dir, journal) = journal();
    // 72 MP of grey, a small PNG.
    let mut huge = Cursor::new(Vec::new());
    image::GrayImage::new(9000, 8000)
        .write_to(&mut huge, image::ImageFormat::Png)
        .unwrap();
    let err = journal
        .import_image("2026-11-16-1402-x.md", huge.get_ref(), OPTS)
        .unwrap_err();
    assert_eq!(err.to_string(), "the image is too large to import");
    assert!(!dir.path().join("assets").exists());
}

#[test]
fn import_image_never_writes_through_an_assets_link() {
    let (dir, journal) = journal();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("assets")).unwrap();
    let err = journal
        .import_image("2026-11-16-1402-x.md", &png(0), OPTS)
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "could not save the image: assets is not a folder"
    );
    assert_eq!(names(outside.path()), Vec::<String>::new());

    // A file of that name is refused too, and left as it is.
    fs::remove_file(dir.path().join("assets")).unwrap();
    fs::write(dir.path().join("assets"), "notes").unwrap();
    let err = journal
        .import_image("2026-11-16-1402-x.md", &png(0), OPTS)
        .unwrap_err();
    assert!(err.to_string().ends_with("assets is not a folder"), "{err}");
    assert_eq!(
        fs::read_to_string(dir.path().join("assets")).unwrap(),
        "notes"
    );
}

#[test]
fn import_image_rejects_non_images_without_writing() {
    let (dir, journal) = journal();
    let err = journal
        .import_image("2026-11-16-1402-x.md", b"GIF89a broken", OPTS)
        .unwrap_err();
    assert!(matches!(err, Error::Image(_)), "{err:?}");
    assert!(!dir.path().join("assets").exists());
}
