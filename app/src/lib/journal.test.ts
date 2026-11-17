import { describe, expect, it } from "vitest";
import {
  dayOfMonth,
  entryTimestamp,
  filterByName,
  groupByMonth,
  groupHits,
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

describe("wordCount", () => {
  it("counts words and ignores markdown punctuation", () => {
    expect(wordCount("# Hello world\n\n- [ ] one **two** _three_\n")).toBe(5);
    expect(wordCount("")).toBe(0);
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
