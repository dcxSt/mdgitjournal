import { EditorView } from "@codemirror/view";
import { type CodeMirrorV, getCM, Vim } from "@replit/codemirror-vim";
import { describe, expect, it, vi } from "vitest";
import { mount } from "./testing";

interface Clipboard {
  files?: File[];
  text?: string;
  html?: string;
}

function transfer({ files = [], text = "", html = "" }: Clipboard) {
  return { files, getData: (type: string) => ({ "text/plain": text, "text/html": html })[type] ?? "" };
}

function paste(view: EditorView, clipboard: Clipboard): Event {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: transfer(clipboard) });
  view.contentDOM.dispatchEvent(event);
  return event;
}

function drop(view: EditorView, clipboard: Clipboard): Event {
  const event = new MouseEvent("drop", { bubbles: true, cancelable: true, clientX: 5, clientY: 5 });
  Object.defineProperty(event, "dataTransfer", { value: transfer(clipboard) });
  view.contentDOM.dispatchEvent(event);
  return event;
}

function key(view: EditorView, init: KeyboardEventInit): Event {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  view.contentDOM.dispatchEvent(event);
  return event;
}

const png = (name = "shot.png") =>
  new File([new Uint8Array([137, 80, 78, 71])], name, { type: "image/png" });

/** An onImageFiles whose promise the test settles. */
function importer() {
  let settle!: (paths: string[]) => void;
  const onImageFiles = vi.fn(
    (_files: File[]) => new Promise<string[]>((resolve) => (settle = resolve)),
  );
  return { onImageFiles, settle: (paths: string[]) => settle(paths) };
}

