import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createMockApi } from "./mock";
import { SEARCH_DEBOUNCE_MS, Search } from "./search.svelte";
import type { SearchHit } from "./types";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

it("filters by name without asking the backend", async () => {
  const api = createMockApi();
  const search = new Search(api);
  search.setQuery("frost");
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS * 2);
  expect(api.mock.calls).toEqual([]);
  expect(search.hits).toEqual([]);
});

it("searches text as you type, sending only the last query", async () => {
  const api = createMockApi();
  const search = new Search(api);
  search.setMode("text");
  for (const q of ["f", "fr", "frost"]) search.setQuery(q);
  expect(search.pending).toBe(true);
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  expect(api.mock.calls).toEqual(["search"]);
  expect(search.pending).toBe(false);
  expect(search.hits.map((h) => h.file)).toEqual(["2026-11-16-1402-morning-run.md"]);
});

it("drops an answer that arrives after a newer query", async () => {
  const api = createMockApi();
  let answerOld: (hits: SearchHit[]) => void = () => {};
  const slow = new Promise<SearchHit[]>((resolve) => (answerOld = resolve));
  const search = new Search({ ...api, search: (q) => (q === "old" ? slow : api.search(q)) });
  search.setMode("text");
  search.setQuery("old");
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  search.setQuery("frost");
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  answerOld([{ file: "stale.md", title: "stale", line: 1, snippet: "old", matchStart: 0, matchEnd: 3 }]);
  await vi.advanceTimersByTimeAsync(0);
  expect(search.hits.map((h) => h.file)).toEqual(["2026-11-16-1402-morning-run.md"]);
});

it("shows backend errors and recovers on clear", async () => {
  const api = createMockApi();
  const search = new Search(api);
  api.mock.failNext("search", "io", "Couldn't read the journal.");
  search.setMode("text");
  search.setQuery("x");
  await vi.advanceTimersByTimeAsync(SEARCH_DEBOUNCE_MS);
  expect(search.error).toBe("Couldn't read the journal.");

  search.clear();
  expect(search).toMatchObject({ query: "", mode: "name", error: null, pending: false, hits: [] });
});
