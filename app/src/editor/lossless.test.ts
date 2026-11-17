import { deleteCharBackward, insertNewlineAndIndent, undo } from "@codemirror/commands";
import type { EditorView } from "@codemirror/view";
import { getCM, Vim } from "@replit/codemirror-vim";
import fc from "fast-check";
import { describe, expect, it } from "vitest";
import type { EditorHandle, EditorMode } from ".";
import { cleanup, mount } from "./testing";

const fragments = [
  "# ", "## ", "###### ", "Title\n===\n", "Sub\n---\n", "**", "*", "_", "__", "~~", "`", "``",
  "```\n", "```js\n", "~~~\n", "    indented", "- ", "* ", "+ ", "1. ", "2) ", "- [ ] ", "- [x] ",
  "[X]", "[ ]", "> ", "> > ", "---", "***", "![](assets/a.jpg)", '![alt](<b c.png> "t")',
  "![x](https://example.com/x.png)", "[link](https://x.y)", "<https://x.y>", "https://z.w",
  "[r]: /u", "<!-- c -->", "<div>", "|a|b|\n|-|-|\n", "\\*", "&amp;", "\r\n", "\n", "\r", "\t",
  " ", "  ", "é", "é", "🙂", "👩‍👩‍👧", "日本語", " ", "​", " ", "﻿", "\0",
];

// Markdown-ish text with every kind of line ending, unicode and arbitrary UTF-16 (lone surrogates
// included), with or without a final newline.
const markdownText = fc
  .array(
    fc.oneof(
      { weight: 4, arbitrary: fc.constantFrom(...fragments) },
      { weight: 1, arbitrary: fc.string({ unit: "binary", maxLength: 6 }) },
      {
        weight: 1,
        arbitrary: fc.string({
          unit: fc.integer({ min: 0, max: 0xffff }).map((code) => String.fromCharCode(code)),
          maxLength: 3,
        }),
      },
    ),
    { maxLength: 40 },
  )
  .map((parts) => parts.join(""));

// Files that use one line break throughout, the common case.
const uniformText = fc
  .tuple(markdownText, fc.constantFrom("\n", "\r\n", "\r"))
  .map(([text, lb]) => text.replace(/\r\n?|\n/g, lb));

type Toggle = ["mode", EditorMode] | ["vim" | "dark", boolean];

const toggle: fc.Arbitrary<Toggle> = fc.oneof(
  fc.tuple(fc.constant("mode" as const), fc.constantFrom<EditorMode>("source", "styled")),
  fc.tuple(fc.constantFrom("vim" as const, "dark" as const), fc.boolean()),
);
const toggles = fc.array(toggle, { maxLength: 12 });

function apply(handle: EditorHandle, ops: Toggle[]) {
  for (const [op, value] of ops) {
    if (op === "mode") handle.setMode(value);
    else if (op === "vim") handle.setVim(value);
    else handle.setDark(value);
  }
}

function paste(view: EditorView, text: string) {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  const data = { files: [], getData: (type: string) => (type === "text/plain" ? text : "") };
  Object.defineProperty(event, "clipboardData", { value: data });
  view.contentDOM.dispatchEvent(event);
}

const vimKeys = (view: EditorView, keys: string) => {
  for (const key of keys) Vim.handleKey(getCM(view)!, key, "user");
};

// Half of a character outside the Basic Multilingual Plane, which no UTF-8 file can hold.
const halfCharacter = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/;
const register = () => Vim.getRegisterController().getRegister('"').toString();

