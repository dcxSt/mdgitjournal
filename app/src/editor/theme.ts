// Every colour, font and size comes from the design tokens in src/styles/tokens.css, so the
// editor follows the app's light/dark switch on its own; setDark only informs CodeMirror's
// built-in panels.

import { syntaxHighlighting } from "@codemirror/language";
import type { Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { NodeType } from "@lezer/common";
import { tagHighlighter, tags as t } from "@lezer/highlight";

const tint = (token: string, percent: number) =>
  `color-mix(in srgb, var(${token}) ${percent}%, transparent)`;

export const baseTheme = EditorView.theme({
  "&": { height: "100%", color: "var(--color-text)", backgroundColor: "var(--color-bg)" },
  "&.cm-focused": { outline: "none" },
  ".cm-content": { caretColor: "var(--color-text)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--color-text)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground":
    { backgroundColor: tint("--color-accent", 22) },
  // The vim block cursor; the extra class outranks the plugin's own high-precedence colour.
  ".cm-vimCursorLayer .cm-fat-cursor": { background: tint("--color-accent", 40) },
  "&:not(.cm-focused) .cm-vimCursorLayer .cm-fat-cursor": {
    background: "none",
    outline: `1px solid ${tint("--color-accent", 60)}`,
  },
  ".cm-searchMatch": { backgroundColor: tint("--color-accent", 15) },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: tint("--color-accent", 35) },
  ".cm-specialChar": { color: "var(--color-danger)" },
  ".cm-panels": {
    backgroundColor: "var(--color-bg)",
    color: "var(--color-text)",
    fontFamily: "var(--font-ui)",
    fontSize: "var(--text-ui)",
  },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--color-syntax)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--color-syntax)" },
  ".cm-code-keyword": { color: "var(--color-accent)" },
  ".cm-code-string": { color: "var(--color-muted)" },
  ".cm-code-comment": { color: "var(--color-syntax)", fontStyle: "italic" },
  ".cm-code-invalid": { color: "var(--color-danger)" },
});

export const sourceTheme = EditorView.theme({
  "&": { fontSize: "var(--text-source)" },
  ".cm-scroller": { fontFamily: "var(--font-source)", lineHeight: "1.5" },
  ".cm-content": { padding: "1rem 0" },
  ".cm-gutters": { backgroundColor: "var(--color-bg)", color: "var(--color-syntax)", border: "none" },
  ".cm-lineNumbers .cm-gutterElement": { padding: "0 0.75em 0 1em" },
  ".cm-activeLine": { backgroundColor: tint("--color-text", 4) },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--color-muted)" },
  // Later rules win where inherited classes meet on one span, e.g. the URL inside a link, or the
  // "~~" of a strikethrough, which stay unstruck.
  ".cm-src-heading": { color: "var(--color-accent)", fontWeight: "bold" },
  ".cm-src-em": { fontStyle: "italic" },
  ".cm-src-strong": { fontWeight: "bold" },
  ".cm-src-strike": { textDecoration: "line-through" },
  ".cm-src-quote": { color: "var(--color-muted)" },
  ".cm-src-link": { color: "var(--color-accent)" },
  ".cm-src-mark": { color: "var(--color-muted)", textDecoration: "none" },
});

export const styledTheme = EditorView.theme({
  "&": { fontSize: "var(--text-styled)" },
  ".cm-scroller": { fontFamily: "var(--font-styled)", lineHeight: "var(--leading-styled)" },
  ".cm-content": {
    maxWidth: "calc(var(--measure-styled) + 3rem)",
    margin: "0 auto",
    padding: "2rem 1.5rem",
  },
  ".cm-line": { padding: "0" },
  ".cm-md-h1": { fontSize: "1.6em" },
  ".cm-md-h2": { fontSize: "1.3em" },
  ".cm-md-h3": { fontSize: "1.15em" },
  ".cm-md-h1, .cm-md-h2, .cm-md-h3, .cm-md-h4, .cm-md-h5, .cm-md-h6": {
    lineHeight: "1.3",
    fontVariantNumeric: "oldstyle-nums",
  },
  ".cm-md-quote": { borderLeft: "2px solid var(--color-syntax)", paddingLeft: "1em" },
  ".cm-md-codeblock": {
    fontFamily: "var(--font-source)",
    fontSize: "0.85em",
    backgroundColor: tint("--color-text", 5),
    padding: "0 0.75em",
  },
  ".cm-md-em": { fontStyle: "italic" },
  ".cm-md-strong": { fontWeight: "bold" },
  ".cm-md-strike": { textDecoration: "line-through" },
  ".cm-md-code": {
    fontFamily: "var(--font-source)",
    fontSize: "0.85em",
    backgroundColor: tint("--color-text", 6),
    borderRadius: "3px",
  },
  ".cm-md-link": { color: "var(--color-accent)" },
  ".cm-md-task": { color: "var(--color-muted)", cursor: "pointer" },
  ".cm-md-task-done": { color: "var(--color-muted)" },
  ".cm-md-mark": { color: "var(--color-syntax)" },
  ".cm-md-image": { padding: "0.5em 0" },
  ".cm-md-image img": { display: "block", maxWidth: "100%", maxHeight: "320px", borderRadius: "6px" },
});

export const mobileStyledTheme = EditorView.theme({
  "&": { fontSize: "var(--text-styled-mobile)" },
  ".cm-content": { padding: "1rem" },
});

/**
 * Colours for fenced code in both modes (`inMarkdown` excludes the markdown tree itself), and
 * for the markdown syntax in source mode. Styled mode draws markdown with decorations instead.
 */
export function highlighters(inMarkdown: (type: NodeType) => boolean): {
  code: Extension;
  source: Extension;
} {
  const code = tagHighlighter(
    [
      { tag: [t.keyword, t.number, t.bool, t.null, t.atom, t.tagName], class: "cm-code-keyword" },
      { tag: [t.string, t.regexp, t.escape, t.attributeValue], class: "cm-code-string" },
      { tag: [t.comment, t.meta], class: "cm-code-comment" },
      { tag: t.invalid, class: "cm-code-invalid" },
    ],
    { scope: (type) => !inMarkdown(type) },
  );
  const source = tagHighlighter(
    [
      { tag: t.heading, class: "cm-src-heading" },
      { tag: t.emphasis, class: "cm-src-em" },
      { tag: t.strong, class: "cm-src-strong" },
      { tag: t.strikethrough, class: "cm-src-strike" },
      { tag: t.quote, class: "cm-src-quote" },
      { tag: [t.link, t.atom], class: "cm-src-link" },
      {
        tag: [t.processingInstruction, t.contentSeparator, t.labelName, t.url, t.string, t.comment],
        class: "cm-src-mark",
      },
    ],
    { scope: inMarkdown },
  );
  return { code: syntaxHighlighting(code), source: syntaxHighlighting(source) };
}
