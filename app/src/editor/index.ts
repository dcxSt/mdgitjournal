// stub — replaced by editor builder.
// A plain CodeMirror editor that satisfies the contract in docs/architecture.md so the UI can be
// built against it. No styled-mode decorations, image paste/drop or image previews.

import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import {
  type ChangeSpec,
  Compartment,
  EditorSelection,
  EditorState,
  type Extension,
} from "@codemirror/state";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import { vim } from "@replit/codemirror-vim";

export type EditorMode = "source" | "styled";

export interface EditorOptions {
  doc: string;
  mode: EditorMode;
  vim: boolean;
  dark: boolean;
  mobile: boolean;
  onChange(doc: string): void;
  onSave?(): void;
  onImageFiles?(files: File[]): Promise<string[]>;
  resolveImageUrl?(relPath: string): string;
}

export interface EditorHandle {
  setDoc(doc: string): void;
  getDoc(): string;
  setMode(m: EditorMode): void;
  setVim(on: boolean): void;
  setDark(on: boolean): void;
  focus(): void;
  insertAtCursor(text: string): void;
  wrapSelection(before: string, after: string): void;
  toggleLinePrefix(prefix: string): void;
  destroy(): void;
}

const baseTheme = EditorView.theme({
  "&": { height: "100%", color: "var(--color-text)", backgroundColor: "var(--color-bg)" },
  "&.cm-focused": { outline: "none" },
});

const sourceTheme = EditorView.theme({
  "&": { fontSize: "var(--text-source)" },
  ".cm-content, .cm-gutters": { fontFamily: "var(--font-source)" },
});

const styledTheme = EditorView.theme({
  "&": { fontSize: "var(--text-styled)" },
  ".cm-content": {
    fontFamily: "var(--font-styled)",
    lineHeight: "var(--leading-styled)",
    maxWidth: "var(--measure-styled)",
    margin: "0 auto",
  },
});

export function createEditor(parent: HTMLElement, opts: EditorOptions): EditorHandle {
  const modeConf = new Compartment();
  const vimConf = new Compartment();
  const darkConf = new Compartment();
  let { mode, vim: vimOn, dark } = opts;

  const modeExtension = (m: EditorMode): Extension =>
    m === "source" ? [lineNumbers(), sourceTheme] : styledTheme;

  const createState = (doc: string) =>
    EditorState.create({
      doc,
      extensions: [
        // Split lines only on the file's own separator so getDoc() returns the exact input.
        EditorState.lineSeparator.of(doc.includes("\r\n") ? "\r\n" : "\n"),
        vimConf.of(vimOn ? vim() : []),
        history(),
        keymap.of([
          { key: "Mod-s", run: () => (opts.onSave?.(), true) },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        markdown(),
        EditorView.lineWrapping,
        baseTheme,
        modeConf.of(modeExtension(mode)),
        darkConf.of(EditorView.darkTheme.of(dark)),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) opts.onChange(u.state.sliceDoc());
        }),
      ],
    });

  const view = new EditorView({ parent, state: createState(opts.doc) });

  return {
    setDoc: (doc) => view.setState(createState(doc)),
    getDoc: () => view.state.sliceDoc(),
    setMode(m) {
      mode = m;
      view.dispatch({ effects: modeConf.reconfigure(modeExtension(m)) });
    },
    setVim(on) {
      vimOn = on;
      view.dispatch({ effects: vimConf.reconfigure(on ? vim() : []) });
    },
    setDark(on) {
      dark = on;
      view.dispatch({ effects: darkConf.reconfigure(EditorView.darkTheme.of(on)) });
    },
    focus: () => view.focus(),
    insertAtCursor: (text) => view.dispatch(view.state.replaceSelection(text)),
    wrapSelection(before, after) {
      view.dispatch(
        view.state.changeByRange((r) => ({
          changes: [
            { from: r.from, insert: before },
            { from: r.to, insert: after },
          ],
          range: EditorSelection.range(r.anchor + before.length, r.head + before.length),
        })),
      );
    },
    toggleLinePrefix(prefix) {
      const { doc, selection } = view.state;
      const changes: ChangeSpec[] = [];
      const last = doc.lineAt(selection.main.to).number;
      for (let n = doc.lineAt(selection.main.from).number; n <= last; n++) {
        const line = doc.line(n);
        changes.push(
          line.text.startsWith(prefix)
            ? { from: line.from, to: line.from + prefix.length }
            : { from: line.from, insert: prefix },
        );
      }
      view.dispatch({ changes });
    },
    destroy: () => view.destroy(),
  };
}
