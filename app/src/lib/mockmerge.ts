// Line diffs for the mock backend (mock.ts): a three-way merge into the backend's `MergeChunk`s, the
// conflict markers `git merge` writes, and unified diff hunks. The real backend uses libgit2's
// xdiff; this is a plain LCS, which is enough to drive the UI. Mock only.

import type { DiffHunk, DiffLine, MergeChunk } from "./types";

/** Lines including their line break, as Rust's `split_inclusive('\n')`: "\r\n" and a missing last newline survive. */
export function splitLines(text: string): string[] {
  const lines: string[] = [];
  let start = 0;
  for (let i = text.indexOf("\n"); i >= 0; i = text.indexOf("\n", start)) {
    lines.push(text.slice(start, i + 1));
    start = i + 1;
  }
  if (start < text.length) lines.push(text.slice(start));
  return lines;
}

/** `a[start, end)` replaced by `lines`. */
interface Edit {
  start: number;
  end: number;
  lines: string[];
}

type Op = { kind: "same"; a: number; b: number } | { kind: "del"; a: number } | { kind: "ins"; b: number };

/** An edit script from a longest common subsequence of whole lines. */
function ops(a: string[], b: string[]): Op[] {
  // The common prefix and suffix need no table.
  let pre = 0;
  while (pre < a.length && pre < b.length && a[pre] === b[pre]) pre++;
  let suf = 0;
  while (suf < a.length - pre && suf < b.length - pre && a[a.length - 1 - suf] === b[b.length - 1 - suf]) suf++;
  const n = a.length - pre - suf;
  const m = b.length - pre - suf;
  // lcs[i * (m + 1) + j] = LCS length of a[pre + i ..] and b[pre + j ..] (middle parts only).
  const lcs = new Uint32Array((n + 1) * (m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i * (m + 1) + j] =
        a[pre + i] === b[pre + j]
          ? lcs[(i + 1) * (m + 1) + j + 1] + 1
          : Math.max(lcs[(i + 1) * (m + 1) + j], lcs[i * (m + 1) + j + 1]);
    }
  }
  const out: Op[] = [];
  for (let k = 0; k < pre; k++) out.push({ kind: "same", a: k, b: k });
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[pre + i] === b[pre + j]) {
      out.push({ kind: "same", a: pre + i++, b: pre + j++ });
    } else if (j >= m || (i < n && lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1])) {
      out.push({ kind: "del", a: pre + i++ });
    } else {
      out.push({ kind: "ins", b: pre + j++ });
    }
  }
  for (let k = 0; k < suf; k++) out.push({ kind: "same", a: a.length - suf + k, b: b.length - suf + k });
  return out;
}

/** The changed regions of `a` → `b`, in order; each is separated from the next by at least one unchanged line. */
function edits(a: string[], b: string[]): Edit[] {
  const result: Edit[] = [];
  let current: Edit | null = null;
  let posA = 0;
  for (const op of ops(a, b)) {
    if (op.kind === "same") {
      if (current) result.push(current);
      current = null;
      posA = op.a + 1;
      continue;
    }
    current ??= { start: posA, end: posA, lines: [] };
    if (op.kind === "del") {
      current.end = op.a + 1;
      posA = op.a + 1;
    } else {
      current.lines.push(b[op.b]);
    }
  }
  if (current) result.push(current);
  return result;
}

/** `base[start, end)` with `changes` (inside that range, in order) applied. */
function applyEdits(base: string[], start: number, end: number, changes: Edit[]): string {
  let out = "";
  let pos = start;
  for (const e of changes) {
    out += base.slice(pos, e.start).join("") + e.lines.join("");
    pos = e.end;
  }
  return out + base.slice(pos, end).join("");
}

/**
 * The three-way merge as mdgj-git's `diff3.rs` describes it: changes from the two sides that
 * overlap or touch form one group, which is a clash unless both made the same change. Joining
 * the chunks (a clash contributing its pick) gives the merged file byte for byte.
 */