describe("image paste and drop", () => {
  it("links each imported image at the cursor, on its own line", async () => {
    const { onImageFiles, settle } = importer();
    const { view, handle } = mount({ doc: "Morning run\r\nbody", onImageFiles });
    view.dispatch({ selection: { anchor: "Morning run".length } });

    const file = png();
    expect(paste(view, { files: [file] }).defaultPrevented).toBe(true);
    expect(onImageFiles).toHaveBeenCalledWith([file]);
    settle(["assets/run-1.jpg", "assets/run-2.jpg"]);

    await vi.waitFor(() =>
      expect(handle.getDoc()).toBe(
        "Morning run\r\n![](assets/run-1.jpg)\r\n![](assets/run-2.jpg)\r\nbody",
      ),
    );
    expect(view.state.selection.main.head).toBe(view.state.doc.line(3).to);
  });

  it("follows the insert position while the import runs", async () => {
    const { onImageFiles, settle } = importer();
    const { view, handle } = mount({ doc: "ab", onImageFiles });
    view.dispatch({ selection: { anchor: 2 } });
    paste(view, { files: [png()] });
    view.dispatch({ changes: { from: 0, insert: "typed " } });
    settle(["assets/x-1.jpg"]);
    await vi.waitFor(() => expect(handle.getDoc()).toBe("typed ab\n![](assets/x-1.jpg)"));
  });

  it("drops the links when another entry was loaded meanwhile, and says so", async () => {
    const { onImageFiles, settle } = importer();
    const onImagesUnlinked = vi.fn();
    const { handle, view, onChange } = mount({ doc: "one", onImageFiles, onImagesUnlinked });
    paste(view, { files: [png()] });
    handle.setDoc("two");
    settle(["assets/one-1.jpg"]);
    await vi.waitFor(() => expect(onImagesUnlinked).toHaveBeenCalledWith(["assets/one-1.jpg"]));
    expect(handle.getDoc()).toBe("two");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("reports nothing when the links were inserted or the import gave none", async () => {
    const { onImageFiles, settle } = importer();
    const onImagesUnlinked = vi.fn();
    const { handle, view } = mount({ doc: "", onImageFiles, onImagesUnlinked });
    paste(view, { files: [png()] });
    settle(["assets/x-1.jpg"]);
    await vi.waitFor(() => expect(handle.getDoc()).toBe("![](assets/x-1.jpg)"));
    paste(view, { files: [png()] });
    handle.setDoc("two");
    settle([]);
    await new Promise((resolve) => setTimeout(resolve));
    expect(onImagesUnlinked).not.toHaveBeenCalled();
  });

  it("importImages (the phone's photo picker) keeps the selected text", async () => {
    const { onImageFiles, settle } = importer();
    const onImagesUnlinked = vi.fn();
    const { view, handle } = mount({ doc: "Five miles", onImageFiles, onImagesUnlinked });
    view.dispatch({ selection: { anchor: 0, head: 4 } });
    const file = png();
    handle.importImages([file]);
    expect(onImageFiles).toHaveBeenCalledWith([file]);
    settle(["assets/x-1.jpg"]);
    await vi.waitFor(() => expect(handle.getDoc()).toBe("Five\n![](assets/x-1.jpg)\n miles"));

    // Destroyed before the import finished: reported, like a paste.
    handle.importImages([png()]);
    handle.destroy();
    settle(["assets/x-2.jpg"]);
    await vi.waitFor(() => expect(onImagesUnlinked).toHaveBeenCalledWith(["assets/x-2.jpg"]));
  });

  it("pastes text, not the picture of it, from rich-text apps", () => {
    const { onImageFiles } = importer();
    const { view, handle } = mount({ doc: "", onImageFiles });
    paste(view, { files: [png()], text: "copied cells", html: "<table>…</table>" });
    expect(onImageFiles).not.toHaveBeenCalled();
    expect(handle.getDoc()).toBe("copied cells");
  });

  it("imports a copied image file rather than pasting its name", () => {
    const { onImageFiles } = importer();
    const { view, handle } = mount({ doc: "", onImageFiles });
    paste(view, { files: [png("cat.png")], text: "cat.png" });
    expect(onImageFiles).toHaveBeenCalledOnce();
    expect(handle.getDoc()).toBe("");
  });

  it("leaves pastes without images alone", () => {
    const { onImageFiles } = importer();
    const { view, handle } = mount({ doc: "", onImageFiles });
    const pdf = new File(["%PDF"], "a.pdf", { type: "application/pdf" });
    paste(view, { files: [pdf], text: "plain" });
    expect(onImageFiles).not.toHaveBeenCalled();
    expect(handle.getDoc()).toBe("plain");
  });

  it("ignores images when the app does not import them", () => {
    const { view, handle } = mount({ doc: "" });
    paste(view, { files: [png()] });
    expect(handle.getDoc()).toBe("");
  });

  it("links dropped images where they were dropped", async () => {
    const { onImageFiles, settle } = importer();
    const { view, handle } = mount({ doc: "first\nsecond", onImageFiles });
    vi.spyOn(view, "posAtCoords").mockReturnValue("first".length);
    const notes = new File(["x"], "notes.txt", { type: "text/plain" });
    expect(drop(view, { files: [png(), notes] }).defaultPrevented).toBe(true);
    expect(onImageFiles).toHaveBeenCalledWith([expect.objectContaining({ name: "shot.png" })]);
    settle(["assets/a-1.jpg"]);
    await vi.waitFor(() => expect(handle.getDoc()).toBe("first\n![](assets/a-1.jpg)\nsecond"));
  });
});

describe("keys", () => {
  it("saves with Mod-s", () => {
    const onSave = vi.fn();
    const { view } = mount({ onSave });
    key(view, { key: "s", ctrlKey: true });
    expect(onSave).toHaveBeenCalledOnce();
  });

  it("wraps the selection in bold with Mod-b and italics with Mod-i", () => {
    const { view, handle } = mount({ doc: "a word" });
    view.dispatch({ selection: { anchor: 2, head: 6 } });
    key(view, { key: "b", ctrlKey: true });
    expect(handle.getDoc()).toBe("a **word**");
    key(view, { key: "i", ctrlKey: true });
    expect(handle.getDoc()).toBe("a **_word_**");
  });

  it("indents with Tab instead of inserting one", () => {
    const { view, handle } = mount({ doc: "- item" });
    key(view, { key: "Tab" });
    expect(handle.getDoc()).toBe("  - item");
  });

  it("inserts nothing on its own: no bracket closing, no list continuation", () => {
    const { view, handle } = mount({ doc: "- [ ] task" });
    view.dispatch({ selection: { anchor: view.state.doc.length } });
    key(view, { key: "Enter" });
    expect(handle.getDoc()).toBe("- [ ] task\n");
    view.dispatch(view.state.replaceSelection("("));
    expect(handle.getDoc()).toBe("- [ ] task\n(");
  });
});

describe("vim", () => {
  it("saves on :w and leaves insert mode on Esc", () => {
    const onSave = vi.fn();
    const { view } = mount({ vim: true, onSave });
    // getCM's type allows a missing vim state, which the running plugin always has.
    const cm = getCM(view) as CodeMirrorV;
    Vim.handleEx(cm, "w");
    expect(onSave).toHaveBeenCalledOnce();
    Vim.handleKey(cm, "i", "user");
    expect(cm.state.vim?.insertMode).toBe(true);
    Vim.handleKey(cm, "<Esc>", "user");
    expect(cm.state.vim?.insertMode).toBe(false);
  });

  it("toggles without touching the document or history", () => {
    const { view, handle } = mount({ doc: "text" });
    expect(getCM(view)).toBeNull();
    handle.setVim(true);
    expect(getCM(view)).not.toBeNull();
    handle.setVim(false);
    expect(getCM(view)).toBeNull();
    expect(handle.getDoc()).toBe("text");
  });

  it("jumps in a shorter entry after jumping far into a longer one", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const long = Array.from({ length: 200 }, (_, n) => `line ${n}`).join("\n");
    const { view, handle } = mount({ doc: long, vim: true });
    expect(key(view, { key: "G", shiftKey: true }).defaultPrevented).toBe(true);
    Vim.handleKey(getCM(view)!, "y", "user");
    Vim.handleKey(getCM(view)!, "y", "user");

    const short = "short\nentry\n";
    handle.setDoc(short);
    view.dispatch({ selection: { anchor: 3 } });
    // A key vim fails on is not consumed, and the browser types it into the entry.
    expect(key(view, { key: "g" }).defaultPrevented).toBe(true);
    expect(key(view, { key: "g" }).defaultPrevented).toBe(true);
    expect(view.state.selection.main.head).toBe(0);
    expect(key(view, { key: "G", shiftKey: true }).defaultPrevented).toBe(true);
    expect(view.state.selection.main.head).toBe(short.length);
    expect(error).not.toHaveBeenCalled();
    expect(handle.getDoc()).toBe(short);
    // Registers carry over to the next entry.
    expect(Vim.getRegisterController().getRegister('"').toString()).toBe("line 199\n");
  });

  it("jumps after the document shrank while vim was off", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const long = Array.from({ length: 200 }, (_, n) => `line ${n}`).join("\n");
    const { view, handle } = mount({ doc: long, vim: true });
    key(view, { key: "G", shiftKey: true });
    handle.setVim(false);
    view.dispatch({ changes: { from: 10, to: long.length } });
    handle.setVim(true);
    expect(key(view, { key: "g" }).defaultPrevented).toBe(true);
    expect(key(view, { key: "g" }).defaultPrevented).toBe(true);
    expect(error).not.toHaveBeenCalled();
    expect(handle.getDoc()).toBe(long.slice(0, 10));
  });

  it("puts over a backward visual selection (codemirror-vim threw there, after deleting it)", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const { view, handle } = mount({ doc: "hello world", vim: true });
    for (const k of "0yiwwvb") key(view, { key: k });
    expect(key(view, { key: "p" }).defaultPrevented).toBe(true);
    expect(handle.getDoc()).toBe("helloorld");
    expect(error).not.toHaveBeenCalled();
  });

  it("drops text that reaches the document outside insert mode, such as a key vim failed on", () => {
    const { view, handle } = mount({ doc: "hello", vim: true });
    const input = (text: string) =>
      view.state.facet(EditorView.inputHandler).some((handler) =>
        handler(view, 0, 0, text, () => view.state.update({ changes: { from: 0, insert: text } })),
      );
    expect(input("p")).toBe(true);
    key(view, { key: "i" });
    expect(input("p")).toBe(false);
    expect(handle.getDoc()).toBe("hello");
  });

  it("keeps :set options across entries", () => {
    const { view, handle } = mount({ vim: true });
    Vim.handleEx(getCM(view) as CodeMirrorV, "set nopcre");
    handle.setDoc("next");
    expect(Vim.getOption("pcre")).toBe(false);
    Vim.setOption("pcre", true);
  });

  it("is never enabled on mobile", () => {
    const { view, handle } = mount({ vim: true, mobile: true });
    expect(getCM(view)).toBeNull();
    handle.setVim(true);
    expect(getCM(view)).toBeNull();
  });
});

