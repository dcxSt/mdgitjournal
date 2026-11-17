import { EditorSelection, EditorState, type TransactionSpec } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { insertImageLinks, toggleLinePrefix, wrapSelection } from "./commands";

/** A state from text where "|" marks the cursor, or "«" and "»" the anchor and head. */
function state(marked: string): EditorState {
  const doc = marked.replace(/[|«»]/g, "");
  // A marker's offset once the other markers are gone.
  const at = (mark: string) => marked.replace(/[|«»]/g, (m) => (m === mark ? m : "")).indexOf(mark);
  const selection = marked.includes("|")
    ? EditorSelection.cursor(at("|"))
    : EditorSelection.range(at("«"), at("»"));
  return EditorState.create({ doc, selection });
}

/** The state's text with its main selection marked like the input of `state`. */
function marked(s: EditorState): string {
  const { anchor, head, empty } = s.selection.main;
  const marks: [number, string][] = empty ? [[head, "|"]] : [[anchor, "«"], [head, "»"]];
  let text = s.sliceDoc();
  for (const [pos, mark] of marks.sort((a, b) => b[0] - a[0])) {
    text = text.slice(0, pos) + mark + text.slice(pos);
  }
  return text;
}

const run = (input: string, command: (s: EditorState) => TransactionSpec) => {
  const s = state(input);
  return marked(s.update(command(s)).state);
};

describe("wrapSelection", () => {
  const bold = (s: EditorState) => wrapSelection(s, "**", "**");

  it("wraps the selection and keeps it on the same text", () => {
    expect(run("a «word» b", bold)).toBe("a **«word»** b");
    expect(run("a »word« b", bold)).toBe("a **»word«** b");
  });

  it("inserts the pair around an empty cursor", () => {
    expect(run("a | b", bold)).toBe("a **|** b");
    expect(run("|", (s) => wrapSelection(s, "_", "_"))).toBe("_|_");
  });

  it("unwraps what is already wrapped", () => {
    expect(run("a **«word»** b", bold)).toBe("a «word» b");
    expect(run("a **|** b", bold)).toBe("a | b");
    expect(run("***«x»***", bold)).toBe("*«x»*");
  });

  it("wraps every selection range", () => {
    const s = EditorState.create({
      doc: "one two",
      selection: EditorSelection.create([EditorSelection.range(0, 3), EditorSelection.range(4, 7)]),
      extensions: EditorState.allowMultipleSelections.of(true),
    });
    expect(s.update(wrapSelection(s, "_", "_")).state.sliceDoc()).toBe("_one_ _two_");
  });
});

describe("toggleLinePrefix", () => {
  const prefix = (p: string) => (s: EditorState) => toggleLinePrefix(s, p);

  it("adds a prefix and moves a cursor at the line start past it", () => {
    expect(run("|Title", prefix("# "))).toBe("# |Title");
    expect(run("Ti|tle", prefix("# "))).toBe("# Ti|tle");
    expect(run("|", prefix("- [ ] "))).toBe("- [ ] |");
  });

  it("removes the prefix when the line has it", () => {
    expect(run("# Ti|tle", prefix("# "))).toBe("Ti|tle");
    expect(run("- [ ] |milk", prefix("- [ ] "))).toBe("|milk");
    expect(run("- [x] milk|", prefix("- [ ] "))).toBe("milk|");
    expect(run("* item|", prefix("- "))).toBe("item|");
  });

  it("replaces a different heading or list prefix", () => {
    expect(run("## Sub|", prefix("# "))).toBe("# Sub|");
    expect(run("- milk|", prefix("- [ ] "))).toBe("- [ ] milk|");
    expect(run("1. milk|", prefix("- "))).toBe("- milk|");
    expect(run("- [ ] milk|", prefix("- "))).toBe("- milk|");
  });

  it("keeps indentation and quote markers in front of the prefix", () => {
    expect(run("  - nested|", prefix("- [ ] "))).toBe("  - [ ] nested|");
    expect(run("> quoted|", prefix("- "))).toBe("> - quoted|");
    expect(run("> > - [ ] deep|", prefix("- [ ] "))).toBe("> > deep|");
  });

  it("toggles all selected lines together and skips blank ones", () => {
    expect(run("«a\n\nb»", prefix("- "))).toBe("«- a\n\n- b»");
    expect(run("«- a\n\n- b»", prefix("- "))).toBe("«a\n\nb»");
    // Mixed lines: the ones missing the prefix get it, the rest stay as they are.
    expect(run("«- a\nb»", prefix("- "))).toBe("«- a\n- b»");
  });

  it("ignores a line the selection only touches at its start", () => {
    expect(run("«a\n»b", prefix("# "))).toBe("«# a\n»b");
  });

  it("never touches text after the prefix", () => {
    expect(run("#hashtag|", prefix("# "))).toBe("# #hashtag|");
    expect(run("-not a list|", prefix("- "))).toBe("- -not a list|");
  });
});

describe("insertImageLinks", () => {
  const insert = (input: string, paths: string[]) =>
    run(input, (s) => insertImageLinks(s, s.selection.main.head, paths));

  it("puts each link on its own line", () => {
    expect(insert("|", ["assets/a-1.jpg"])).toBe("![](assets/a-1.jpg)|");
    expect(insert("text|", ["a.jpg", "b.jpg"])).toBe("text\n![](a.jpg)\n![](b.jpg)|");
    expect(insert("|text", ["a.jpg"])).toBe("![](a.jpg)|\ntext");
    expect(insert("te|xt", ["a.jpg"])).toBe("te\n![](a.jpg)|\nxt");
  });
});
