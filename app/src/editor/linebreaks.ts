// How a file's line breaks survive editing. CodeMirror counts a line break as one position and
// joins lines with a single string, so the file's own break is chosen once, at load time.

import { type ChangeSpec, EditorState, type Extension, Facet } from "@codemirror/state";

const fileBreak = Facet.define<string, string>({ combine: (values) => values[0] ?? "\n" });

/**
 * In a file split on "\n" alone, the "\r" of a CRLF line is the last character of its line, and
 * End puts the cursor after it. Input never separates it from its "\n":
 * - What is typed or pasted after it goes in before it.
 * - What replaces the end of the line, "\r" included (typing over a selection to the line end,
 *   or Enter, which drops the spaces after the cursor), gets it back at its end; a new line
 *   gets it back before the new line break, so the line above keeps its CRLF.
 * A new line typed after the "\r" already keeps it next to a "\n".
 */
const keepCrlf = EditorState.transactionFilter.of((tr) => {
  if (!tr.docChanged || !tr.isUserEvent("input")) return tr;
  const doc = tr.startState.doc;
  const repairs: ChangeSpec[] = [];
  tr.changes.iterChanges((fromA, toA, fromB, toB, inserted) => {
    const line = doc.lineAt(toA);
    if (toA !== line.to || !line.text.endsWith("\r")) return;
    const newLine = inserted.sliceString(0, 1) === "\n";
    if (fromA < toA) repairs.push({ from: newLine ? fromB : toB, insert: "\r" });
    else if (!newLine) repairs.push({ from: fromB - 1, to: fromB }, { from: toB, insert: "\r" });
  });
  return repairs.length ? [tr, { changes: repairs, sequential: true }] : tr;
});

/**
 * A file that uses one kind of break throughout is edited as plain lines and written back with
 * that break, so nothing that inserts text (paste, vim registers, IME) can mix in another. A file
 * that already mixes them is split on "\n" alone and keeps every "\r" as a visible character.
 */
export function lineBreaks(doc: string): Extension {
  const used = new Set(doc.match(/\r\n?|\n/g));
  if (used.size > 1) return [EditorState.lineSeparator.of("\n"), keepCrlf];
  return fileBreak.of(used.values().next().value ?? "\n");
}

/** The document as the file's bytes: exactly the text it was loaded with, plus the edits. */
export const fileText = (state: EditorState): string =>
  state.doc.sliceString(0, state.doc.length, state.facet(fileBreak));

/** Rewrites every line break in `text` to the editor's own, before it is inserted. */
export const normalizeBreaks = (text: string, state: EditorState): string =>
  text.replace(/\r\n?|\n/g, state.lineBreak);