describe("lossless round trip", () => {
  it("returns exactly the given text after any mode, vim and theme toggles", () => {
    fc.assert(
      fc.property(
        fc.oneof(markdownText, uniformText),
        fc.oneof(markdownText, uniformText),
        fc.constantFrom<EditorMode>("source", "styled"),
        fc.boolean(),
        toggles,
        toggles,
        (first, second, mode, mobile, before, after) => {
          const { handle, onChange } = mount({ doc: first, mode, mobile, vim: true });
          apply(handle, before);
          expect(handle.getDoc()).toBe(first);
          handle.setDoc(second);
          expect(handle.getDoc()).toBe(second);
          apply(handle, after);
          expect(handle.getDoc()).toBe(second);
          expect(onChange).not.toHaveBeenCalled();
          cleanup();
        },
      ),
      { numRuns: 300 },
    );
  });

  it("restores the exact bytes when edits made in either mode are undone", () => {
    fc.assert(
      fc.property(
        fc.oneof(markdownText, uniformText),
        fc.array(fc.tuple(fc.nat(), fc.constantFrom("x", "\n", "- [ ] ", "**")), { maxLength: 4 }),
        (doc, edits) => {
          const { handle, view } = mount({ doc });
          for (const [at, text] of edits) {
            handle.setMode(at % 2 ? "styled" : "source");
            view.dispatch({ selection: { anchor: at % (view.state.doc.length + 1) } });
            handle.insertAtCursor(text);
          }
          while (undo(view));
          expect(handle.getDoc()).toBe(doc);
          cleanup();
        },
      ),
      { numRuns: 200 },
    );
  });

  it.each([
    "",
    "\n",
    "# title\r\nbody\r\n",
    "no final newline",
    "mixed\r\nline\nends\r",
    "lone\rcr\r",
    "\r\n\r\n",
    "\r\r\n",
    "﻿# BOM\n",
  ])("keeps %j byte for byte", (doc) => {
    const { handle } = mount({ doc, mode: "styled" });
    handle.setMode("source");
    handle.setVim(true);
    handle.setDark(true);
    handle.setMode("styled");
    expect(handle.getDoc()).toBe(doc);
  });

  it("writes new lines with the file's own line break and shares undo history across modes", () => {
    const doc = "# a\r\nb\r\n";
    const { handle, view, onChange } = mount({ doc });
    view.dispatch({ selection: { anchor: view.state.doc.line(2).to } });
    insertNewlineAndIndent(view);
    handle.insertAtCursor("c");
    expect(handle.getDoc()).toBe("# a\r\nb\r\nc\r\n");
    expect(onChange).toHaveBeenLastCalledWith("# a\r\nb\r\nc\r\n");
    handle.setMode("styled");
    while (undo(view));
    expect(handle.getDoc()).toBe(doc);
  });

  it("keeps CRLF when vim puts a yanked line", () => {
    const { handle, view } = mount({ doc: "one\r\ntwo\r\n", vim: true });
    vimKeys(view, "yyp");
    expect(handle.getDoc()).toBe("one\r\none\r\ntwo\r\n");
  });

  it("pastes with the file's line break, even into a file that mixes them", () => {
    const crlf = mount({ doc: "a\r\nb" });
    paste(crlf.view, "x\ny\r\n");
    expect(crlf.handle.getDoc()).toBe("x\r\ny\r\na\r\nb");

    const mixed = mount({ doc: "a\r\nb\nc" });
    paste(mixed.view, "x\r\ny\rz");
    expect(mixed.handle.getDoc()).toBe("x\ny\nza\r\nb\nc");
  });

  it("keeps a CRLF together when text is typed or pasted at the end of its line", () => {
    const doc = "first\r\nsecond\n";
    const atEnd = (vim = false) => {
      const { handle, view } = mount({ doc, vim });
      view.dispatch({ selection: { anchor: view.state.doc.line(1).to } });
      return { handle, view };
    };

    const typed = atEnd();
    typed.handle.insertAtCursor("X");
    expect(typed.handle.getDoc()).toBe("firstX\r\nsecond\n");
    expect(typed.view.state.selection.main.head).toBe("firstX".length);

    const pasted = atEnd();
    paste(pasted.view, "x\ny");
    expect(pasted.handle.getDoc()).toBe("firstx\ny\r\nsecond\n");

    const appended = atEnd(true);
    vimKeys(appended.view, "A");
    appended.handle.insertAtCursor("X");
    expect(appended.handle.getDoc()).toBe("firstX\r\nsecond\n");

    // A new line there already keeps the "\r" next to a "\n", and the line above untouched.
    const broken = atEnd();
    insertNewlineAndIndent(broken.view);
    expect(broken.handle.getDoc()).toBe("first\r\n\nsecond\n");
  });

  it("never drops the \\r of a CRLF when a new line or typing replaces the end of its line", () => {
    // Enter between the text and its "\r", where the cursor is after typing at the end:
    // CodeMirror drops the spaces after the cursor, but the "\r" stays on the line above.
    const enter = mount({ doc: "first  \r\nsecond\n" });
    enter.view.dispatch({ selection: { anchor: "first".length } });
    insertNewlineAndIndent(enter.view);
    expect(enter.handle.getDoc()).toBe("first\r\n\nsecond\n");

    // Typing over a selection to the end of the line, which takes the "\r" in.
    const over = mount({ doc: "first\r\nsecond\n" });
    over.view.dispatch({ selection: { anchor: 2, head: over.view.state.doc.line(1).to } });
    over.handle.insertAtCursor("X");
    expect(over.handle.getDoc()).toBe("fiX\r\nsecond\n");
    over.view.dispatch({ selection: { anchor: 2, head: over.view.state.doc.line(1).to } });
    paste(over.view, "a\nb");
    expect(over.handle.getDoc()).toBe("fia\nb\r\nsecond\n");

    // Deleting it is still possible.
    const deleted = mount({ doc: "first\r\nsecond\n" });
    deleted.view.dispatch({ selection: { anchor: deleted.view.state.doc.line(1).to } });
    deleteCharBackward(deleted.view);
    expect(deleted.handle.getDoc()).toBe("first\nsecond\n");
  });

  it("starts a fresh history on setDoc", () => {
    const { handle, view } = mount({ doc: "one" });
    handle.insertAtCursor("x");
    handle.setDoc("two");
    expect(undo(view)).toBe(false);
    expect(handle.getDoc()).toBe("two");
  });
});

