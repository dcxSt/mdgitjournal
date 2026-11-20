import { EditorSelection } from "@codemirror/state";
import { getCM, Vim } from "@replit/codemirror-vim";
import { describe, expect, it, vi } from "vitest";
import { mount } from "./testing";

const png = () => new File([new Uint8Array([137, 80, 78, 71])], "shot.png", { type: "image/png" });

function paste(target: HTMLElement, files: File[]): Event {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  const data = { files, getData: () => "" };
  Object.defineProperty(event, "clipboardData", { value: data });
  target.dispatchEvent(event);
  return event;
}

describe("setReadOnly", () => {
  it("stops every edit, and the text is exactly as it was", () => {
    const doc = "one\r\ntwo\r\n";
    const { handle, view, onChange } = mount({ doc });
    handle.setReadOnly(true);
    view.dispatch({ changes: { from: 0, insert: "x" }, userEvent: "input.type" });
    handle.insertAtCursor("typed");
    handle.wrapSelection("**", "**");
    handle.toggleLinePrefix("# ");
    expect(handle.getDoc()).toBe(doc);
    expect(onChange).not.toHaveBeenCalled();
    expect(view.state.readOnly).toBe(true);
  });

  it("keeps selection and moving the cursor working", () => {
    const { handle, view } = mount({ doc: "one\ntwo\n" });
    handle.setReadOnly(true);
    view.dispatch({ selection: EditorSelection.cursor(5) });
    expect(view.state.selection.main.head).toBe(5);
  });

  it("stops vim's edits too", () => {
    const { handle, view } = mount({ doc: "one\ntwo\n", vim: true });
    handle.setReadOnly(true);
    const cm = getCM(view)!;
    Vim.handleKey(cm, "d", "test");
    Vim.handleKey(cm, "d", "test");
    Vim.handleKey(cm, "x", "test");
    expect(handle.getDoc()).toBe("one\ntwo\n");
  });

  it("edits work again once it is off, and the mode switches never change the text", () => {
    const { handle, view } = mount({ doc: "a\n" });
    handle.setReadOnly(true);
    handle.setMode("styled");
    handle.setReadOnly(false);
    handle.setMode("source");
    expect(handle.getDoc()).toBe("a\n");
    view.dispatch({ changes: { from: 0, insert: "b" } });
    expect(handle.getDoc()).toBe("ba\n");
  });

  it("survives setDoc", () => {
    const { handle } = mount({ doc: "a\n" });
    handle.setReadOnly(true);
    handle.setDoc("reloaded\n");
    handle.insertAtCursor("x");
    expect(handle.getDoc()).toBe("reloaded\n");
  });

  it("links an image imported meanwhile only once it is off", async () => {
    let settle!: (paths: string[]) => void;
    const onImageFiles = vi.fn(() => new Promise<string[]>((resolve) => (settle = resolve)));
    const { handle, view } = mount({ doc: "text\n", onImageFiles });
    view.dispatch({ selection: EditorSelection.cursor(5) });
    expect(paste(view.contentDOM, [png()]).defaultPrevented).toBe(true);
    handle.setReadOnly(true);
    settle(["assets/a-1.jpg"]);
    await new Promise((resolve) => setTimeout(resolve));
    expect(handle.getDoc()).toBe("text\n");
    handle.setReadOnly(false);
    expect(handle.getDoc()).toBe("text\n![](assets/a-1.jpg)");
  });

  it("reports an image imported meanwhile as unlinked if the document is replaced", async () => {
    let settle!: (paths: string[]) => void;
    const onImageFiles = vi.fn(() => new Promise<string[]>((resolve) => (settle = resolve)));
    const onImagesUnlinked = vi.fn();
    const { handle, view } = mount({ doc: "text\n", onImageFiles, onImagesUnlinked });
    paste(view.contentDOM, [png()]);
    handle.setReadOnly(true);
    settle(["assets/a-1.jpg"]);
    await new Promise((resolve) => setTimeout(resolve));
    handle.setDoc("pulled\n");
    expect(onImagesUnlinked).toHaveBeenCalledWith(["assets/a-1.jpg"]);
    handle.setReadOnly(false);
    expect(handle.getDoc()).toBe("pulled\n");
  });
});

describe("setDoc with keepCursor", () => {
  it("keeps the cursor's line and column where the new text has them", () => {
    const { handle, view } = mount({ doc: "first\nsecond line\nthird\n" });
    view.dispatch({ selection: EditorSelection.cursor("first\nsecond".length) });
    handle.setDoc("first\nSECOND line, edited on GitHub\nthird\n", { keepCursor: true });
    expect(view.state.selection.main.head).toBe("first\nSECOND".length);
  });

  it("clamps to a shorter text", () => {
    const { handle, view } = mount({ doc: "a\nb\nc long line\n" });
    view.dispatch({ selection: EditorSelection.cursor("a\nb\nc long".length) });
    handle.setDoc("a\nb", { keepCursor: true });
    expect(view.state.selection.main.head).toBe(3);
  });

  it("never lands between the halves of an emoji", () => {
    const { handle, view } = mount({ doc: "abc\n" });
    view.dispatch({ selection: EditorSelection.cursor(2) });
    handle.setDoc("a🙂\n", { keepCursor: true });
    expect(view.state.selection.main.head).toBe(1);
  });

  it("without it, the cursor starts at the top", () => {
    const { handle, view } = mount({ doc: "first\nsecond\n" });
    view.dispatch({ selection: EditorSelection.cursor(8) });
    handle.setDoc("first\nsecond\n");
    expect(view.state.selection.main.head).toBe(0);
  });
});
