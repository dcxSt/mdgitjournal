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
  ViewPlugin,
} from "@codemirror/view";
import { GFM } from "@lezer/markdown";
import { Vim, vim } from "@replit/codemirror-vim";
import { insertImageLinks, toggleLinePrefix, wrapSelection } from "./commands";
import { fileText, lineBreaks, normalizeBreaks } from "./linebreaks";
import { styledMode } from "./styled";
import { wholeCharacters } from "./surrogates";
import { baseTheme, highlighters, sourceTheme } from "./theme";
import { vimGuards } from "./vimguards";

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
  /**
   * Imported images whose links were not inserted, because setDoc() replaced the document (or
   * the editor was destroyed) while they were imported. The files exist; nothing links to them.
   */
  onImagesUnlinked?(paths: string[]): void;
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
  /**
   * Imports images through `onImageFiles` and links them at the cursor, one per line, as a paste
   * does; selected text is kept. Used by the phone's photo picker.
   */
  importImages(files: File[]): void;
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

/**
 * codemirror-vim keeps one jump list (G, gg, n, '', Ctrl-o, ...) for all editors, as offsets into
 * the document it was recorded in, and it outlives the vim instance. Each instance, so each
 * document, starts a fresh one: offsets past the end of a shorter document would make every
 * later jump throw, and the key would be typed as text instead. The rest of vim's global state
 * (registers, search and command history, macros, `:set` options) carries over.
 */
const freshJumpList = ViewPlugin.define(() => {
  const kept = { ...Vim.getVimGlobalState_() };
  // `:set` values are reset too, so they are put back.
  const options = ["pcre", "insertModeEscKeysTimeout"].map(
    (name) => [name, Vim.getOption(name)] as const,
  );
  Vim.resetVimGlobalState_();
  const fresh = Vim.getVimGlobalState_();
  Object.assign(fresh, kept, { jumpList: fresh.jumpList });
  for (const [name, value] of options) Vim.setOption(name, value);
  return {};
});

const vimKeys: Extension = [vim(), freshJumpList, wholeCharacters, vimGuards];

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
      if (paths.length === 0) return;
      // A setDoc() in the meantime means the document they were meant for is gone.
      if (pendingImports.delete(slot)) view.dispatch(insertImageLinks(view.state, slot.pos, paths));
      else opts.onImagesUnlinked?.(paths);
    } finally {
      pendingImports.delete(slot);
    }
  }

  function importAt(view: EditorView, files: File[], pos: number): boolean {
    if (!opts.onImageFiles || files.length === 0) return false;
    void insertImported(view, opts.onImageFiles(files), pos);
    return true;
  }

  function handleImages(view: EditorView, event: Event, files: File[], pos: number): boolean {
    if (!importAt(view, files, pos)) return false;
    event.preventDefault();
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
        vimConf.of(vimOn ? vimKeys : []),
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
      view.dispatch({ effects: vimConf.reconfigure(vimOn ? vimKeys : []) });
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
    importImages: (files) => void importAt(view, files, view.state.selection.main.head),
    wrapSelection: (before, after) => view.dispatch(wrapSelection(view.state, before, after)),
    toggleLinePrefix: (prefix) => view.dispatch(toggleLinePrefix(view.state, prefix)),
    destroy() {
      pendingImports.clear();
      view.destroy();
    },
  };
}