describe("whole characters under vim", () => {
  it.each([
    ["great run 🙂\n", "0yiw$p", "great run 🙂great\n"],
    ["great run 🙂\n", "0yw$p", "great run 🙂great \n"],
    ["hi 🙂 there", "0w~x", "hi 🙂there"],
    ["a👍🏽b", "0lylp", "a👍👍🏽b"],
    // Vim works on code points, so 👍🏽 is 👍 and a skin tone, for v as for x and yl.
    ["a👍🏽b", "0lvd", "a🏽b"],
    ["a👍🏽b", "0lx", "a🏽b"],
    ["a🙂bc", "0vld", "bc"],
    // Visual mode started on an emoji used to take one character too many (patches/).
    ["a🙂b", "0lvd", "ab"],
    ["hi 🙂there", "0wvld", "hi here"],
    ["x🙂y", "0lvx", "xy"],
    ["🏴󠁧󠁢󠁥󠁮󠁧󠁿 x", "0lx", "🏴󠁢󠁥󠁮󠁧󠁿 x"],
  ])("%j with %s gives %j", (doc, keys, expected) => {
    const { handle, view } = mount({ doc, vim: true });
    vimKeys(view, keys);
    expect(handle.getDoc()).toBe(expected);
    expect(register()).not.toMatch(halfCharacter);
  });

  it("yanks just the emoji a visual selection started on", () => {
    const { view } = mount({ doc: "a🙂b", vim: true });
    vimKeys(view, "0lvy");
    expect(register()).toBe("🙂");
  });

  it("deletes and puts back the whole character", () => {
    const { handle, view } = mount({ doc: "a🙂bc", vim: true });
    vimKeys(view, "0vld");
    expect(register()).toBe("a🙂");
    vimKeys(view, "P");
    expect(handle.getDoc()).toBe("a🙂bc");
  });

  it("never leaves half a character in the document or a register, whatever keys are pressed", () => {
    const text = fc
      .array(fc.constantFrom("a", "b", " ", "\n", "é", "🙂", "👍🏽", "👩‍👩‍👧", "🏴󠁧󠁢󠁥󠁮󠁧󠁿", "𝔸"), {
        minLength: 1,
        maxLength: 12,
      })
      .map((parts) => parts.join(""));
    const commands = [
      "p", "P", "yl", "yiw", "yw", "y$", "~", "x", "X", "l", "h", "w", "b", "e", "$", "0", "j", "k",
      "v", "d", "c", "s", "D", "J", "u", ".", "<Esc>",
    ];
    fc.assert(
      fc.property(text, fc.array(fc.constantFrom(...commands), { maxLength: 16 }), (doc, keys) => {
        const { handle, view } = mount({ doc, vim: true });
        const cm = getCM(view)!;
        for (const key of keys) {
          if (key === "<Esc>") Vim.handleKey(cm, key, "user");
          else vimKeys(view, key);
          if (cm.state.vim!.insertMode) Vim.handleKey(cm, "<Esc>", "user");
        }
        expect(handle.getDoc()).not.toMatch(halfCharacter);
        expect(register()).not.toMatch(halfCharacter);
        cleanup();
      }),
      { numRuns: 300 },
    );
  });
});
