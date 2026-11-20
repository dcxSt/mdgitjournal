//! One changed file against HEAD, as the Commit sheet shows it: git's own unified diff.

use git2::{DiffOptions, Patch};
use serde::Serialize;

use crate::pull::text;
use crate::repo::{changes, head_tree_id, lossy};
use crate::worktree::{tree_files, Snapshot};
use crate::{ChangeKind, Error, Repo, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    /// Without its line break.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    /// A side isn't UTF-8 text (or holds a NUL byte, or isn't a regular file): no hunks.
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

impl Repo {
    /// The diff of `path`, one of the changes `status` lists, against HEAD.
    pub fn diff(&self, path: &str) -> Result<FileDiff> {
        let snapshot = Snapshot::take(&self.git)?;
        let git = &snapshot.repo;
        let head_tree = head_tree_id(git)?;
        let change = changes(git, head_tree, snapshot.tree)?
            .into_iter()
            .find(|change| lossy(&change.path) == path)
            .ok_or_else(|| Error::Invalid(format!("\"{path}\" has no changes to show.")))?;
        let head = tree_files(&git.find_tree(head_tree)?)?;
        let old = match change.kind {
            ChangeKind::Added => None,
            _ => head.get(change.old_path.as_ref().unwrap_or(&change.path)),
        };
        let new = match change.kind {
            ChangeKind::Deleted => None,
            _ => snapshot.files.get(&change.path),
        };
        let old = old.map(|&(id, mode)| text(git, id, mode)).transpose()?;
        let new = new.map(|&(id, mode)| text(git, id, mode)).transpose()?;
        let (binary, hunks) = match (old, new) {
            (Some(None), _) | (_, Some(None)) => (true, Vec::new()),
            (old, new) => (
                false,
                hunks(
                    old.flatten().as_deref().unwrap_or(""),
                    new.flatten().as_deref().unwrap_or(""),
                )?,
            ),
        };
        Ok(FileDiff {
            path: lossy(&change.path),
            old_path: change.old_path.as_deref().map(lossy),
            kind: change.kind,
            binary,
            hunks,
        })
    }
}

fn hunks(old: &str, new: &str) -> Result<Vec<DiffHunk>> {
    let mut options = DiffOptions::new();
    options
        .context_lines(3)
        .force_text(true)
        .indent_heuristic(true);
    let patch = Patch::from_buffers(
        old.as_bytes(),
        None,
        new.as_bytes(),
        None,
        Some(&mut options),
    )?;
    (0..patch.num_hunks())
        .map(|h| {
            let (hunk, count) = patch.hunk(h)?;
            let mut lines = Vec::new();
            for l in 0..count {
                let line = patch.line_in_hunk(h, l)?;
                let kind = match line.origin() {
                    ' ' => LineKind::Context,
                    '+' => LineKind::Added,
                    '-' => LineKind::Removed,
                    // "\ No newline at end of file" and friends.
                    _ => continue,
                };
                let content = String::from_utf8_lossy(line.content());
                let text = content.strip_suffix('\n').unwrap_or(&content);
                let text = text.strip_suffix('\r').unwrap_or(text);
                lines.push(DiffLine {
                    kind,
                    text: text.to_owned(),
                });
            }
            Ok(DiffHunk {
                old_start: hunk.old_start(),
                old_lines: hunk.old_lines(),
                new_start: hunk.new_start(),
                new_lines: hunk.new_lines(),
                lines,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(hunk: &DiffHunk) -> String {
        hunk.lines
            .iter()
            .map(|line| match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
            })
            .collect()
    }

    #[test]
    fn hunks_have_three_lines_of_context() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let new = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n";
        let hunks = hunks(old, new).unwrap();
        assert_eq!(hunks.len(), 1);
        let hunk = &hunks[0];
        assert_eq!(
            (
                hunk.old_start,
                hunk.old_lines,
                hunk.new_start,
                hunk.new_lines
            ),
            (2, 7, 2, 7)
        );
        assert_eq!(kinds(hunk), "   -+   ");
        assert_eq!(hunk.lines[3].text, "5");
        assert_eq!(hunk.lines[4].text, "five");
    }

    #[test]
    fn line_endings_and_final_newlines_show_as_changes() {
        let hunks = hunks("a\nb\n", "a\r\nb").unwrap();
        let lines: Vec<_> = hunks[0]
            .lines
            .iter()
            .map(|line| (kinds_of(line.kind), line.text.as_str()))
            .collect();
        assert_eq!(lines, [('-', "a"), ('-', "b"), ('+', "a"), ('+', "b")]);
    }

    fn kinds_of(kind: LineKind) -> char {
        match kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
        }
    }
}
