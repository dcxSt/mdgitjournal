//! The Sync review's suggested merge of one text file: which lines changed on which side since the
//! common version, and where the two sides clash.
//!
//! Lines keep their line breaks (`split_inclusive('\n')`), so `\r\n` and a missing final newline
//! survive, and concatenating the chunks gives the merged file byte for byte. The line diffs are
//! libgit2's (git's own xdiff, Myers); two changes clash when they overlap or touch, as in git.

use std::ops::Range;

use git2::{DiffOptions, Patch};
use serde::Serialize;

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeSuggestion {
    /// No chunk clashes: the merge is ready as it is.
    pub clean: bool,
    pub chunks: Vec<MergeChunk>,
}

/// A run of whole lines of the merge result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MergeChunk {
    /// Unchanged on both sides.
    Same { text: String },
    /// Changed here only, and already applied; `text` is "" where lines were deleted.
    Mine { text: String, base: String },
    /// Changed on GitHub only, and already applied.
    Theirs { text: String, base: String },
    /// The same change on both sides.
    Both { text: String, base: String },
    /// Different changes to the same lines (or neighbouring ones): the user picks.
    Clash {
        base: String,
        mine: String,
        theirs: String,
    },
}

impl MergeSuggestion {
    /// The merged text, or `None` while a chunk clashes.
    pub fn text(&self) -> Option<String> {
        self.chunks
            .iter()
            .map(|chunk| match chunk {
                MergeChunk::Same { text }
                | MergeChunk::Mine { text, .. }
                | MergeChunk::Theirs { text, .. }
                | MergeChunk::Both { text, .. } => Some(text.as_str()),
                MergeChunk::Clash { .. } => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Mine,
    Theirs,
}

/// One side's change: base lines `base` became that side's lines `side` (0-based, half-open).
#[derive(Debug, Clone)]
struct Hunk {
    side: Side,
    base: Range<usize>,
    lines: Range<usize>,
}

/// Splits `base`, `mine` and `theirs` into the chunks of their three-way merge.
pub(crate) fn suggest(base: &str, mine: &str, theirs: &str) -> Result<MergeSuggestion> {
    let base_lines = lines(base);
    let mine_lines = lines(mine);
    let theirs_lines = lines(theirs);
    let mut edits = hunks(Side::Mine, base, mine)?;
    edits.extend(hunks(Side::Theirs, base, theirs)?);
    edits.sort_by_key(|hunk| (hunk.base.start, hunk.base.end));

    let mut chunks = Vec::new();
    let mut done = 0;
    let mut next = 0;
    while next < edits.len() {
        // A group: changes that overlap or touch, from either side.
        let start = edits[next].base.start;
        let mut end = edits[next].base.end;
        let first = next;
        next += 1;
        while next < edits.len() && edits[next].base.start <= end {
            end = end.max(edits[next].base.end);
            next += 1;
        }
        let group = &edits[first..next];
        if done < start {
            chunks.push(MergeChunk::Same {
                text: base_lines[done..start].concat(),
            });
        }
        let text_of = |side: Side, side_lines: &[&str]| {
            let mut text = String::new();
            let mut at = start;
            for hunk in group.iter().filter(|hunk| hunk.side == side) {
                text.push_str(&base_lines[at..hunk.base.start].concat());
                text.push_str(&side_lines[hunk.lines.clone()].concat());
                at = hunk.base.end;
            }
            text.push_str(&base_lines[at..end].concat());
            text
        };
        let base = base_lines[start..end].concat();
        let has = |side: Side| group.iter().any(|hunk| hunk.side == side);
        chunks.push(match (has(Side::Mine), has(Side::Theirs)) {
            (true, false) => MergeChunk::Mine {
                text: text_of(Side::Mine, &mine_lines),
                base,
            },
            (false, _) => MergeChunk::Theirs {
                text: text_of(Side::Theirs, &theirs_lines),
                base,
            },
            (true, true) => {
                let mine = text_of(Side::Mine, &mine_lines);
                let theirs = text_of(Side::Theirs, &theirs_lines);
                if mine == theirs {
                    MergeChunk::Both { text: mine, base }
                } else {
                    MergeChunk::Clash { base, mine, theirs }
                }
            }
        });
        done = end;
    }
    if done < base_lines.len() {
        chunks.push(MergeChunk::Same {
            text: base_lines[done..].concat(),
        });
    }
    let clean = !chunks
        .iter()
        .any(|chunk| matches!(chunk, MergeChunk::Clash { .. }));
    Ok(MergeSuggestion { clean, chunks })
}

fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The changes from `base` to `other`, with no context lines and no merging of nearby changes.
fn hunks(side: Side, base: &str, other: &str) -> Result<Vec<Hunk>> {
    let mut options = DiffOptions::new();
    options.context_lines(0).interhunk_lines(0).force_text(true);
    let patch = Patch::from_buffers(
        base.as_bytes(),
        None,
        other.as_bytes(),
        None,
        Some(&mut options),
    )?;
    (0..patch.num_hunks())
        .map(|i| {
            let (hunk, _) = patch.hunk(i)?;
            Ok(Hunk {
                side,
                base: range(hunk.old_start(), hunk.old_lines()),
                lines: range(hunk.new_start(), hunk.new_lines()),
            })
        })
        .collect()
}

/// A unified-diff line range as 0-based indices. Starts are 1-based, except that an empty range
/// starts at the line before it (0 at the top of the file).
fn range(start: u32, count: u32) -> Range<usize> {
    let (start, count) = (start as usize, count as usize);
    let first = if count == 0 { start } else { start - 1 };
    first..first + count
}

#[cfg(test)]
mod tests {
    use super::*;
    use MergeChunk::*;

    fn s(text: &str) -> String {
        text.to_owned()
    }

    fn merge(base: &str, mine: &str, theirs: &str) -> MergeSuggestion {
        let suggestion = suggest(base, mine, theirs).unwrap();
        check(base, mine, theirs, &suggestion);
        suggestion
    }

    /// Every chunk is consistent with all three inputs: putting each chunk's version of a side
    /// back together gives that side exactly.
    fn check(base: &str, mine: &str, theirs: &str, suggestion: &MergeSuggestion) {
        let side = |pick: fn(&MergeChunk) -> &str| -> String {
            suggestion.chunks.iter().map(pick).collect()
        };
        let as_base = side(|chunk| match chunk {
            Same { text } => text,
            Mine { base, .. } | Theirs { base, .. } | Both { base, .. } | Clash { base, .. } => {
                base
            }
        });
        let as_mine = side(|chunk| match chunk {
            Same { text } | Mine { text, .. } | Both { text, .. } => text,
            Theirs { base, .. } => base,
            Clash { mine, .. } => mine,
        });
        let as_theirs = side(|chunk| match chunk {
            Same { text } | Theirs { text, .. } | Both { text, .. } => text,
            Mine { base, .. } => base,
            Clash { theirs, .. } => theirs,
        });
        assert_eq!(as_base, base, "base");
        assert_eq!(as_mine, mine, "mine");
        assert_eq!(as_theirs, theirs, "theirs");
        assert_eq!(
            suggestion.clean,
            !suggestion.chunks.iter().any(|c| matches!(c, Clash { .. }))
        );
        for pair in suggestion.chunks.windows(2) {
            let same = |chunk: &MergeChunk| matches!(chunk, Same { .. });
            // Exactly one of two neighbours is unchanged: unchanged runs are whole, and changes
            // that touch are one chunk.
            assert!(same(&pair[0]) != same(&pair[1]), "{pair:?}");
        }
        for chunk in &suggestion.chunks {
            match chunk {
                Same { text } => assert!(!text.is_empty()),
                Mine { text, base } | Theirs { text, base } | Both { text, base } => {
                    assert_ne!(text, base, "{chunk:?}")
                }
                Clash { mine, theirs, .. } => assert_ne!(mine, theirs),
            }
        }
    }

    #[test]
    fn different_paragraphs_merge_cleanly() {
        let base = "# Run\n\nFirst paragraph.\n\nSecond paragraph.\n";
        let mine = "# Run\n\nFirst paragraph, edited here.\n\nSecond paragraph.\n";
        let theirs = "# Run\n\nFirst paragraph.\n\nSecond paragraph, edited on GitHub.\n";
        let merged = merge(base, mine, theirs);
        assert!(merged.clean);
        assert_eq!(
            merged.chunks,
            [
                Same {
                    text: s("# Run\n\n")
                },
                Mine {
                    text: s("First paragraph, edited here.\n"),
                    base: s("First paragraph.\n")
                },
                Same { text: s("\n") },
                Theirs {
                    text: s("Second paragraph, edited on GitHub.\n"),
                    base: s("Second paragraph.\n")
                },
            ]
        );
        assert_eq!(
            merged.text().unwrap(),
            "# Run\n\nFirst paragraph, edited here.\n\nSecond paragraph, edited on GitHub.\n"
        );
    }

    #[test]
    fn the_same_line_clashes() {
        let merged = merge("a\nb\nc\n", "a\nB here\nc\n", "a\nB there\nc\n");
        assert!(!merged.clean);
        assert_eq!(merged.text(), None);
        assert_eq!(
            merged.chunks,
            [
                Same { text: s("a\n") },
                Clash {
                    base: s("b\n"),
                    mine: s("B here\n"),
                    theirs: s("B there\n")
                },
                Same { text: s("c\n") },
            ]
        );
    }

    #[test]
    fn neighbouring_lines_clash_as_in_git() {
        let merged = merge("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n");
        assert_eq!(
            merged.chunks,
            [
                Same { text: s("a\n") },
                Clash {
                    base: s("b\nc\n"),
                    mine: s("B\nc\n"),
                    theirs: s("b\nC\n")
                },
                Same { text: s("d\n") },
            ]
        );
        // An insertion right next to the other side's change clashes too.
        let merged = merge("a\nb\nc\n", "a\nnew\nb\nc\n", "a\nB\nc\n");
        assert_eq!(
            merged.chunks,
            [
                Same { text: s("a\n") },
                Clash {
                    base: s("b\n"),
                    mine: s("new\nb\n"),
                    theirs: s("B\n")
                },
                Same { text: s("c\n") },
            ]
        );
        // One unchanged line between them is enough to merge.
        assert!(merge("a\nb\nc\n", "A\nb\nc\n", "a\nb\nC\n").clean);
    }

    #[test]
    fn identical_changes_are_both() {
        let merged = merge("a\nb\nc\n", "a\nB\nc\nmine\n", "a\nB\nc\n");
        assert_eq!(
            merged.chunks,
            [
                Same { text: s("a\n") },
                Both {
                    text: s("B\n"),
                    base: s("b\n")
                },
                Same { text: s("c\n") },
                Mine {
                    text: s("mine\n"),
                    base: s("")
                },
            ]
        );
        assert_eq!(merged.text().unwrap(), "a\nB\nc\nmine\n");
    }

    #[test]
    fn deletions_have_empty_text_and_keep_their_base() {
        let merged = merge("a\nb\nc\nd\ne\n", "a\nc\nd\ne\n", "a\nb\nc\nd\n");
        assert_eq!(
            merged.chunks,
            [
                Same { text: s("a\n") },
                Mine {
                    text: s(""),
                    base: s("b\n")
                },
                Same { text: s("c\nd\n") },
                Theirs {
                    text: s(""),
                    base: s("e\n")
                },
            ]
        );
        assert_eq!(merged.text().unwrap(), "a\nc\nd\n");
    }

    #[test]
    fn crlf_and_missing_final_newlines_survive() {
        let base = "one\r\ntwo\r\nthree\r\nfour";
        let mine = "ONE\r\ntwo\r\nthree\r\nfour";
        let theirs = "one\r\ntwo\r\nthree\r\nfour, no newline either";
        let merged = merge(base, mine, theirs);
        assert!(merged.clean);
        assert_eq!(
            merged.text().unwrap(),
            "ONE\r\ntwo\r\nthree\r\nfour, no newline either"
        );
        // Only a line ending changed: that is a change too.
        let merged = merge("a\nb\n", "a\r\nb\n", "a\nb\n");
        assert_eq!(merged.text().unwrap(), "a\r\nb\n");
        // Adding a final newline changes the last line.
        let merged = merge("a\nb", "a\nb\n", "a\nb");
        assert_eq!(merged.text().unwrap(), "a\nb\n");
    }

    #[test]
    fn files_added_on_both_sides_are_one_clash() {
        let merged = merge("", "mine\nshared\n", "theirs\nshared\n");
        assert_eq!(
            merged.chunks,
            [Clash {
                base: s(""),
                mine: s("mine\nshared\n"),
                theirs: s("theirs\nshared\n")
            }]
        );
        assert_eq!(
            merge("", "", "x\n").chunks,
            [Theirs {
                text: s("x\n"),
                base: s("")
            }]
        );
    }

    #[test]
    fn empty_and_unchanged_inputs() {
        assert_eq!(merge("", "", "").chunks, []);
        assert!(merge("", "", "").clean);
        assert_eq!(merge("a\n", "a\n", "a\n").chunks, [Same { text: s("a\n") }]);
        assert_eq!(
            merge("a\n", "", "").chunks,
            [Both {
                text: s(""),
                base: s("a\n")
            }]
        );
    }

    /// A small deterministic generator, so the property test needs no extra crate.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn mutate(rng: &mut Rng, base: &[&str]) -> String {
        const WORDS: [&str; 6] = ["a\n", "b\n", "c\r\n", "x\n", "y", "\n"];
        let mut out = Vec::new();
        for line in base {
            match rng.below(6) {
                0 => {}
                1 => out.push(WORDS[rng.below(WORDS.len())]),
                2 => {
                    out.push(WORDS[rng.below(WORDS.len())]);
                    out.push(line);
                }
                _ => out.push(line),
            }
        }
        if rng.below(4) == 0 {
            out.push(WORDS[rng.below(WORDS.len())]);
        }
        out.concat()
    }

    #[test]
    fn chunks_always_rebuild_every_side_byte_for_byte() {
        const WORDS: [&str; 5] = ["a\n", "b\n", "c\r\n", "a\n", "\n"];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..3000 {
            let len = rng.below(8);
            let mut base: Vec<&str> = (0..len).map(|_| WORDS[rng.below(WORDS.len())]).collect();
            if rng.below(5) == 0 {
                base.push("no newline");
            }
            let mine = mutate(&mut rng, &base);
            let theirs = mutate(&mut rng, &base);
            let base = base.concat();
            let suggestion = suggest(&base, &mine, &theirs).unwrap();
            check(&base, &mine, &theirs, &suggestion);
            if mine == base {
                assert_eq!(suggestion.text().unwrap(), theirs);
            }
            if theirs == base {
                assert_eq!(suggestion.text().unwrap(), mine);
            }
        }
    }
}
