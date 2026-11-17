// Guards around codemirror-vim, which drives CodeMirror 6 through a CodeMirror 5 adapter. When vim
// throws on a key, it has done part of the command, and the key itself goes on to the browser.

import { EditorView, ViewPlugin } from "@codemirror/view";
import { getCM } from "@replit/codemirror-vim";

/**
 * CodeMirror 5's `posFromIndex` clamps an offset to the document, and vim relies on that: a put
 * over a backward visual selection (`vbp`) asks for a position past the end. The adapter's
 * throws instead, after vim has deleted the selection and before it puts the text.
 */
const clampedOffsets = ViewPlugin.define((view) => {
  const cm = getCM(view);
  if (cm) {
    const posFromIndex = cm.posFromIndex.bind(cm);
    cm.posFromIndex = (offset) => posFromIndex(Math.max(0, Math.min(offset, view.state.doc.length)));
  }
  return {};
});

/**
 * Outside insert mode, keys are commands: vim reads them from keydown, IME input included, before
 * this sees any text. So text that still arrives (a key vim failed on) is dropped, not typed.
 */
const noTextOutsideInsertMode = EditorView.inputHandler.of((view) => {
  const cm = getCM(view);
  const vim = cm?.state.vim;
  return !!vim && !vim.insertMode && !vim.expectLiteralNext && !view.composing && !cm.curOp?.isVimOp;
});

/** Goes after vim(), whose adapter and input handler these follow. */
export const vimGuards = [clampedOffsets, noTextOutsideInsertMode];
