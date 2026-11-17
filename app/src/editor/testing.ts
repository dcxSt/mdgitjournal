// Shared helpers for the editor tests.

import { EditorView } from "@codemirror/view";
import { afterEach, vi } from "vitest";
import { createEditor, type EditorHandle, type EditorOptions } from ".";

// jsdom has no layout. CodeMirror measures text ranges in an animation frame; with no rects it
// falls back to estimates, as it does for an editor that is not displayed.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();

const mounted: EditorHandle[] = [];

/** Destroys every editor mounted so far. Runs after each test. */
export function cleanup() {
  for (const handle of mounted.splice(0)) handle.destroy();
  document.body.replaceChildren();
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

export function mount(options: Partial<EditorOptions> = {}) {
  const parent = document.body.appendChild(document.createElement("div"));
  const onChange = vi.fn<(doc: string) => void>();
  const handle = createEditor(parent, {
    doc: "",
    mode: "source",
    vim: false,
    dark: false,
    mobile: false,
    onChange,
    ...options,
  });
  mounted.push(handle);
  return { handle, onChange, view: EditorView.findFromDOM(parent)! };
}
