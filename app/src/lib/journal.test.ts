import { describe, expect, it } from "vitest";
import {
  dayOfMonth,
  entryTimestamp,
  filterByName,
  groupByMonth,
  groupHits,
  isEntryFile,
  pathLabel,
  quickOpen,
  slugPreview,
  snippetParts,
  wordCount,
} from "./journal";
import type { EntryMeta, SearchHit } from "./types";

function entry(file: string): EntryMeta {
  const m = /^(\d{4}-\d{2}-\d{2})-(\d{2})(\d{2})-(.+)\.md$/.exec(file);
  return {
    file,
    date: m?.[1] ?? null,
    time: m ? `${m[2]}:${m[3]}` : null,
    title: m?.[4] ?? file.replace(/\.md$/, ""),
    mtime: 0,
    size: 0,
    words: null,
  };
}

const ENTRIES = [
  "2026-11-16-1402-morning-run.md",
  "2026-11-05-0900-ideas.md",
  "2026-10-28-0815-reading-list.md",
  "2025-12-31-2359-new-years-eve.md",
  "notes.md",
].map(entry);

describe("groupByMonth", () => {
  it("groups by month in the given order and labels the months", () => {
    const groups = groupByMonth(ENTRIES);
    expect(groups.map((g) => g.label)).toEqual(["November 2026", "October 2026", "December 2025", "Other"]);
    expect(groups[0].entries.map((e) => e.title)).toEqual(["morning-run", "ideas"]);
  });

  it("puts entries that don't follow the naming scheme last", () => {
    const groups = groupByMonth([entry("aaa.md"), ...ENTRIES.slice(0, 1)]);
    expect(groups.map((g) => g.key)).toEqual(["2026-11", "other"]);
    expect(groups[1].entries.map((e) => e.file)).toEqual(["aaa.md"]);
  });

  it("returns nothing for an empty journal", () => {
    expect(groupByMonth([])).toEqual([]);
  });
});

describe("row text", () => {
  it("shows the day without a leading zero, nothing for Other", () => {
    expect(ENTRIES.map(dayOfMonth)).toEqual(["16", "5", "28", "31", ""]);
  });

  it("formats the header timestamp", () => {
    expect(entryTimestamp(ENTRIES[0])).toBe("2026-11-16 14:02");
    expect(entryTimestamp(ENTRIES[4])).toBeNull();
  });
});

describe("filterByName", () => {
  it("matches every word, case-insensitively, anywhere in the file name", () => {
    expect(filterByName(ENTRIES, "RUN").map((e) => e.title)).toEqual(["morning-run"]);
    expect(filterByName(ENTRIES, "2026 list").map((e) => e.title)).toEqual(["reading-list"]);
    expect(filterByName(ENTRIES, "  ")).toBe(ENTRIES);
    expect(filterByName(ENTRIES, "nothing")).toEqual([]);
  });
});

describe("quickOpen", () => {
  it("ranks fuzzy matches, best first", () => {
    expect(quickOpen(ENTRIES, "rdlst")[0].title).toBe("reading-list");
    expect(quickOpen(ENTRIES, "mr")[0].title).toBe("morning-run");
    expect(quickOpen(ENTRIES, "zzz")).toEqual([]);
  });

  it("lists everything, newest first, for an empty query", () => {
    expect(quickOpen(ENTRIES, "")).toEqual(ENTRIES);
  });
});

describe("slugPreview", () => {
  it("previews the backend's slug", () => {
    expect(slugPreview("Morning Run!")).toBe("morning-run");
    expect(slugPreview("Évening -- walk")).toBe("evening-walk");
    expect(slugPreview("  ")).toBe("untitled");
  });

  it("declines titles only the backend can transliterate", () => {
    expect(slugPreview("日記")).toBeNull();
  });
});

describe("pathLabel", () => {
  it("names entries by title and date, other files by path", () => {
    expect(pathLabel("2026-11-16-1402-morning-run.md")).toEqual({
      title: "morning-run",
      when: "2026-11-16 14:02",
      entry: true,
    });
    expect(pathLabel("2026-11-16-1402-run.from-github-1530.md").title).toBe("run.from-github-1530");
    expect(pathLabel("notes.md")).toEqual({ title: "notes.md", when: null, entry: true });
    expect(pathLabel("assets/run-5f3a09c2.jpg")).toEqual({ title: "assets/run-5f3a09c2.jpg", when: null, entry: false });
  });
});

