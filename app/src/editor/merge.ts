// The merge view's editor (docs/architecture.md, "Merge editor"): one editable result document,
// prefilled with the three-way merge. What came from each side is tinted, and each clash is a
// block offering both versions until the user picks one. Plain CodeMirror: a StateField of
// regions mapped through every change, drawn as line decorations and block widgets. Nothing here
// ever writes a conflict marker; an unpicked clash contributes no text at all.

import {
  defaultKeymap,
  history,
  historyKeymap,
  invertedEffects,
  isolateHistory,
  redo,
  undo,
  undoDepth,
} from "@codemirror/commands";
import {
  type ChangeDesc,
  Compartment,
  EditorState,
  type Extension,
  type Range,
  StateEffect,
  StateField,
  Transaction,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  drawSelection,
  EditorView,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  WidgetType,
} from "@codemirror/view";
import { highlighting, markdownSupport } from "./language";
import { fileText, lineBreaks } from "./linebreaks";
import { baseTheme, sourceTheme } from "./theme";

/** The backend's chunks (types.ts): whole lines, in file order. */
export type MergeChunk =
  | { kind: "same"; text: string }
  | { kind: "mine" | "theirs" | "both"; text: string; base: string }
  | { kind: "clash"; base: string; mine: string; theirs: string };

type ClashChunk = Extract<MergeChunk, { kind: "clash" }>;

export type ClashPick = "mine" | "theirs" | "both";

/**
 * An editor's document and picks, to open it again where the user left it. Undo history starts
 * afresh: CodeMirror can't store a pick's undo in JSON.
 */
export interface MergeSnapshot {
  readonly json: unknown;
}

export interface MergeEditorOptions {
  chunks: MergeChunk[];
  dark: boolean;
  mobile: boolean;
  /** After every edit, pick or undo, with the result and the number of clashes still unpicked. */
  onChange(doc: string, unresolved: number): void;
  /** Reopens an earlier editor of the same chunks (`MergeEditorHandle.snapshot`). */
  snapshot?: MergeSnapshot;
}

export interface MergeEditorHandle {
  /** The result. Unpicked clashes contribute nothing. */
  getDoc(): string;
  unresolved(): number;
  /** What a clash block's buttons do; `clash` counts from 0 in document order. */
  pick(clash: number, pick: ClashPick): void;
  /** Undo and redo, as Mod-z and Mod-Shift-z do; false if there was nothing to undo or redo. */
  undo(): boolean;
  redo(): boolean;
  undoDepth(): number;
  snapshot(): MergeSnapshot;
  setDark(on: boolean): void;
  focus(): void;
  destroy(): void;
}

/** "Use both": mine, then theirs, with a line break between them if mine doesn't end in one. */
export function bothText(mine: string, theirs: string): string {
  if (mine === "" || /[\r\n]$/.test(mine)) return mine + theirs;
  return mine + (mine.includes("\r\n") ? "\r\n" : "\n") + theirs;
}

function pickText(chunk: ClashChunk, pick: ClashPick): string {
  if (pick === "mine") return chunk.mine;
  if (pick === "theirs") return chunk.theirs;
  return bothText(chunk.mine, chunk.theirs);
}

/** The merged text with these picks (missing = unpicked), or null if a clash is unpicked. */
export function joinChunks(chunks: MergeChunk[], picks?: readonly (ClashPick | null)[]): string | null {
  let text = "";
  let clash = 0;
  for (const chunk of chunks) {
    if (chunk.kind !== "clash") {
      text += chunk.text;
      continue;
    }
    const pick = picks?.[clash++] ?? null;
    if (pick === null) return null;
    text += pickText(chunk, pick);
  }
  return text;
}

// Regions: where each side's text is in the document. Positions follow edits; text inserted at
// a region's start belongs to it (it is on the region's first line), text at its end doesn't.

interface Marked {
  kind: "mine" | "theirs" | "both";
  from: number;
  to: number;
  base: string;
  /** Lines deleted on that side: drawn as a thin marker, since they have no text. */
  deletion: boolean;
}

interface Clash {
  kind: "clash";
  index: number;
  from: number;
  to: number;
  chunk: ClashChunk;
  pick: ClashPick | null;
  /** For a "both" pick: where GitHub's lines start. */
  mid: number;
}

type Region = Marked | Clash;

interface PickValue {
  index: number;
  pick: ClashPick;
  /** Document length of the inserted text, and of its mine part for "both". */
  length: number;
  mineLength: number;
}

const pickEffect = StateEffect.define<PickValue>();
const unpickEffect = StateEffect.define<PickValue>();
/** Sets every region at once: only when the editor is created. */
const setRegions = StateEffect.define<Region[]>();

