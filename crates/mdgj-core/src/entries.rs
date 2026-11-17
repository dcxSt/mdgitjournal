//! Entry filenames: `yyyy-mm-dd-hhmm-<slug>.md`, slugify, parse and format; word count.

use chrono::{Datelike, NaiveDate, NaiveTime, Timelike};
use unicode_segmentation::UnicodeSegmentation;

/// Longest slug `slugify` returns, so names stay well under the 255-byte filename limit.
const MAX_SLUG_LEN: usize = 80;

/// Turns a title into a filename slug: ASCII-folded, lowercase, runs of anything other than
/// `[a-z0-9]` replaced by one `-`, no leading or trailing `-`. Returns `"untitled"` if nothing
/// is left.
pub fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    for c in deunicode::deunicode(title).chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(MAX_SLUG_LEN);
    let len = slug.trim_end_matches('-').len();
    slug.truncate(len);
    if slug.is_empty() {
        slug.push_str("untitled");
    }
    slug
}

/// A parsed entry filename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryName {
    pub date: NaiveDate,
    /// Hours and minutes; seconds are always zero.
    pub time: NaiveTime,
    /// Everything after `hhmm-`, including any `-N` collision suffix.
    pub slug: String,
    /// The trailing `-N` (N ≥ 2) that collision handling adds, if the slug ends in one. It can
    /// also be part of the title ("day-2"), so it is only a hint and stays in `slug`.
    pub dup: Option<u32>,
}

/// Parses `yyyy-mm-dd-hhmm-<slug>.md` with a real date and time and a non-empty slug.
pub fn parse_filename(name: &str) -> Option<EntryName> {
    let stem = name.strip_suffix(".md")?;
    let b = stem.as_bytes();
    if b.len() < 17 || [4, 7, 10, 15].iter().any(|&i| b[i] != b'-') {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<u32> {
        let digits = &b[from..to];
        digits
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| digits.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0')))
    };
    let year = i32::try_from(number(0, 4)?).ok()?;
    let date = NaiveDate::from_ymd_opt(year, number(5, 7)?, number(8, 10)?)?;
    let time = NaiveTime::from_hms_opt(number(11, 13)?, number(13, 15)?, 0)?;
    let slug = &stem[16..];
    Some(EntryName {
        date,
        time,
        slug: slug.to_owned(),
        dup: dup_suffix(slug),
    })
}

fn dup_suffix(slug: &str) -> Option<u32> {
    let (base, digits) = slug.rsplit_once('-')?;
    if base.is_empty() || digits.starts_with('0') || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|&n| n >= 2)
}

/// Formats `yyyy-mm-dd-hhmm-<slug>.md`. Seconds in `time` are ignored.
pub fn format_filename(date: NaiveDate, time: NaiveTime, slug: &str) -> String {
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}-{slug}.md",
        date.year(),
        date.month(),
        date.day(),
        time.hour(),
        time.minute()
    )
}

/// Counts Unicode words (UAX #29) as a reader sees them. Markdown punctuation such as `#`, `-`,
/// `*` and `[ ]` is not a word, nor is the `x` of a checked task `[x]`, nor anything in the
/// `(…)` of an inline link or image such as `![](assets/….jpg)`; link text and alt text are.
/// Each CJK ideograph counts as one.
pub fn word_count(text: &str) -> u32 {
    let mut words = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let (prose, link) = rest.split_once("](").unwrap_or((rest, ""));
        words += prose_words(prose);
        // A destination ends at `)`; if the line ends first, it wasn't one.
        rest = match link.find([')', '\n']) {
            Some(end) if link.as_bytes()[end] == b')' => &link[end + 1..],
            _ => link,
        };
    }
    u32::try_from(words).unwrap_or(u32::MAX)
}