describe("handle", () => {
  it("keeps mode, vim and theme across setDoc", () => {
    const { view, handle } = mount({ doc: "one", mode: "styled", vim: true, dark: true });
    handle.setDoc("# two\n");
    expect(view.contentDOM.querySelector(".cm-md-h1")).not.toBeNull();
    expect(getCM(view)).not.toBeNull();
    expect(view.state.facet(EditorView.darkTheme)).toBe(true);
    handle.setDark(false);
    expect(view.state.facet(EditorView.darkTheme)).toBe(false);
  });

  it("inserts at the cursor with the file's line break", () => {
    const { view, handle, onChange } = mount({ doc: "a\r\nb" });
    view.dispatch({ selection: { anchor: 1 } });
    handle.insertAtCursor("\n- [ ] ");
    expect(handle.getDoc()).toBe("a\r\n- [ ] \r\nb");
    expect(onChange).toHaveBeenLastCalledWith("a\r\n- [ ] \r\nb");
  });

  it("wraps the selection and toggles line prefixes for the toolbar", () => {
    const { view, handle } = mount({ doc: "milk" });
    view.dispatch({ selection: { anchor: 0, head: 4 } });
    handle.wrapSelection("**", "**");
    expect(handle.getDoc()).toBe("**milk**");
    handle.toggleLinePrefix("- [ ] ");
    expect(handle.getDoc()).toBe("- [ ] **milk**");
    handle.toggleLinePrefix("- [ ] ");
    expect(handle.getDoc()).toBe("**milk**");
  });

  it("asks the platform for spelling help only in styled mode", () => {
    const { view, handle } = mount({ mode: "styled" });
    expect(view.contentDOM.getAttribute("spellcheck")).toBe("true");
    handle.setMode("source");
    expect(view.contentDOM.getAttribute("spellcheck")).toBe("false");
  });

  it("removes itself on destroy", () => {
    const { view, handle } = mount();
    handle.destroy();
    expect(view.dom.isConnected).toBe(false);
  });
});
