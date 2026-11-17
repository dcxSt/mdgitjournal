import type { EditorView } from "@codemirror/view";
import { describe, expect, it, vi } from "vitest";
import { mount } from "./testing";

const doc = [
  "# Title",
  "## Sub",
  "Some **bold**, _italic_, ~~gone~~ and `code`.",
  "A [link](https://example.com) here.",
  "> quoted",
  "```js",
  "let x = 1;",
  "```",
  "---",
  "- item",
  "- [ ] open task",
  "- [x] done task",
  "",
].join("\n");

const lines = (view: EditorView) => Array.from(view.contentDOM.querySelectorAll(".cm-line"));
const line = (view: EditorView, n: number) => lines(view)[n - 1]!;
const marks = (view: EditorView, n: number) =>
  Array.from(line(view, n).querySelectorAll(".cm-md-mark"), (el) => el.textContent);

function click(el: Element) {
  el.dispatchEvent(new MouseEvent("mousedown", { button: 0, bubbles: true, cancelable: true }));
}

describe("styled mode", () => {
  it("keeps every character of the markdown on screen", () => {
    const { view } = mount({ doc, mode: "styled" });
    expect(lines(view).map((el) => el.textContent)).toEqual(doc.split("\n"));
  });

  it("sizes headings and dims their marks", () => {
    const { view } = mount({ doc, mode: "styled" });
    expect(line(view, 1).classList).toContain("cm-md-h1");
    expect(line(view, 2).classList).toContain("cm-md-h2");
    expect(marks(view, 1)).toEqual(["#"]);
    expect(marks(view, 2)).toEqual(["##"]);
  });

  it("styles inline markup and keeps its syntax visible but dimmed", () => {
    const { view } = mount({ doc, mode: "styled" });
    const prose = line(view, 3);
    expect(prose.querySelector(".cm-md-strong")?.textContent).toBe("**bold**");
    expect(prose.querySelector(".cm-md-em")?.textContent).toBe("_italic_");
    expect(prose.querySelector(".cm-md-strike")?.textContent).toBe("~~gone~~");
    expect(prose.querySelector(".cm-md-code")?.textContent).toBe("`code`");
    expect(marks(view, 3)).toEqual(["**", "**", "_", "_", "~~", "~~", "`", "`"]);
  });

  it("colours links and dims their target", () => {
    const { view } = mount({ doc, mode: "styled" });
    expect(line(view, 4).querySelector(".cm-md-link")?.textContent).toBe("[link](https://example.com)");
    expect(marks(view, 4)).toEqual(["[", "]", "(", "https://example.com", ")"]);
  });

  it("draws quotes, code blocks, rules and list markers", () => {
    const { view } = mount({ doc, mode: "styled" });
    expect(line(view, 5).classList).toContain("cm-md-quote");
    expect(marks(view, 5)).toEqual([">"]);
    for (const n of [6, 7, 8]) expect(line(view, n).classList).toContain("cm-md-codeblock");
    expect(marks(view, 9)).toEqual(["---"]);
    expect(marks(view, 10)).toEqual(["-"]);
  });

  it("mutes the text of checked tasks only", () => {
    const { view } = mount({ doc, mode: "styled" });
    expect(line(view, 11).querySelector(".cm-md-task-done")).toBeNull();
    expect(line(view, 12).querySelector(".cm-md-task-done")?.textContent).toBe(" done task");
  });

  it("leaves source mode to syntax highlighting, with line numbers", () => {
    const { view, handle } = mount({ doc, mode: "styled" });
    expect(view.dom.querySelector(".cm-lineNumbers")).toBeNull();
    handle.setMode("source");
    expect(view.dom.querySelector(".cm-lineNumbers")).not.toBeNull();
    expect(view.contentDOM.querySelector("[class*='cm-md-']")).toBeNull();
    expect(line(view, 1).querySelector(".cm-src-heading")).not.toBeNull();
    expect(lines(view).map((el) => el.textContent)).toEqual(doc.split("\n"));
  });

  it("keeps the selection when the mode changes", () => {
    const { view, handle } = mount({ doc });
    view.dispatch({ selection: { anchor: 3, head: 9 } });
    handle.setMode("styled");
    handle.setMode("source");
    expect(view.state.selection.main).toMatchObject({ anchor: 3, head: 9 });
  });
});

