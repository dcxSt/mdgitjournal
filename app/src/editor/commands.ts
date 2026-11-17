// State-level editing helpers. Each returns a transaction spec and never touches text outside
// the lines or ranges it is asked to change.

import { EditorSelection, type EditorState, type Line, type TransactionSpec } from "@codemirror/state";

/** Wraps each selection in `before`/`after`, or unwraps it if it is already wrapped. */
export function wrapSelection(state: EditorState, before: string, after: string): TransactionSpec {
  return state.changeByRange((range) => {
    const { from, to } = range;
    const wrapped =
      from >= before.length &&
      state.sliceDoc(from - before.length, from) === before &&
      state.sliceDoc(to, to + after.length) === after;
    const shift = wrapped ? -before.length : before.length;
    return {
      changes: wrapped
        ? [
            { from: from - before.length, to: from },
            { from: to, to: to + after.length },
          ]
        : [
            { from, insert: before },
            { from: to, insert: after },
          ],
      range: EditorSelection.range(range.anchor + shift, range.head + shift),
    };
  });
}

// Indentation and blockquote markers, then an optional heading or list prefix.
const LINE_PREFIX = /^((?:[ \t]*>)*[ \t]*)(#{1,6} |[-*+] \[[ xX]\] |[-*+] |\d+[.)] )?/;

/** Prefixes of the same kind toggle each other off: any bullet, any task, any ordered marker. */
function prefixKind(prefix: string): string {
  if (/^[-*+] \[[ xX]\] $/.test(prefix)) return "task";
  if (/^[-*+] $/.test(prefix)) return "bullet";
  if (/^\d+[.)] $/.test(prefix)) return "ordered";
  return prefix;
}

/**
 * Toggles a block prefix ("# ", "- ", "- [ ] ") on every selected line, after its indentation
 * and quote markers. If every line already has it, it is removed; otherwise lines without it get
 * it, replacing a different heading or list prefix. Blank lines inside a multi-line selection
 * are left alone.
 */
export function toggleLinePrefix(state: EditorState, prefix: string): TransactionSpec {
  const kind = prefixKind(prefix);
  let lines = selectedLines(state);
  if (lines.length > 1) lines = lines.filter((line) => line.text.trim() !== "");
  const targets = lines.map((line) => {
    const [, lead, existing = ""] = LINE_PREFIX.exec(line.text)!;
    const from = line.from + lead.length;
    return { from, to: from + existing.length, has: existing !== "" && prefixKind(existing) === kind };
  });
  const remove = targets.length > 0 && targets.every((t) => t.has);
  const changes = state.changes(
    targets
      .filter((t) => remove || !t.has)
      .map(({ from, to }) => ({ from, to, insert: remove ? "" : prefix })),
  );
  const selection = EditorSelection.create(
    state.selection.ranges.map((range) => {
      // A cursor moves past a new prefix; a selection grows to take in one added at its start.
      const from = changes.mapPos(range.from, range.empty ? 1 : -1);
      const to = changes.mapPos(range.to, 1);
      const [anchor, head] = range.anchor <= range.head ? [from, to] : [to, from];
      return EditorSelection.range(anchor, head);
    }),
    state.selection.mainIndex,
  );
  return { changes, selection };
}

/** Lines touched by any selection range; a range ending at a line start excludes that line. */
function selectedLines(state: EditorState): Line[] {
  const lines: Line[] = [];
  for (const range of state.selection.ranges) {
    const first = state.doc.lineAt(range.from).number;
    const end = state.doc.lineAt(range.to);
    const last = range.to > range.from && end.from === range.to ? end.number - 1 : end.number;
    for (let n = Math.max(first, (lines.at(-1)?.number ?? 0) + 1); n <= last; n++) {
      lines.push(state.doc.line(n));
    }
  }
  return lines;
}

/** Inserts one `![](path)` line per path at `pos`, breaking the surrounding line if needed. */
export function insertImageLinks(state: EditorState, pos: number, paths: string[]): TransactionSpec {
  const line = state.doc.lineAt(pos);
  const lb = state.lineBreak;
  const links = (pos > line.from ? lb : "") + paths.map((path) => `![](${path})`).join(lb);
  const insert = links + (pos < line.to ? lb : "");
  return {
    changes: { from: pos, insert },
    selection: { anchor: pos + state.toText(links).length },
    scrollIntoView: true,
    userEvent: "input",
  };
}
