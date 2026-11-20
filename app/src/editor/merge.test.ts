import { undo } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";
import { bothText, createMergeEditor, joinChunks, type MergeChunk, type MergeEditorHandle } from "./merge";
import "./testing";

const mounted: MergeEditorHandle[] = [];

function mount(chunks: MergeChunk[], extra: { mobile?: boolean; snapshot?: ReturnType<MergeEditorHandle["snapshot"]> } = {}) {
  const parent = document.body.appendChild(document.createElement("div"));
  const onChange = vi.fn<(doc: string, unresolved: number) => void>();
  const handle = createMergeEditor(parent, { chunks, dark: false, mobile: false, onChange, ...extra });
  mounted.push(handle);
  return { handle, onChange, parent, view: EditorView.findFromDOM(parent)! };
}

const clash = (mine: string, theirs: string, base = "base\n"): MergeChunk => ({ kind: "clash", base, mine, theirs });

// Different paragraphs changed on each side, one paragraph changed on both.
const CHUNKS: MergeChunk[] = [
  { kind: "same", text: "# Run\n\n" },
  { kind: "mine", text: "Five miles, cold.\n", base: "Five miles.\n" },
  { kind: "same", text: "\n" },
  clash("Saw a heron.\n", "Saw two herons.\n", "Saw a bird.\n"),
  { kind: "same", text: "\n" },
  { kind: "theirs", text: "- [x] stretch\n", base: "- [ ] stretch\n" },
  { kind: "both", text: "", base: "old line\n" },
];

describe("joinChunks", () => {
  it("joins a clean merge, and is null while a clash is unpicked", () => {
    const clean = CHUNKS.filter((c) => c.kind !== "clash");
    expect(joinChunks(clean)).toBe("# Run\n\nFive miles, cold.\n\n\n- [x] stretch\n");
    expect(joinChunks(CHUNKS)).toBeNull();
    expect(joinChunks(CHUNKS, [null])).toBeNull();
  });

  it("uses each pick", () => {
    const at = (pick: "mine" | "theirs" | "both") => joinChunks([clash("a\n", "b\n")], [pick]);
    expect(at("mine")).toBe("a\n");
    expect(at("theirs")).toBe("b\n");
    expect(at("both")).toBe("a\nb\n");
  });

  it("keeps CRLF and a missing final newline byte for byte", () => {
    const chunks: MergeChunk[] = [
      { kind: "same", text: "one\r\n" },
      { kind: "theirs", text: "two!\r\n", base: "two\r\n" },
      clash("end", "END", "End"),
    ];
    expect(joinChunks(chunks, ["theirs"])).toBe("one\r\ntwo!\r\nEND");
    expect(joinChunks(chunks, ["both"])).toBe("one\r\ntwo!\r\nend\nEND");
  });
});

describe("bothText", () => {
  it("puts mine first and adds a line break only where mine has none", () => {
    expect(bothText("a\n", "b\n")).toBe("a\nb\n");
    expect(bothText("a", "b")).toBe("a\nb");
    expect(bothText("a\r\nb", "c\r\n")).toBe("a\r\nb\r\nc\r\n");
    expect(bothText("a\r\n", "b\r\n")).toBe("a\r\nb\r\n");
    expect(bothText("", "b\n")).toBe("b\n");
    expect(bothText("a\n", "")).toBe("a\n");
  });
});