describe("task boxes", () => {
  it("toggle exactly the one character between the brackets", () => {
    const { view, handle, onChange } = mount({ doc: "- [ ] milk\r\n- [X] eggs\r\n", mode: "styled" });
    const box = () => view.contentDOM.querySelector(".cm-md-task")!;

    click(box());
    expect(handle.getDoc()).toBe("- [x] milk\r\n- [X] eggs\r\n");
    click(box());
    expect(handle.getDoc()).toBe("- [ ] milk\r\n- [X] eggs\r\n");
    click(view.contentDOM.querySelectorAll(".cm-md-task")[1]!);
    expect(handle.getDoc()).toBe("- [ ] milk\r\n- [ ] eggs\r\n");
    expect(onChange).toHaveBeenCalledTimes(3);
  });

  it("are plain text in source mode", () => {
    const { view, handle } = mount({ doc: "- [ ] milk\n" });
    expect(view.contentDOM.querySelector(".cm-md-task")).toBeNull();
    handle.setMode("styled");
    expect(view.contentDOM.querySelector(".cm-md-task")).not.toBeNull();
  });
});

describe("image previews", () => {
  const resolveImageUrl = (path: string) => `asset://localhost/journal/${path}`;
  const previews = (view: EditorView) =>
    Array.from(view.contentDOM.querySelectorAll<HTMLImageElement>(".cm-md-image img"), (img) =>
      img.getAttribute("src"),
    );

  it("appear below the image's line, keeping its text", () => {
    const text = "before\n![a cat](assets/cat-1.jpg) after\nnext\n";
    const { view } = mount({ doc: text, mode: "styled", resolveImageUrl });
    expect(previews(view)).toEqual(["asset://localhost/journal/assets/cat-1.jpg"]);
    const preview = view.contentDOM.querySelector(".cm-md-image")!;
    expect(preview.previousElementSibling?.textContent).toBe("![a cat](assets/cat-1.jpg) after");
    expect(lines(view).map((el) => el.textContent)).toEqual(text.split("\n"));
  });

  it("load http(s) as is, relative paths through the app, and nothing else", () => {
    const resolve = vi.fn(resolveImageUrl);
    const text = [
      "![](https://example.com/a.png)",
      "![](<assets/my photo.jpg>)",
      "![](assets/my%20photo.jpg)",
      "![](/etc/passwd)",
      "![](file:///etc/passwd)",
      "![](javascript:alert(1))",
      "![](//example.com/a.png)",
    ].join("\n");
    const { view } = mount({ doc: text, mode: "styled", resolveImageUrl: resolve });
    expect(previews(view)).toEqual([
      "https://example.com/a.png",
      "asset://localhost/journal/assets/my photo.jpg",
      "asset://localhost/journal/assets/my photo.jpg",
    ]);
    expect(resolve.mock.calls).toEqual([["assets/my photo.jpg"], ["assets/my photo.jpg"]]);
  });

  it("hide when the image fails to load", () => {
    const { view } = mount({ doc: "![](assets/gone.jpg)\n", mode: "styled", resolveImageUrl });
    const preview = view.contentDOM.querySelector<HTMLElement>(".cm-md-image")!;
    preview.querySelector("img")!.dispatchEvent(new Event("error"));
    expect(preview.style.display).toBe("none");
  });

  it("only show in styled mode", () => {
    const { view, handle } = mount({ doc: "![](assets/a.jpg)\n", resolveImageUrl });
    expect(previews(view)).toEqual([]);
    handle.setMode("styled");
    expect(previews(view)).toHaveLength(1);
  });
});
