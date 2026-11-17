// Styled mode: the markdown stays in the text, dimmed, while decorations derived from the
// syntax tree draw it as prose. Nothing here changes the document except a task-box click.

import { syntaxTree } from "@codemirror/language";
import type { EditorState, Extension, Range } from "@codemirror/state";
import { Prec, StateField } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import { IterMode, type SyntaxNodeRef } from "@lezer/common";
import { mobileStyledTheme, styledTheme } from "./theme";

const line = (cls: string) => Decoration.line({ class: cls });
const mark = (cls: string) => Decoration.mark({ class: cls });

const lineDecorations: Record<string, Decoration> = {
  ATXHeading1: line("cm-md-h1"),
  ATXHeading2: line("cm-md-h2"),
  ATXHeading3: line("cm-md-h3"),
  ATXHeading4: line("cm-md-h4"),
  ATXHeading5: line("cm-md-h5"),
  ATXHeading6: line("cm-md-h6"),
  SetextHeading1: line("cm-md-h1"),
  SetextHeading2: line("cm-md-h2"),
  Blockquote: line("cm-md-quote"),
  FencedCode: line("cm-md-codeblock"),
  CodeBlock: line("cm-md-codeblock"),
};

const dimmed = mark("cm-md-mark");
const linkText = mark("cm-md-link");
const taskDone = mark("cm-md-task-done");
const struck = mark("cm-md-strike");

const markDecorations: Record<string, Decoration> = {
  Emphasis: mark("cm-md-em"),
  StrongEmphasis: mark("cm-md-strong"),
  InlineCode: mark("cm-md-code"),
  Link: linkText,
  Image: linkText,
  TaskMarker: mark("cm-md-task"),
};

const syntaxMarks = new Set([
  "HeaderMark",
  "QuoteMark",
  "ListMark",
  "EmphasisMark",
  "StrikethroughMark",
  "CodeMark",
  "CodeInfo",
  "LinkMark",
  "LinkLabel",
  "LinkTitle",
  "TableDelimiter",
  "HorizontalRule",
  "Comment",
  "CommentBlock",
]);

const linkParents = new Set(["Link", "Image", "LinkReference"]);

function markFor(node: SyntaxNodeRef): Decoration | undefined {
  // A link's target is syntax; a bare or <angle-bracket> URL is the link itself.
  if (node.name === "URL") return linkParents.has(node.node.parent?.name ?? "") ? dimmed : linkText;
  return markDecorations[node.name] ?? (syntaxMarks.has(node.name) ? dimmed : undefined);
}

function decorate(view: EditorView): DecorationSet {
  const { state } = view;
  const ranges: Range<Decoration>[] = [];
  for (const { from, to } of view.visibleRanges) {
    syntaxTree(state).iterate({
      from,
      to,
      mode: IterMode.IgnoreMounts,
      enter(node) {
        const start = Math.max(node.from, from);
        const end = Math.min(node.to, to);
        const block = lineDecorations[node.name];
        if (block) {
          for (let pos = start; pos <= end; ) {
            const { from: lineStart, to: lineEnd } = state.doc.lineAt(pos);
            ranges.push(block.range(lineStart));
            pos = lineEnd + 1;
          }
        }
        const deco = markFor(node);
        if (deco && start < end) ranges.push(deco.range(start, end));
        // Only the text between the "~~" is struck: a line-through reaches every inline element
        // inside, so it cannot be kept off the marks if they are inside.
        if (node.name === "Strikethrough") {
          const text = Math.max(node.node.firstChild?.to ?? node.from, from);
          const textEnd = Math.min(node.node.lastChild?.from ?? node.to, to);
          if (text < textEnd) ranges.push(struck.range(text, textEnd));
        }
        if (node.name === "Task") {
          const box = node.node.firstChild;
          if (box?.name !== "TaskMarker" || state.sliceDoc(box.from + 1, box.to - 1) === " ") return;
          const text = Math.max(box.to, from);
          if (text < end) ranges.push(taskDone.range(text, end));
        }
      },
    });
  }
  return Decoration.set(ranges, true);
}