describe("merge editor", () => {
  it("starts with every clash empty and counts it as unresolved", () => {
    const { handle, parent } = mount(CHUNKS);
    expect(handle.getDoc()).toBe("# Run\n\nFive miles, cold.\n\n\n- [x] stretch\n");
    expect(handle.unresolved()).toBe(1);
    const block = parent.querySelector(".mdgj-clash")!;
    expect(block.textContent).toContain("Saw a heron.");
    expect(block.textContent).toContain("Saw two herons.");
    expect([...block.querySelectorAll("button")].map((b) => b.textContent)).toEqual([
      "Use mine",
      "Use GitHub's",
      "Use both",
    ]);
  });

  it("tints what came from each side, and marks deleted lines", () => {
    const { parent } = mount(CHUNKS);
    expect(parent.querySelector(".cm-line.mdgj-merge-mine")?.textContent).toBe("Five miles, cold.");
    expect(parent.querySelector(".cm-line.mdgj-merge-theirs")?.textContent).toBe("- [x] stretch");
    expect(parent.querySelector(".mdgj-merge-deleted.mdgj-merge-both")?.getAttribute("title")).toBe(
      "Deleted in both versions: old line",
    );
  });

  for (const [pick, text] of [
    ["mine", "Saw a heron.\n"],
    ["theirs", "Saw two herons.\n"],
    ["both", "Saw a heron.\nSaw two herons.\n"],
  ] as const) {
    it(`"Use ${pick}" puts that text at the clash`, () => {
      const { handle, onChange, parent } = mount(CHUNKS);
      handle.pick(0, pick);
      const doc = `# Run\n\nFive miles, cold.\n\n${text}\n- [x] stretch\n`;
      expect(handle.getDoc()).toBe(doc);
      expect(joinChunks(CHUNKS, [pick])).toBe(doc);
      expect(handle.unresolved()).toBe(0);
      expect(onChange).toHaveBeenLastCalledWith(doc, 0);
      expect(parent.querySelector(".mdgj-clash")).toBeNull();
      expect(parent.querySelectorAll(".cm-line.mdgj-merge-picked").length).toBe(pick === "both" ? 2 : 1);
    });
  }

  it("the block's buttons pick", () => {
    const { handle, parent } = mount(CHUNKS);
    const button = [...parent.querySelectorAll<HTMLButtonElement>(".mdgj-clash button")].find(
      (b) => b.textContent === "Use GitHub's",
    )!;
    button.click();
    expect(handle.getDoc()).toContain("Saw two herons.\n");
    expect(handle.unresolved()).toBe(0);
  });

  it("a second pick of the same clash does nothing", () => {
    const { handle } = mount(CHUNKS);
    handle.pick(0, "mine");
    handle.pick(0, "theirs");
    expect(handle.getDoc()).toContain("Saw a heron.\n");
    expect(handle.getDoc()).not.toContain("herons");
  });

  it("undo takes a pick back, and redo makes it again", () => {
    const { handle, onChange, view } = mount(CHUNKS);
    const before = handle.getDoc();
    handle.pick(0, "theirs");
    expect(handle.undoDepth()).toBe(1);
    expect(undo(view)).toBe(true);
    expect(handle.getDoc()).toBe(before);
    expect(handle.unresolved()).toBe(1);
    expect(onChange).toHaveBeenLastCalledWith(before, 1);
    expect(handle.redo()).toBe(true);
    expect(handle.getDoc()).toContain("Saw two herons.\n");
    expect(handle.unresolved()).toBe(0);
  });

  it("a pick is its own undo step, apart from typing just before it", () => {
    const { handle, view } = mount(CHUNKS);
    view.dispatch({ changes: { from: 0, insert: "x" }, userEvent: "input.type" });
    handle.pick(0, "mine");
    expect(handle.undo()).toBe(true);
    expect(handle.getDoc().startsWith("x# Run")).toBe(true);
    expect(handle.unresolved()).toBe(1);
  });

  it("an empty side is a pick too, with nothing inserted", () => {
    const { handle, onChange } = mount([{ kind: "same", text: "a\n" }, clash("", "b\n"), { kind: "same", text: "c\n" }]);
    handle.pick(0, "mine");
    expect(handle.getDoc()).toBe("a\nc\n");
    expect(handle.unresolved()).toBe(0);
    expect(onChange).toHaveBeenLastCalledWith("a\nc\n", 0);
    handle.undo();
    expect(handle.unresolved()).toBe(1);
  });

  it("editing before, after and at the clash keeps it in its place", () => {
    const { handle, view } = mount(CHUNKS);
    const slot = handle.getDoc().indexOf("\n- [x]");
    // Type at the start of the line after the clash, on the line before it, and above.
    view.dispatch({ changes: { from: slot, insert: "after " } });
    view.dispatch({ changes: { from: handle.getDoc().indexOf(".\n") + 1, insert: " before" } });
    view.dispatch({ changes: { from: 0, insert: "Title: " } });
    handle.pick(0, "mine");
    expect(handle.getDoc()).toBe(
      "Title: # Run\n\nFive miles, cold. before\n\nSaw a heron.\nafter \n- [x] stretch\n",
    );
  });

  it("free edits inside a picked clash are kept, and so is the pick", () => {
    const { handle, view } = mount(CHUNKS);
    handle.pick(0, "mine");
    const at = handle.getDoc().indexOf("heron") + "heron".length;
    view.dispatch({ changes: { from: at, insert: " by the lock" } });
    expect(handle.getDoc()).toContain("Saw a heron by the lock.\n");
    expect(handle.unresolved()).toBe(0);
  });

  it("deleting a tinted region's text removes its tint", () => {
    const { parent, view, handle } = mount(CHUNKS);
    const from = handle.getDoc().indexOf("Five");
    view.dispatch({ changes: { from, to: from + "Five miles, cold.\n".length } });
    expect(parent.querySelector(".cm-line.mdgj-merge-mine")).toBeNull();
  });

  it("keeps CRLF and a missing final newline, and inserts picks with the file's line break", () => {
    const chunks: MergeChunk[] = [
      { kind: "same", text: "one\r\n" },
      { kind: "mine", text: "two!\r\n", base: "two\r\n" },
      clash("three\r\nmore\r\n", "THREE\r\n", "3\r\n"),
      { kind: "same", text: "end" },
    ];
    const { handle } = mount(chunks);
    expect(handle.getDoc()).toBe("one\r\ntwo!\r\nend");
    handle.pick(0, "both");
    expect(handle.getDoc()).toBe("one\r\ntwo!\r\nthree\r\nmore\r\nTHREE\r\nend");
    expect(handle.getDoc()).toBe(joinChunks(chunks, ["both"]));
  });

  it("keeps a file's mixed line breaks byte for byte", () => {
    const chunks: MergeChunk[] = [
      { kind: "same", text: "dos\r\n" },
      clash("unix\n", "mac\r\n"),
      { kind: "theirs", text: "tail", base: "tail\n" },
    ];
    for (const pick of ["mine", "theirs", "both"] as const) {
      const { handle } = mount(chunks);
      handle.pick(0, pick);
      expect(handle.getDoc()).toBe(joinChunks(chunks, [pick]));
    }
  });

  it("never writes a conflict marker", () => {
    const { handle } = mount(CHUNKS);
    for (const doc of [handle.getDoc(), (handle.pick(0, "both"), handle.getDoc())]) {
      expect(doc).not.toMatch(/^(<{7}|={7}|>{7})/m);
    }
  });

  it("several clashes are numbered and picked by index in document order", () => {
    const chunks: MergeChunk[] = [clash("a\n", "A\n"), { kind: "same", text: "-\n" }, clash("b\n", "B\n")];
    const { handle, parent } = mount(chunks);
    expect([...parent.querySelectorAll(".mdgj-clash-head")].map((h) => h.textContent)).toEqual([
      "Changed on both sides · 1 of 2",
      "Changed on both sides · 2 of 2",
    ]);
    handle.pick(1, "theirs");
    expect(handle.getDoc()).toBe("-\nB\n");
    expect(handle.unresolved()).toBe(1);
    handle.pick(0, "mine");
    expect(handle.getDoc()).toBe("a\n-\nB\n");
  });

  it("a snapshot reopens the text and the picks, with a fresh undo history", () => {
    const first = mount(CHUNKS);
    first.handle.pick(0, "theirs");
    first.view.dispatch({ changes: { from: 0, insert: "Edited. " } });
    const snapshot = first.handle.snapshot();
    first.handle.destroy();

    const again = mount(CHUNKS, { snapshot });
    expect(again.handle.getDoc()).toBe(first.handle.getDoc());
    expect(again.handle.unresolved()).toBe(0);
    expect(again.parent.querySelector(".cm-line.mdgj-merge-picked")?.textContent).toBe("Saw two herons.");
    expect(again.handle.undoDepth()).toBe(0);
    expect(again.handle.undo()).toBe(false);
    again.view.dispatch({ changes: { from: 0, insert: "More. " } });
    again.handle.undo();
    expect(again.handle.getDoc()).toBe(first.handle.getDoc());
    expect(again.handle.unresolved()).toBe(0);
  });

  it("on the phone, has no line numbers and large buttons", () => {
    const { parent } = mount(CHUNKS, { mobile: true });
    expect(parent.querySelector(".cm-gutters")).toBeNull();
    expect(parent.querySelector(".cm-editor.mdgj-merge-touch")).not.toBeNull();
  });
});