export function diff3(base: string, mine: string, theirs: string): MergeChunk[] {
  const b = splitLines(base);
  const all = [
    ...edits(b, splitLines(mine)).map((e) => ({ ...e, side: "mine" as const })),
    ...edits(b, splitLines(theirs)).map((e) => ({ ...e, side: "theirs" as const })),
  ].sort((x, y) => x.start - y.start || x.end - y.end);

  const chunks: MergeChunk[] = [];
  let pos = 0;
  for (let i = 0; i < all.length; ) {
    const start = all[i].start;
    let end = all[i].end;
    const group = [all[i++]];
    // Touching counts: an edit starting where the group ends clashes with it, as in git.
    while (i < all.length && all[i].start <= end) {
      end = Math.max(end, all[i].end);
      group.push(all[i++]);
    }
    if (start > pos) chunks.push({ kind: "same", text: b.slice(pos, start).join("") });
    const ours = group.filter((e) => e.side === "mine");
    const theirsEdits = group.filter((e) => e.side === "theirs");
    const baseText = b.slice(start, end).join("");
    const mineText = applyEdits(b, start, end, ours);
    const theirsText = applyEdits(b, start, end, theirsEdits);
    if (!theirsEdits.length) chunks.push({ kind: "mine", text: mineText, base: baseText });
    else if (!ours.length) chunks.push({ kind: "theirs", text: theirsText, base: baseText });
    else if (mineText === theirsText) chunks.push({ kind: "both", text: mineText, base: baseText });
    else chunks.push({ kind: "clash", base: baseText, mine: mineText, theirs: theirsText });
    pos = end;
  }
  if (pos < b.length) chunks.push({ kind: "same", text: b.slice(pos).join("") });
  return chunks;
}

/** The merged text if nothing clashes, else null. */
export function cleanMerge(chunks: MergeChunk[]): string | null {
  let out = "";
  for (const c of chunks) {
    if (c.kind === "clash") return null;
    out += c.text;
  }
  return out;
}

/** What `git merge` leaves in a file whose changes clash: the standard conflict markers. */
export function withConflictMarkers(chunks: MergeChunk[], theirsLabel = "origin/main"): string {
  const ended = (text: string) => (text === "" || text.endsWith("\n") ? text : `${text}\n`);
  let out = "";
  for (const c of chunks) {
    out +=
      c.kind === "clash"
        ? `<<<<<<< HEAD\n${ended(c.mine)}=======\n${ended(c.theirs)}>>>>>>> ${theirsLabel}\n`
        : c.text;
  }
  return out;
}

const CONTEXT = 3;

/** Unified diff hunks with three lines of context, line breaks stripped; a changed line ending counts as a change. */
export function unifiedHunks(oldText: string, newText: string): DiffHunk[] {
  const a = splitLines(oldText);
  const b = splitLines(newText);
  const strip = (line: string) => line.replace(/\r?\n$/, "");
  const script = ops(a, b);
  const lines: (DiffLine & { a: number; b: number })[] = [];
  // a, b = how many old and new lines come before this one.
  let na = 0;
  let nb = 0;
  for (const op of script) {
    if (op.kind === "same") lines.push({ kind: "context", text: strip(a[op.a]), a: na++, b: nb++ });
    else if (op.kind === "del") lines.push({ kind: "removed", text: strip(a[op.a]), a: na++, b: nb });
    else lines.push({ kind: "added", text: strip(b[op.b]), a: na, b: nb++ });
  }

  const hunks: DiffHunk[] = [];
  let i = 0;
  while (i < lines.length) {
    while (i < lines.length && lines[i].kind === "context") i++;
    if (i >= lines.length) break;
    const first = Math.max(0, i - CONTEXT);
    let last = i;
    // Extend over changes separated by at most 2 × CONTEXT unchanged lines, as git does.
    for (let k = i; k < lines.length; k++) {
      if (lines[k].kind !== "context") last = k;
      else if (k - last > 2 * CONTEXT) break;
    }
    const end = Math.min(lines.length, last + 1 + CONTEXT);
    const body = lines.slice(first, end);
    const oldLines = body.filter((l) => l.kind !== "added").length;
    const newLines = body.filter((l) => l.kind !== "removed").length;
    hunks.push({
      // git numbers from 1, and an empty side by the line before it.
      oldStart: oldLines ? body[0].a + 1 : body[0].a,
      oldLines,
      newStart: newLines ? body[0].b + 1 : body[0].b,
      newLines,
      lines: body.map(({ kind, text }) => ({ kind, text })),
    });
    i = end;
  }
  return hunks;
}