describe("isEntryFile", () => {
  it("is true only for what the entry list can open", () => {
    for (const name of ["2026-11-16-1402-run.md", "notes.md", "x.conflict-1402.md"]) {
      expect(isEntryFile(name), name).toBe(true);
    }
    for (const name of ["assets/x-1a2b3c4d.jpg", "x.conflict-1402.jpg", "sub/x.md", ".hidden.md", ".md", "a..md"]) {
      expect(isEntryFile(name), name).toBe(false);
    }
  });
});

// The same cases as mdgj-core's word_count tests (crates/mdgj-core/src/entries.rs), so the status
// bar agrees with EntryMeta.words. CJK differs: ICU counts 日本語 as one word, mdgj-core as three.
describe("wordCount", () => {
  it("counts words and ignores markdown punctuation", () => {
    expect(wordCount("# Hello world\n\n- [ ] one **two** _three_\n")).toBe(5);
    expect(wordCount("")).toBe(0);
    expect(wordCount("--- *** ### > - [ ] [x]")).toBe(0);
    expect(wordCount("# Title\n\n- one *two* **three**\n- [ ] four")).toBe(5);
    expect(wordCount("> quoted _text_ ~~gone~~ `code`")).toBe(4);
    expect(wordCount("don't stop")).toBe(2);
    expect(wordCount("pi is 3.14, not 1,000")).toBe(5);
    expect(wordCount("line one\r\nline two\r\n")).toBe(4);
  });

  it("ignores task marks", () => {
    expect(wordCount("- [x] buy milk")).toBe(wordCount("- [ ] buy milk"));
    expect(wordCount("- [X] buy milk\n  - [x] and eggs")).toBe(4);
    expect(wordCount("[x]")).toBe(0);
    expect(wordCount("x [xy] [x x] x]")).toBe(5);
  });

  it("skips link and image destinations", () => {
    expect(wordCount("Ran 5km. ![](assets/2026-11-16-0800-run-1.jpg)")).toBe(2);
    expect(wordCount("![a sunset](x.jpg) over the bay")).toBe(5);
    expect(wordCount('see [the docs](https://example.com/a-b-c "Title") now')).toBe(4);
    expect(wordCount("[a](b)[c](d)")).toBe(2);
    expect(wordCount("[Rust](https://w.org/Rust_(lang)) end")).toBe(2);
    expect(wordCount("[x](https://x.org) [x] done")).toBe(2);
    expect(wordCount("a](b c\nd)")).toBe(4);
    expect(wordCount("a](b c")).toBe(3);
    expect(wordCount("](")).toBe(0);
    expect(wordCount("one ](two) three\r\n")).toBe(2);
  });

  it("counts letters of any script, but not emoji", () => {
    expect(wordCount("Café crème")).toBe(2);
    expect(wordCount("Привет мир")).toBe(2);
    expect(wordCount("🦀 🎉")).toBe(0);
  });
});

describe("search hits", () => {
  const hit = (file: string, line: number, snippet: string, matchStart: number, matchEnd: number): SearchHit => ({
    file,
    title: entry(file).title,
    line,
    snippet,
    matchStart,
    matchEnd,
  });

  it("splits snippets on code-point offsets", () => {
    expect(snippetParts(hit("a.md", 3, "😀 first frost", 8, 13))).toEqual({
      before: "😀 first ",
      match: "frost",
      after: "",
    });
  });

  it("cuts long context before the match to its last words, so the match stays in view", () => {
    const line = "…long the towpath to the lock, and saw a heron standing in the reeds by the far bank.";
    const at = line.indexOf("heron"); // no astral characters before it, so also the code-point offset
    expect(snippetParts(hit("a.md", 1, line, at, at + 5))).toEqual({
      before: "…and saw a ",
      match: "heron",
      after: " standing in the reeds by the far bank.",
    });
    expect(snippetParts(hit("a.md", 1, "saw a heron", 6, 11)).before).toBe("saw a ");
  });

  it("groups by file in order; a filename match adds no snippet", () => {
    const groups = groupHits([
      hit("2026-11-16-1402-morning-run.md", 0, "2026-11-16-1402-morning-run.md", 16, 19),
      hit("2026-11-16-1402-morning-run.md", 3, "first frost on the run", 19, 22),
      hit("notes.md", 1, "a run", 2, 5),
    ]);
    expect(groups.map((g) => [g.title, g.lines.map((l) => l.line)])).toEqual([
      ["morning-run", [3]],
      ["notes", [1]],
    ]);
  });
});
