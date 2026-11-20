import fc from "fast-check";
import { describe, expect, it } from "vitest";
import { cleanMerge, diff3, splitLines, unifiedHunks, withConflictMarkers } from "./mockmerge";
import type { MergeChunk } from "./types";

/** The merged text with every clash resolved by `pick`. */
function join(chunks: MergeChunk[], pick: "mine" | "theirs" = "mine"): string {
  return chunks.map((c) => (c.kind === "clash" ? c[pick] : c.text)).join("");
}

describe("splitLines", () => {
  it("keeps every line break and a missing last one", () => {
    expect(splitLines("")).toEqual([]);
    expect(splitLines("a")).toEqual(["a"]);
    expect(splitLines("a\r\nb\n\nc")).toEqual(["a\r\n", "b\n", "\n", "c"]);
  });
});

describe("diff3", () => {
  const base = "# Run\n\nFive miles.\n\n- stretch\n";

  it("merges changes to different paragraphs", () => {
    const chunks = diff3(base, "# Run\n\nSix miles.\n\n- stretch\n", "# Run\n\nFive miles.\n\n- stretch\n- shoes\n");
    expect(chunks.map((c) => c.kind)).toEqual(["same", "mine", "same", "theirs"]);
    expect(cleanMerge(chunks)).toBe("# Run\n\nSix miles.\n\n- stretch\n- shoes\n");
    expect(chunks[1]).toEqual({ kind: "mine", text: "Six miles.\n", base: "Five miles.\n" });
  });

  it("makes a clash of the same line, with each side's text", () => {
    const chunks = diff3(base, "# Run\n\nSix miles.\n\n- stretch\n", "# Run\n\nTen miles.\n\n- stretch\n");
    expect(chunks).toEqual([
      { kind: "same", text: "# Run\n\n" },
      { kind: "clash", base: "Five miles.\n", mine: "Six miles.\n", theirs: "Ten miles.\n" },
      { kind: "same", text: "\n- stretch\n" },
    ]);
    expect(cleanMerge(chunks)).toBeNull();
  });

  it("makes a clash of changes to adjacent lines, as git does", () => {
    const chunks = diff3("a\nb\nc\n", "A\nb\nc\n", "a\nB\nc\n");
    expect(chunks).toEqual([
      { kind: "clash", base: "a\nb\n", mine: "A\nb\n", theirs: "a\nB\n" },
      { kind: "same", text: "c\n" },
    ]);
    // Insertions at one place clash too.
    const appended = diff3("a\n", "a\nmine\n", "a\ntheirs\n");
    expect(appended).toEqual([
      { kind: "same", text: "a\n" },
      { kind: "clash", base: "", mine: "mine\n", theirs: "theirs\n" },
    ]);
  });

  it("takes the same change on both sides once", () => {
    const chunks = diff3("a\nb\nc\n", "a\nB\nc\n", "a\nB\nc\n");
    expect(chunks).toEqual([
      { kind: "same", text: "a\n" },
      { kind: "both", text: "B\n", base: "b\n" },
      { kind: "same", text: "c\n" },
    ]);
  });

  it("shows deletions as empty text with the deleted base", () => {
    const chunks = diff3("a\nb\nc\nd\ne\n", "a\nc\nd\ne\n", "a\nb\nc\nd\n");
    expect(chunks).toEqual([
      { kind: "same", text: "a\n" },
      { kind: "mine", text: "", base: "b\n" },
      { kind: "same", text: "c\nd\n" },
      { kind: "theirs", text: "", base: "e\n" },
    ]);
  });

  it("is byte-exact with CRLF and no final newline", () => {
    const chunks = diff3("a\r\nb\r\nc", "A\r\nb\r\nc", "a\r\nb\r\nc!");
    expect(cleanMerge(chunks)).toBe("A\r\nb\r\nc!");
    // A changed line ending is a change.
    expect(cleanMerge(diff3("a\nb\n", "a\r\nb\n", "a\nb\n"))).toBe("a\r\nb\n");
  });

  it("treats a file added on both sides as added to an empty base", () => {
    expect(diff3("", "mine\n", "theirs\n")).toEqual([{ kind: "clash", base: "", mine: "mine\n", theirs: "theirs\n" }]);
  });

  const lines = fc.array(fc.constantFrom("a\n", "b\n", "c\n", "\n", "d\r\n", "e"), { maxLength: 12 }).map((l) => l.join(""));

  it("reproduces either side when the other didn't change (property)", () => {
    fc.assert(
      fc.property(lines, lines, (base, side) => {
        expect(cleanMerge(diff3(base, side, base))).toBe(side);
        expect(cleanMerge(diff3(base, base, side))).toBe(side);
        expect(cleanMerge(diff3(base, side, side))).toBe(side);
      }),
    );
  });

  it("joins to mine or theirs when every clash takes that side and the other side made no clean change (property)", () => {
    fc.assert(
      fc.property(lines, lines, lines, (base, mine, theirs) => {
        const chunks = diff3(base, mine, theirs);
        // Chunks are whole runs of base: their bases joined give the base back.
        expect(chunks.map((c) => (c.kind === "same" ? c.text : c.base)).join("")).toBe(base);
        if (!chunks.some((c) => c.kind === "theirs")) expect(join(chunks, "mine")).toBe(mine);
        if (!chunks.some((c) => c.kind === "mine")) expect(join(chunks, "theirs")).toBe(theirs);
      }),
    );
  });
});