const mapRegion = (r: Region, changes: ChangeDesc): Region => {
  const from = changes.mapPos(r.from, -1);
  const to = Math.max(from, changes.mapPos(r.to, -1));
  if (r.kind !== "clash") return { ...r, from, to };
  return { ...r, from, to, mid: Math.min(to, Math.max(from, changes.mapPos(r.mid, -1))) };
};

function initialRegions(chunks: MergeChunk[], state: EditorState): Region[] {
  const regions: Region[] = [];
  let pos = 0;
  let index = 0;
  for (const chunk of chunks) {
    if (chunk.kind === "clash") {
      regions.push({ kind: "clash", index: index++, from: pos, to: pos, mid: pos, chunk, pick: null });
      continue;
    }
    const length = state.toText(chunk.text).length;
    if (chunk.kind !== "same") {
      regions.push({ kind: chunk.kind, from: pos, to: pos + length, base: chunk.base, deletion: chunk.text === "" });
    }
    pos += length;
  }
  return regions;
}

const regionsField = StateField.define<Region[]>({
  create: () => [],
  update(regions, tr) {
    let next = tr.docChanged ? regions.map((r) => mapRegion(r, tr.changes)) : regions;
    for (const effect of tr.effects) {
      if (effect.is(setRegions)) {
        next = effect.value;
        continue;
      }
      const picked = effect.is(pickEffect);
      if (!picked && !effect.is(unpickEffect)) continue;
      const { index, pick, length, mineLength } = effect.value;
      next = next.map((r) =>
        r.kind === "clash" && r.index === index
          ? picked
            ? { ...r, pick, to: r.from + length, mid: r.from + (pick === "both" ? mineLength : length) }
            : { ...r, pick: null, to: r.from, mid: r.from }
          : r,
      );
    }
    return next;
  },
  toJSON: (regions) => regions,
  fromJSON: (json) => json as Region[],
});

// Undo of a pick unpicks, and redo picks again.
const invertPicks = invertedEffects.of((tr) =>
  tr.effects.flatMap((e) =>
    e.is(pickEffect) ? [unpickEffect.of(e.value)] : e.is(unpickEffect) ? [pickEffect.of(e.value)] : [],
  ),
);

const unresolvedIn = (state: EditorState) =>
  state.field(regionsField).filter((r) => r.kind === "clash" && r.pick === null).length;

function pickClash(view: EditorView, index: number, pick: ClashPick): boolean {
  const clash = view.state
    .field(regionsField)
    .find((r): r is Clash => r.kind === "clash" && r.index === index);
  if (!clash || clash.pick !== null) return false;
  const { state } = view;
  const insert = state.toText(pickText(clash.chunk, pick));
  const mineLength = pick === "both" ? state.toText(bothText(clash.chunk.mine, "")).length : 0;
  view.dispatch({
    changes: { from: clash.from, insert },
    effects: pickEffect.of({ index, pick, length: insert.length, mineLength }),
    annotations: isolateHistory.of("full"),
    userEvent: "input.pick",
  });
  return true;
}

// Drawing

const SIDE = { mine: "Yours", theirs: "GitHub's" } as const;
const DELETED = { mine: "Deleted in your version.", theirs: "Deleted on GitHub." } as const;
const ORIGIN = { mine: "your version", theirs: "GitHub's version", both: "both versions" } as const;

/** Text as shown in a block: without its last line break. */
const shown = (text: string) => text.replace(/\r?\n$|\r$/, "");

class ClashWidget extends WidgetType {
  constructor(
    readonly index: number,
    readonly total: number,
    readonly chunk: ClashChunk,
  ) {
    super();
  }

  override eq(other: ClashWidget): boolean {
    return other.index === this.index && other.total === this.total && other.chunk === this.chunk;
  }

  toDOM(view: EditorView): HTMLElement {
    const el = (tag: string, cls: string, text?: string) => {
      const node = document.createElement(tag);
      node.className = cls;
      if (text !== undefined) node.textContent = text;
      return node;
    };
    const box = el("div", "mdgj-clash");
    box.setAttribute("role", "group");
    box.setAttribute("aria-label", `Clash ${this.index + 1} of ${this.total}`);
    box.append(
      el("div", "mdgj-clash-head", `Changed on both sides${this.total > 1 ? ` · ${this.index + 1} of ${this.total}` : ""}`),
    );
    for (const side of ["mine", "theirs"] as const) {
      const part = el("div", `mdgj-clash-side mdgj-clash-${side}`);
      part.append(el("div", "mdgj-clash-label", SIDE[side]));
      const text = this.chunk[side];
      part.append(text === "" ? el("p", "mdgj-clash-empty", DELETED[side]) : el("pre", "mdgj-clash-text", shown(text)));
      box.append(part);
    }
    const actions = el("div", "mdgj-clash-actions");
    for (const [pick, label] of [
      ["mine", "Use mine"],
      ["theirs", "Use GitHub's"],
      ["both", "Use both"],
    ] as const) {
      const button = el("button", "button", label) as HTMLButtonElement;
      button.type = "button";
      button.addEventListener("click", () => {
        pickClash(view, this.index, pick);
        view.focus();
      });
      actions.append(button);
    }
    box.append(actions);
    return box;
  }

