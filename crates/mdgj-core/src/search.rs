//! Full-text search: a case-insensitive substring scan over file names and lines.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::journal::{stem, title};
use crate::{Journal, Result};

/// Characters of context kept on each side of a match.
const CONTEXT: usize = 40;
const ELLIPSIS: char = '…';

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub file: String,
    pub title: String,
    /// 1-based line of the match; 0 for a match in the file name.
    pub line: u32,
    /// The matching line (or file stem), cut to 40 characters either side of the match, with
    /// `…` where it was cut.
    pub snippet: String,
    /// The match within `snippet`, in chars (Unicode scalar values), end exclusive.
    pub match_start: u32,
    pub match_end: u32,
}

impl Journal {
    /// Finds `query` in file stems and lines, ignoring case, in [`Journal::list`] order (newest
    /// entry first). Each file gives at most one hit for its name and one per line. A blank
    /// query finds nothing.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let mut hits = Vec::new();
        if query.trim().is_empty() || limit == 0 {
            return Ok(hits);
        }
        let needle = fold(query);
        for file in self.files()? {
            let title = title(&file);
            let hit = |line: usize, text: &str| {
                find(text, &needle).map(|(snippet, match_start, match_end)| SearchHit {
                    file: file.clone(),
                    title: title.clone(),
                    line: u32::try_from(line).unwrap_or(u32::MAX),
                    snippet,
                    match_start,
                    match_end,
                })
            };
            hits.extend(hit(0, stem(&file)));
            // Unreadable or non-UTF-8 files can still match by name.
            if let Ok(content) = fs::read_to_string(self.root().join(&file)) {
                if fold(&content).contains(&needle) {
                    let lines = content.lines().enumerate();
                    hits.extend(
                        lines
                            .filter_map(|(i, text)| hit(i + 1, text))
                            .take(limit - hits.len()),
                    );
                }
            }
            if hits.len() == limit {
                break;
            }
        }
        Ok(hits)
    }
}

/// Lowercases one char at a time, so every folded char maps back to one original char.
fn fold_char(c: char) -> impl Iterator<Item = char> {
    // Σ lowercases to σ, but a typed final ς should match it too.
    c.to_lowercase().map(|c| if c == 'ς' { 'σ' } else { c })
}

fn fold(text: &str) -> String {
    text.chars().flat_map(fold_char).collect()
}

/// The first case-insensitive match of `needle` (already folded, non-empty) in `text`, as a
/// snippet with the match's char range in it.
fn find(text: &str, needle: &str) -> Option<(String, u32, u32)> {
    let mut folded = String::with_capacity(text.len());
    // For each char of `folded`, the index of the char of `text` it came from.
    let mut origin = Vec::with_capacity(text.len());
    for (i, c) in text.chars().enumerate() {
        for f in fold_char(c) {
            folded.push(f);
            origin.push(i);
        }
    }
    let at = folded.find(needle)?;
    let first = folded[..at].chars().count();
    let last = first + needle.chars().count() - 1;
    Some(snippet(text, origin[first], origin[last] + 1))
}

/// `text` cut to [`CONTEXT`] chars either side of the char range `start..end`, and that range
/// within the result.
fn snippet(text: &str, start: usize, end: usize) -> (String, u32, u32) {
    let chars: Vec<char> = text.chars().collect();
    let from = start.saturating_sub(CONTEXT);
    let to = (end + CONTEXT).min(chars.len());
    let mut snippet = String::new();
    if from > 0 {
        snippet.push(ELLIPSIS);
    }
    snippet.extend(&chars[from..to]);
    if to < chars.len() {
        snippet.push(ELLIPSIS);
    }
    let shift = from - usize::from(from > 0);
    let offset = |i: usize| u32::try_from(i - shift).unwrap_or(u32::MAX);
    (snippet, offset(start), offset(end))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The matched text, sliced by the returned char offsets.
    fn matched(text: &str, needle: &str) -> Option<String> {
        let (snippet, start, end) = find(text, &fold(needle))?;
        Some(
            snippet
                .chars()
                .skip(start as usize)
                .take((end - start) as usize)
                .collect(),
        )
    }

    #[test]
    fn finds_case_insensitively() {
        assert_eq!(matched("Morning Run", "run").as_deref(), Some("Run"));
        assert_eq!(matched("ÉTÉ À PARIS", "été").as_deref(), Some("ÉTÉ"));
        assert_eq!(matched("Straße", "STRASSE"), None); // lowercase, not full case folding
        assert_eq!(matched("nothing here", "run"), None);
    }

    #[test]
    fn final_sigma_matches_both_ways() {
        assert_eq!(matched("ΟΔΟΣ", "οδος").as_deref(), Some("ΟΔΟΣ"));
        assert_eq!(matched("οδος", "ΟΔΟΣ").as_deref(), Some("οδος"));
    }

    #[test]
    fn offsets_survive_lowercase_expansion() {
        // 'İ' lowercases to two chars, which must not shift the match.
        let (snippet, start, end) = find("İİ abc", "abc").unwrap();
        assert_eq!((snippet.as_str(), start, end), ("İİ abc", 3, 6));
        // A match inside the expansion maps to the whole original char.
        assert_eq!(matched("xİy", "i").as_deref(), Some("İ"));
    }

    #[test]
    fn short_lines_are_not_cut() {
        assert_eq!(find("a run b", "run"), Some(("a run b".to_owned(), 2, 5)));
    }

    #[test]
    fn long_lines_are_cut_on_char_boundaries() {
        let before = "é日🦀".repeat(30); // 90 chars, 270 bytes
        let text = format!("{before}NEEDLE{}", "ü".repeat(100));
        let (snippet, start, end) = find(&text, "needle").unwrap();
        let context: String = before.chars().skip(90 - CONTEXT).collect();
        let expected = format!("…{context}NEEDLE{}…", "ü".repeat(CONTEXT));
        assert_eq!(snippet, expected);
        assert_eq!((start, end), (1 + CONTEXT as u32, 1 + CONTEXT as u32 + 6));
    }

    #[test]
    fn cut_on_one_side_only() {
        let text = format!("start needle{}", "x".repeat(100));
        let (snippet, start, end) = find(&text, "needle").unwrap();
        assert!(snippet.starts_with("start needle") && snippet.ends_with(ELLIPSIS));
        assert_eq!((start, end), (6, 12));

        let text = format!("{}needle end", "x".repeat(100));
        let (snippet, start, end) = find(&text, "needle").unwrap();
        assert!(snippet.starts_with(ELLIPSIS) && snippet.ends_with("needle end"));
        assert_eq!((start, end), (1 + CONTEXT as u32, 7 + CONTEXT as u32));
    }
}
