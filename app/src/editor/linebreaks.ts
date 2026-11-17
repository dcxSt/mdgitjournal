// How a file's line breaks survive editing. CodeMirror counts a line break as one position and
// joins lines with a single string, so the file's own break is chosen once, at load time.

import { EditorState, type Extension, Facet } from "@codemirror/state";

const fileBreak = Facet.define<string, string>({ combine: (values) => values[0] ?? "\n" });

/**
 * A file that uses one kind of break throughout is edited as plain lines and written back with
 * that break, so nothing that inserts text (paste, vim registers, IME) can mix in another. A file
 * that already mixes them is split on "\n" alone and keeps every "\r" as a visible character.
 */
export function lineBreaks(doc: string): Extension {
  const used = new Set(doc.match(/\r\n?|\n/g));
  if (used.size > 1) return EditorState.lineSeparator.of("\n");
  return fileBreak.of(used.values().next().value ?? "\n");
}

/** The document as the file's bytes: exactly the text it was loaded with, plus the edits. */
export const fileText = (state: EditorState): string =>
  state.doc.sliceString(0, state.doc.length, state.facet(fileBreak));

/** Rewrites every line break in `text` to the editor's own, before it is inserted. */
export const normalizeBreaks = (text: string, state: EditorState): string =>
  text.replace(/\r\n?|\n/g, state.lineBreak);