  override ignoreEvent(): boolean {
    return true;
  }
}

/** The thin line where a side deleted lines (or a pick was empty). */
class DeletionWidget extends WidgetType {
  constructor(
    readonly cls: string,
    readonly title: string,
  ) {
    super();
  }

  override eq(other: DeletionWidget): boolean {
    return other.cls === this.cls && other.title === this.title;
  }

  toDOM(): HTMLElement {
    const node = document.createElement("div");
    node.className = `mdgj-merge-deleted ${this.cls}`;
    node.title = this.title;
    node.setAttribute("aria-label", this.title);
    return node;
  }
}

/** A block widget before the line `pos` starts, or after the line it is inside of. */
function blockAt(state: EditorState, pos: number, widget: WidgetType): Range<Decoration> {
  const line = state.doc.lineAt(pos);
  return pos === line.from
    ? Decoration.widget({ widget, block: true, side: -1 }).range(line.from)
    : Decoration.widget({ widget, block: true, side: 1 }).range(line.to);
}

const lineClass = new Map<string, Decoration>();
const lineDeco = (cls: string) => {
  let deco = lineClass.get(cls);
  if (!deco) lineClass.set(cls, (deco = Decoration.line({ class: cls })));
  return deco;
};

function markLines(state: EditorState, from: number, to: number, cls: string, out: Range<Decoration>[]) {
  if (from >= to) return;
  const deco = lineDeco(cls);
  for (let line = state.doc.lineAt(from); ; line = state.doc.line(line.number + 1)) {
    out.push(deco.range(line.from));
    if (line.number === state.doc.lines || line.to + 1 >= to) break;
  }
}

function decorations(state: EditorState): DecorationSet {
  const regions = state.field(regionsField);
  const total = regions.filter((r) => r.kind === "clash").length;
  const out: Range<Decoration>[] = [];
  for (const r of regions) {
    if (r.kind === "clash") {
      if (r.pick === null) {
        out.push(blockAt(state, r.from, new ClashWidget(r.index, total, r.chunk)));
      } else if (r.from === r.to) {
        out.push(blockAt(state, r.from, new DeletionWidget(`mdgj-merge-picked mdgj-merge-${r.pick}`, "Clash resolved: nothing kept")));
      } else if (r.pick === "both") {
        markLines(state, r.from, r.mid, "mdgj-merge-picked mdgj-merge-mine", out);
        markLines(state, r.mid, r.to, "mdgj-merge-picked mdgj-merge-theirs", out);
      } else {
        markLines(state, r.from, r.to, `mdgj-merge-picked mdgj-merge-${r.pick}`, out);
      }
    } else if (r.from < r.to) {
      markLines(state, r.from, r.to, `mdgj-merge-${r.kind}`, out);
    } else if (r.deletion) {
      const title = `Deleted in ${ORIGIN[r.kind]}: ${shown(r.base)}`;
      out.push(blockAt(state, r.from, new DeletionWidget(`mdgj-merge-${r.kind}`, title)));
    }
  }
  return Decoration.set(out, true);
}

const tint = (token: string, percent: number) => `color-mix(in srgb, var(${token}) ${percent}%, transparent)`;