fn prose_words(text: &str) -> usize {
    let is_task_mark = |at: usize, word: &str| {
        word.eq_ignore_ascii_case("x")
            && text[..at].ends_with('[')
            && text[at + 1..].starts_with(']')
    };
    text.unicode_word_indices()
        .filter(|&(at, word)| !is_task_mark(at, word))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Morning Run"), "morning-run");
        assert_eq!(slugify("morning-run"), "morning-run");
        assert_eq!(slugify("2026 plans"), "2026-plans");
        assert_eq!(slugify("Q3 OKRs v2"), "q3-okrs-v2");
    }

    #[test]
    fn slugify_collapses_and_trims_separators() {
        assert_eq!(slugify("  --Hello,   World!!  "), "hello-world");
        assert_eq!(slugify("a___b"), "a-b");
        assert_eq!(slugify("C++ & Rust"), "c-rust");
        assert_eq!(slugify("one/two\\three.md"), "one-two-three-md");
        assert_eq!(slugify("tab\there\nnewline"), "tab-here-newline");
    }

    #[test]
    fn slugify_folds_unicode() {
        assert_eq!(slugify("Café crème brûlée"), "cafe-creme-brulee");
        assert_eq!(slugify("Grüße aus Köln"), "grusse-aus-koln");
        assert_eq!(slugify("北京"), "bei-jing");
        assert_eq!(slugify("Привет, мир"), "privet-mir");
        assert_eq!(slugify("Ελληνικά"), "ellenika");
        assert_eq!(slugify("e\u{301}te\u{301}"), "ete");
    }

    #[test]
    fn slugify_emoji() {
        assert_eq!(slugify("🦀 Rust"), "crab-rust");
        assert_eq!(slugify("🎉🎉"), "tada-tada");
    }

    #[test]
    fn slugify_empty_results_are_untitled() {
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("   "), "untitled");
        assert_eq!(slugify("!!!"), "untitled");
        assert_eq!(slugify("--- *** ###"), "untitled");
        assert_eq!(slugify("\u{E000}"), "untitled"); // private use: no transliteration
    }

    #[test]
    fn slugify_limits_length_without_trailing_dash() {
        let long = slugify(&"word ".repeat(100));
        assert!(long.len() <= MAX_SLUG_LEN);
        assert!(!long.ends_with('-'));
        assert!(long.starts_with("word-word"));

        // Cut exactly after a separator: 79 chars then '-' at index 79.
        let title = format!("{} tail", "a".repeat(79));
        assert_eq!(slugify(&title), "a".repeat(79));
    }

    #[test]
    fn slugify_output_is_always_a_valid_slug() {
        for title in [
            "Ünïcødé ✓",
            "--",
            "x",
            "日本語のテキスト",
            "a\u{0}b",
            "🏳️‍🌈 pride",
        ] {
            let slug = slugify(title);
            assert!(!slug.is_empty());
            assert!(slug
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'));
            assert!(!slug.starts_with('-') && !slug.ends_with('-') && !slug.contains("--"));
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn time(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn parse_entry() {
        assert_eq!(
            parse_filename("2026-11-16-1402-morning-run.md"),
            Some(EntryName {
                date: date(2026, 11, 16),
                time: time(14, 2),
                slug: "morning-run".into(),
                dup: None,
            })
        );
    }

    #[test]
    fn parse_keeps_dup_suffix_in_slug() {
        let e = parse_filename("2026-11-16-1402-morning-run-2.md").unwrap();
        assert_eq!(e.slug, "morning-run-2");
        assert_eq!(e.dup, Some(2));
        assert_eq!(
            parse_filename("2026-11-16-1402-x-17.md").unwrap().dup,
            Some(17)
        );
    }

    #[test]
    fn parse_dup_only_for_collision_shaped_suffixes() {
        for (name, dup) in [
            ("2026-11-16-1402-2.md", None),      // slug is just "2"
            ("2026-11-16-1402-day-1.md", None),  // collisions start at 2
            ("2026-11-16-1402-day-02.md", None), // never zero-padded
            ("2026-11-16-1402-day-.md", None),
            ("2026-11-16-1402-day-2x.md", None),
            ("2026-11-16-1402-day-+2.md", None),
            ("2026-11-16-1402-day-99999999999.md", None), // overflows u32
        ] {
            assert_eq!(parse_filename(name).unwrap().dup, dup, "{name}");
        }
    }

    #[test]
    fn parse_accepts_any_nonempty_slug() {
        // Hand-made or conflict-copy names still sort as entries of that date.
        assert_eq!(
            parse_filename("2026-11-16-1402-My Notes.md").unwrap().slug,
            "My Notes"
        );
        assert_eq!(
            parse_filename("2026-11-16-1402-run.conflict-1530.md")
                .unwrap()
                .slug,
            "run.conflict-1530"
        );
        assert_eq!(parse_filename("2026-11-16-1402-é.md").unwrap().slug, "é");
    }

    #[test]
    fn parse_rejects_non_entries() {
        for name in [
            "",
            ".md",
            "notes.md",
            "2026-11-16-1402.md",
            "2026-11-16-1402-.md",
            "2026-11-16-1402-run",
            "2026-11-16-1402-run.txt",
            "2026-11-16-1402-run.md.bak",
            "2026-11-16-1402-run.MD",
            "2026-11-16_1402-run.md",
            "2026-11-16-14:02-run.md",
            "2026-11-16-142-run.md",
            "26-11-16-1402-run.md",
            "2026-1-16-1402-run.md",
            "+026-11-16-1402-run.md",
            "2026-+1-16-1402-run.md",
            "2026-11-16-1402xrun.md",
            "2026-02-30-1402-run.md",
            "2026-13-01-1402-run.md",
            "2026-11-16-2400-run.md",
            "2026-11-16-1260-run.md",
            "２０２６-11-16-1402-run.md",
            "é026-11-16-1402-run.md",
        ] {
            assert_eq!(parse_filename(name), None, "{name:?}");
        }
    }

    #[test]
    fn format_entry() {
        assert_eq!(
            format_filename(date(2026, 11, 16), time(14, 2), "morning-run"),
            "2026-11-16-1402-morning-run.md"
        );
        assert_eq!(
            format_filename(date(987, 1, 2), time(0, 0), "x"),
            "0987-01-02-0000-x.md"
        );
        let with_seconds = NaiveTime::from_hms_milli_opt(9, 5, 59, 999).unwrap();
        assert_eq!(
            format_filename(date(2026, 1, 1), with_seconds, "x"),
            "2026-01-01-0905-x.md"
        );
    }

    #[test]
    fn parse_format_roundtrip() {
        for (d, t, slug) in [
            (date(2026, 11, 16), time(14, 2), "morning-run"),
            (date(2024, 2, 29), time(23, 59), "leap-day-2"),
            (date(1, 1, 1), time(0, 0), "a"),
            (date(9999, 12, 31), time(12, 30), "end"),
        ] {
            let name = format_filename(d, t, slug);
            let e = parse_filename(&name).unwrap();
            assert_eq!((e.date, e.time, e.slug.as_str()), (d, t, slug));
            assert_eq!(format_filename(e.date, e.time, &e.slug), name);
        }
    }

    #[test]
    fn word_count_ignores_markdown_punctuation() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("--- *** ### > - [ ] [x]"), 0);
        assert_eq!(
            word_count("# Title\n\n- one *two* **three**\n- [ ] four"),
            5
        );
        assert_eq!(word_count("> quoted _text_ ~~gone~~ `code`"), 4);
        assert_eq!(word_count("don't stop"), 2);
        assert_eq!(word_count("pi is 3.14, not 1,000"), 5);
        assert_eq!(word_count("line one\r\nline two\r\n"), 4);
    }

    #[test]
    fn word_count_ignores_task_marks() {
        assert_eq!(word_count("- [ ] buy milk"), word_count("- [x] buy milk"));
        assert_eq!(word_count("- [X] buy milk\n  - [x] and eggs"), 4);
        assert_eq!(word_count("[x]"), 0);
        // Only a lone x in brackets is a mark.
        assert_eq!(word_count("x [xy] [x x] x]"), 5);
    }

    #[test]
    fn word_count_skips_link_and_image_destinations() {
        assert_eq!(
            word_count("Ran 5km. ![](assets/2026-11-16-0800-run-1.jpg)"),
            2
        );
        assert_eq!(word_count("![a sunset](x.jpg) over the bay"), 5);
        assert_eq!(
            word_count("see [the docs](https://example.com/a-b-c \"Title\") now"),
            4
        );
        assert_eq!(word_count("[a](b)[c](d)"), 2);
        // Up to the first `)`: the rest is punctuation.
        assert_eq!(word_count("[Rust](https://w.org/Rust_(lang)) end"), 2);
        // Link text "x" is a word, unlike the task mark after it.
        assert_eq!(word_count("[x](https://x.org) [x] done"), 2);
        // Without a `)` on the same line, nothing is skipped.
        assert_eq!(word_count("a](b c\nd)"), 4);
        assert_eq!(word_count("a](b c"), 3);
        assert_eq!(word_count("]("), 0);
        assert_eq!(word_count("one ](two) three\r\n"), 2);
    }

    #[test]
    fn word_count_unicode() {
        assert_eq!(word_count("Café crème"), 2);
        assert_eq!(word_count("Привет мир"), 2);
        assert_eq!(word_count("日本語"), 3);
        assert_eq!(word_count("🦀 🎉"), 0);
    }
}