describe("withConflictMarkers", () => {
  it("writes what git merge writes", () => {
    const chunks = diff3("a\nb\nc\n", "a\nmine\nc\n", "a\ntheirs\nc\n");
    expect(withConflictMarkers(chunks)).toBe("a\n<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> origin/main\nc\n");
    // A side without a final newline still ends its block.
    expect(withConflictMarkers(diff3("x", "mine", "theirs"))).toBe(
      "<<<<<<< HEAD\nmine\n=======\ntheirs\n>>>>>>> origin/main\n",
    );
  });
});

describe("unifiedHunks", () => {
  const numbered = (n: number, from = 1) => Array.from({ length: n }, (_, i) => `line ${i + from}\n`).join("");

  it("shows an added file as one hunk from nothing", () => {
    expect(unifiedHunks("", "a\nb")).toEqual([
      {
        oldStart: 0,
        oldLines: 0,
        newStart: 1,
        newLines: 2,
        lines: [
          { kind: "added", text: "a" },
          { kind: "added", text: "b" },
        ],
      },
    ]);
    expect(unifiedHunks("a\n", "")[0]).toMatchObject({ oldStart: 1, oldLines: 1, newStart: 0, newLines: 0 });
  });

  it("keeps three lines of context and splits distant changes", () => {
    const old = numbered(20);
    const changed = old.replace("line 2\n", "line two\n").replace("line 18\n", "line eighteen\n");
    const hunks = unifiedHunks(old, changed);
    expect(hunks).toHaveLength(2);
    expect(hunks[0]).toMatchObject({ oldStart: 1, oldLines: 5, newStart: 1, newLines: 5 });
    expect(hunks[0].lines.map((l) => l.kind)).toEqual(["context", "removed", "added", "context", "context", "context"]);
    expect(hunks[1]).toMatchObject({ oldStart: 15, oldLines: 6, newStart: 15, newLines: 6 });
    // Six unchanged lines between two changes still make one hunk.
    const close = old.replace("line 2\n", "x\n").replace("line 9\n", "y\n");
    expect(unifiedHunks(old, close)).toHaveLength(1);
    expect(unifiedHunks(old, old.replace("line 2\n", "x\n").replace("line 10\n", "y\n"))).toHaveLength(2);
  });

  it("shows a changed line ending as removed and added, without the breaks", () => {
    expect(unifiedHunks("a\nb\n", "a\r\nb\n")[0].lines).toEqual([
      { kind: "removed", text: "a" },
      { kind: "added", text: "a" },
      { kind: "context", text: "b" },
    ]);
  });

  it("has no hunks for identical text", () => {
    expect(unifiedHunks("same\n", "same\n")).toEqual([]);
  });
});