const mergeTheme = EditorView.theme({
  ".cm-line.mdgj-merge-mine": { backgroundColor: tint("--color-accent", 9), boxShadow: `inset 2px 0 ${tint("--color-accent", 70)}` },
  ".cm-line.mdgj-merge-theirs": { backgroundColor: tint("--color-theirs", 11), boxShadow: `inset 2px 0 ${tint("--color-theirs", 80)}` },
  ".cm-line.mdgj-merge-both": { backgroundColor: tint("--color-text", 5), boxShadow: `inset 2px 0 ${tint("--color-text", 25)}` },
  ".cm-line.mdgj-merge-picked": { boxShadow: "inset 4px 0 var(--mdgj-bar)" },
  ".cm-line.mdgj-merge-picked.mdgj-merge-mine": { "--mdgj-bar": "var(--color-accent)" },
  ".cm-line.mdgj-merge-picked.mdgj-merge-theirs": { "--mdgj-bar": "var(--color-theirs)" },
  ".mdgj-merge-deleted": { height: "3px", margin: "1px 0", borderRadius: "2px" },
  ".mdgj-merge-deleted.mdgj-merge-mine": { backgroundColor: tint("--color-accent", 70) },
  ".mdgj-merge-deleted.mdgj-merge-theirs": { backgroundColor: tint("--color-theirs", 80) },
  ".mdgj-merge-deleted.mdgj-merge-both": { backgroundColor: tint("--color-text", 30) },
  ".mdgj-clash": {
    margin: "6px 8px 6px 0",
    padding: "8px 10px 10px",
    border: `1px solid ${tint("--color-text", 18)}`,
    borderRadius: "6px",
    backgroundColor: "var(--color-bg)",
    fontFamily: "var(--font-ui)",
    fontSize: "var(--text-ui)",
    lineHeight: "1.4",
    whiteSpace: "normal",
  },
  ".mdgj-clash-head": { marginBottom: "6px", color: "var(--color-muted)" },
  ".mdgj-clash-side": { margin: "0 0 6px", padding: "4px 8px 6px", borderRadius: "4px" },
  ".mdgj-clash-mine": { backgroundColor: tint("--color-accent", 9), boxShadow: `inset 2px 0 ${tint("--color-accent", 70)}` },
  ".mdgj-clash-theirs": { backgroundColor: tint("--color-theirs", 11), boxShadow: `inset 2px 0 ${tint("--color-theirs", 80)}` },
  ".mdgj-clash-label": { fontSize: "11px", color: "var(--color-muted)" },
  ".mdgj-clash-text": {
    margin: "2px 0 0",
    fontFamily: "var(--font-source)",
    fontSize: "var(--text-source)",
    lineHeight: "1.5",
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
  },
  ".mdgj-clash-empty": { margin: "2px 0 0", color: "var(--color-muted)", fontStyle: "italic" },
  ".mdgj-clash-actions": { display: "flex", flexWrap: "wrap", gap: "6px", marginTop: "8px" },
  ".mdgj-clash-actions .button": { minHeight: "28px" },
  "&.mdgj-merge-touch .mdgj-clash-actions .button": { minHeight: "44px", flex: "1 1 auto", padding: "0 12px" },
  "&.mdgj-merge-touch": { fontSize: "16px" },
  "&.mdgj-merge-touch .cm-content": { padding: "0.75rem 0" },
  "&.mdgj-merge-touch .cm-line": { padding: "0 12px" },
  "&.mdgj-merge-touch .mdgj-clash": { margin: "8px 12px" },
});

const mergeKeymap = keymap.of([...defaultKeymap, ...historyKeymap]);

export function createMergeEditor(parent: HTMLElement, opts: MergeEditorOptions): MergeEditorHandle {
  // One kind of line break if every version of the file uses one; else lines split on "\n".
  const texts = opts.chunks.map((c) => (c.kind === "clash" ? `${c.mine}x${c.theirs}` : c.text));
  const breaks = lineBreaks(texts.join("x"));
  const darkConf = new Compartment();
  const initial = opts.chunks.map((c) => (c.kind === "clash" ? "" : c.text)).join("");

  const extensions: Extension = [
    breaks,
    regionsField,
    history(),
    invertPicks,
    drawSelection(),
    highlightSpecialChars(),
    EditorView.lineWrapping,
    markdownSupport,
    highlighting.code,
    highlighting.source,
    mergeKeymap,
    EditorView.decorations.compute([regionsField], decorations),
    EditorView.updateListener.of((update) => {
      const picked = update.transactions.some((tr) => tr.effects.some((e) => e.is(pickEffect) || e.is(unpickEffect)));
      if (update.docChanged || picked) opts.onChange(fileText(update.state), unresolvedIn(update.state));
    }),
    opts.mobile ? EditorView.editorAttributes.of({ class: "mdgj-merge-touch" }) : lineNumbers(),
    baseTheme,
    sourceTheme,
    mergeTheme,
    EditorView.contentAttributes.of({ "aria-label": "Merged text" }),
    darkConf.of(EditorView.darkTheme.of(opts.dark)),
  ];
  const fields = { regions: regionsField };
  let state: EditorState;
  if (opts.snapshot) {
    state = EditorState.fromJSON(opts.snapshot.json, { extensions }, fields);
  } else {
    const blank = EditorState.create({ doc: initial, extensions });
    state = blank.update({
      effects: setRegions.of(initialRegions(opts.chunks, blank)),
      annotations: Transaction.addToHistory.of(false),
    }).state;
  }
  const view = new EditorView({ parent, state });

  return {
    getDoc: () => fileText(view.state),
    unresolved: () => unresolvedIn(view.state),
    pick: (clash, pick) => void pickClash(view, clash, pick),
    undo: () => undo(view),
    redo: () => redo(view),
    undoDepth: () => undoDepth(view.state),
    snapshot: () => ({ json: view.state.toJSON(fields) }),
    setDark: (on) => view.dispatch({ effects: darkConf.reconfigure(EditorView.darkTheme.of(on)) }),
    focus: () => view.focus(),
    destroy: () => view.destroy(),
  };
}