const decorations = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = decorate(view);
    }
    update(update: ViewUpdate) {
      const reparsed = syntaxTree(update.startState) !== syntaxTree(update.state);
      if (update.docChanged || update.viewportChanged || reparsed) {
        this.decorations = decorate(update.view);
      }
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

/** A click on "[ ]" or "[x]" rewrites exactly the one character between the brackets. */
const taskToggle = EditorView.domEventHandlers({
  mousedown(event, view) {
    const box = event.target instanceof Element ? event.target.closest(".cm-md-task") : null;
    if (event.button !== 0 || !box) return false;
    const pos = view.posAtDOM(box);
    const text = view.state.sliceDoc(pos, pos + 3);
    if (!/^\[[ xX]\]$/.test(text)) return false;
    view.dispatch({
      changes: { from: pos + 1, to: pos + 2, insert: text[1] === " " ? "x" : " " },
      userEvent: "input",
    });
    event.preventDefault();
    return true;
  },
});

class ImageWidget extends WidgetType {
  constructor(readonly src: string) {
    super();
  }

  override eq(other: ImageWidget): boolean {
    return other.src === this.src;
  }

  override toDOM(view: EditorView): HTMLElement {
    const wrap = document.createElement("div");
    wrap.className = "cm-md-image";
    const img = wrap.appendChild(document.createElement("img"));
    // The markdown above already carries the alt text.
    img.alt = "";
    img.draggable = false;
    img.onload = () => view.requestMeasure();
    img.onerror = () => {
      wrap.style.display = "none";
      view.requestMeasure();
    };
    img.src = this.src;
    return wrap;
  }
}

interface StyledOptions {
  mobile: boolean;
  resolveImageUrl?(relPath: string): string;
}

/**
 * Only images inside the journal get a preview: remote ones would contact their host whenever an
 * entry is shown, and the app talks to no one but GitHub.
 */
function imageSource(target: string, opts: StyledOptions): string | null {
  const url = target.startsWith("<") && target.endsWith(">") ? target.slice(1, -1) : target;
  if (!opts.resolveImageUrl || /^([a-z][a-z\d+.-]*:|\/)/i.test(url)) return null;
  // Link targets are URLs, so "my%20photo.jpg" names the file "my photo.jpg".
  let path = url;
  try {
    path = decodeURI(url);
  } catch {
    // Malformed escapes: use the path as written.
  }
  return opts.resolveImageUrl(path);
}

function imageWidgets(state: EditorState, opts: StyledOptions): DecorationSet {
  const widgets: Range<Decoration>[] = [];
  syntaxTree(state).iterate({
    mode: IterMode.IgnoreMounts,
    enter(node) {
      if (node.name !== "Image") return;
      const target = node.node.getChild("URL");
      const src = target && imageSource(state.sliceDoc(target.from, target.to), opts);
      if (src) {
        const widget = Decoration.widget({ widget: new ImageWidget(src), block: true, side: 1 });
        widgets.push(widget.range(state.doc.lineAt(node.to).to));
      }
      return false;
    },
  });
  return Decoration.set(widgets, true);
}

/**
 * Previews sit below their line as block widgets, which CodeMirror only accepts from a state
 * field, so unlike the other decorations they cover the whole document.
 */
function imagePreviews(opts: StyledOptions): Extension {
  return StateField.define<DecorationSet>({
    create: (state) => imageWidgets(state, opts),
    update(widgets, tr) {
      const reparsed = syntaxTree(tr.startState) !== syntaxTree(tr.state);
      return tr.docChanged || reparsed ? imageWidgets(tr.state, opts) : widgets;
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}

// Prose gets the platform's spelling help; source mode keeps CodeMirror's defaults (all off).
const proseInput = EditorView.contentAttributes.of({
  spellcheck: "true",
  autocorrect: "on",
  autocapitalize: "sentences",
});

export function styledMode(opts: StyledOptions): Extension {
  return [
    styledTheme,
    // Without the precedence, the later theme is mounted first and loses the cascade.
    opts.mobile ? Prec.high(mobileStyledTheme) : [],
    proseInput,
    decorations,
    imagePreviews(opts),
    taskToggle,
  ];
}
