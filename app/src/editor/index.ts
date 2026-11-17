// The CodeMirror 6 editor: one view whose source/styled mode, vim keys and dark flag live in
// compartments, so switching never rebuilds the state and keeps cursor, scroll and undo history.
// Markdown is never parsed into a model and written back: getDoc() returns the exact string given
// to setDoc(), plus whatever the user typed.

import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { languageDataProp } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension, Facet } from "@codemirror/state";
import {
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { GFM } from "@lezer/markdown";
import { Vim, vim } from "@replit/codemirror-vim";
import { insertImageLinks, toggleLinePrefix, wrapSelection } from "./commands";
import { fileText, lineBreaks, normalizeBreaks } from "./linebreaks";
import { styledMode } from "./styled";
import { baseTheme, highlighters, sourceTheme } from "./theme";

export { imageLink } from "./commands";

export type EditorMode = "source" | "styled";

export interface EditorOptions {
  doc: string;
  mode: EditorMode;
  vim: boolean;
  dark: boolean;
  mobile: boolean;
  /** Called after every change with the full document. */
  onChange(doc: string): void;
  /** Vim `:w` and Mod-s. */
  onSave?(): void;
  /**
   * Pasted or dropped images; resolves to the relative paths to link, e.g. "assets/x-1.jpg".
   * On failure, report it and resolve to []; nothing is inserted then.
   */
  onImageFiles?(files: File[]): Promise<string[]>;
  /** Maps a relative image path to a loadable URL for styled-mode previews. */
  resolveImageUrl?(relPath: string): string;
}

export interface EditorHandle {
  /** Replaces the document and resets undo history; mode, vim and theme are kept. */
  setDoc(doc: string): void;
  getDoc(): string;
  setMode(m: EditorMode): void;
  /** Ignored on mobile. */
  setVim(on: boolean): void;
  setDark(on: boolean): void;
  focus(): void;
  insertAtCursor(text: string): void;
  /** Wraps the selection (or inserts the pair around the cursor); unwraps if already wrapped. */
  wrapSelection(before: string, after: string): void;
  /** Toggles "# ", "- " or "- [ ] " on the selected lines. */
  toggleLinePrefix(prefix: string): void;
  destroy(): void;
}

// No autocompletion, list continuation, link-on-paste or tag closing: every inserted byte is one
// the user typed, and Enter never renumbers the lines below it.
const markdownSupport = markdown({
  extensions: GFM,
  codeLanguages: languages,
  addKeymap: false,
  completeHTMLTags: false,
  pasteURLAsLink: false,
});
const highlighting = highlighters(
  (type) => type.prop(languageDataProp) === markdownSupport.language.data,
);

const sourceMode: Extension = [
  lineNumbers(),
  highlightActiveLine(),
  highlightActiveLineGutter(),
  highlighting.source,
  sourceTheme,
];

const saveHandlers = Facet.define<() => void>();
const save = (view: EditorView) => {
  for (const handler of view.state.facet(saveHandlers)) handler();
  return true;
};
Vim.defineEx("write", "w", (cm) => save(cm.cm6 as EditorView));

const editingKeymap = keymap.of([
  { key: "Mod-s", run: save },
  { key: "Mod-b", run: (view) => (view.dispatch(wrapSelection(view.state, "**", "**")), true) },
  { key: "Mod-i", run: (view) => (view.dispatch(wrapSelection(view.state, "_", "_")), true) },
  indentWithTab,
  ...defaultKeymap,
  ...historyKeymap,
  // Mod-Shift-l is the app's Pull shortcut.
  ...searchKeymap.filter((binding) => binding.key !== "Mod-Shift-l"),
]);

const imageFiles = (data: DataTransfer | null): File[] =>
  Array.from(data?.files ?? []).filter((file) => file.type.startsWith("image/"));

export function createEditor(parent: HTMLElement, opts: EditorOptions): EditorHandle {
  const vimConf = new Compartment();
  const modeConf = new Compartment();
  const darkConf = new Compartment();
  let { mode, dark } = opts;
  let vimOn = opts.vim && !opts.mobile;
  const styled = styledMode(opts);
  const modeExtension = () => (mode === "source" ? sourceMode : styled);

  // Insert positions of image imports still in flight, kept current as the document changes.
  const pendingImports = new Set<{ pos: number }>();

  async function insertImported(view: EditorView, imported: Promise<string[]>, pos: number) {
    const slot = { pos };
    pendingImports.add(slot);
    try {
      const paths = await imported;
      // A setDoc() in the meantime means the images belong to another entry.
      if (pendingImports.delete(slot) && paths.length > 0) {
        view.dispatch(insertImageLinks(view.state, slot.pos, paths));
      }
    } finally {
      pendingImports.delete(slot);
    }
  }

  function handleImages(view: EditorView, event: Event, files: File[], pos: number): boolean {
    if (!opts.onImageFiles || files.length === 0) return false;
    event.preventDefault();
    void insertImported(view, opts.onImageFiles(files), pos);
    return true;
  }

  const handlers = EditorView.domEventHandlers({
    paste(event, view) {
      const data = event.clipboardData;
      // Rich-text apps (Word, Pages, Excel) add a picture of the copied text; the text is wanted.
      if (data?.getData("text/html") && data.getData("text/plain")) return false;
      return handleImages(view, event, imageFiles(data), view.state.selection.main.head);
    },
    drop(event, view) {
      const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
      const files = imageFiles(event.dataTransfer);
      return handleImages(view, event, files, pos ?? view.state.selection.main.head);
    },
  });

  const createState = (doc: string) =>
    EditorState.create({
      doc,
      extensions: [
        // First, so vim sees keys before every other keymap.
        vimConf.of(vimOn ? vim() : []),
        lineBreaks(doc),
        EditorView.clipboardInputFilter.of(normalizeBreaks),
        saveHandlers.of(() => opts.onSave?.()),
        history(),
        drawSelection(),
        dropCursor(),
        highlightSpecialChars(),
        EditorView.lineWrapping,
        markdownSupport,
        highlighting.code,
        editingKeymap,
        handlers,
        EditorView.updateListener.of((update) => {
          if (!update.docChanged) return;
          for (const slot of pendingImports) slot.pos = update.changes.mapPos(slot.pos);
          opts.onChange(fileText(update.state));
        }),
        baseTheme,
        darkConf.of(EditorView.darkTheme.of(dark)),
        modeConf.of(modeExtension()),
      ],
    });

  const view = new EditorView({ parent, state: createState(opts.doc) });

  return {
    setDoc(doc) {
      pendingImports.clear();
      view.setState(createState(doc));
    },
    getDoc: () => fileText(view.state),
    setMode(m) {
      mode = m;
      // Line heights change with the font, so pin the line at the top of the view.
      view.dispatch({ effects: [modeConf.reconfigure(modeExtension()), view.scrollSnapshot()] });
    },
    setVim(on) {
      vimOn = on && !opts.mobile;
      view.dispatch({ effects: vimConf.reconfigure(vimOn ? vim() : []) });
    },
    setDark(on) {
      dark = on;
      view.dispatch({ effects: darkConf.reconfigure(EditorView.darkTheme.of(on)) });
    },
    focus: () => view.focus(),
    insertAtCursor(text) {
      view.dispatch(view.state.replaceSelection(normalizeBreaks(text, view.state)), {
        scrollIntoView: true,
        userEvent: "input",
      });
    },
    wrapSelection: (before, after) => view.dispatch(wrapSelection(view.state, before, after)),
    toggleLinePrefix: (prefix) => view.dispatch(toggleLinePrefix(view.state, prefix)),
    destroy() {
      pendingImports.clear();
      view.destroy();
    },
  };
}
