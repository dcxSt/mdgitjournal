import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AUTOSAVE_MS, CurrentEntry, SaveError } from "./entry.svelte";
import { createMockApi } from "./mock";

const FILE = "2026-11-16-1402-morning-run.md";

async function setup() {
  const api = createMockApi();
  const entry = new CurrentEntry(api);
  entry.load(await api.readEntry(FILE));
  api.mock.calls.length = 0;
  return { api, entry };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("autosave", () => {
  it("writes once, 400 ms after the last edit", async () => {
    const { api, entry } = await setup();
    entry.edit("a");
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS - 1);
    entry.edit("ab");
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS - 1);
    expect(api.mock.calls).toEqual([]);
    expect(entry.dirty).toBe(true);

    await vi.advanceTimersByTimeAsync(1);
    expect(api.mock.calls).toEqual(["writeEntry"]);
    expect(api.mock.files.get(FILE)).toBe("ab");
    expect(entry.dirty).toBe(false);
  });

  it("does not write on load or for an unchanged buffer", async () => {
    const { api, entry } = await setup();
    entry.edit(entry.content);
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS * 2);
    expect(api.mock.calls).toEqual([]);
    expect(entry.dirty).toBe(false);
  });

  it("writes the text byte for byte, line endings included", async () => {
    const { api, entry } = await setup();
    const text = "# Title\r\n\r\nno final newline\ronly CR";
    entry.edit(text);
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS);
    expect(api.mock.files.get(FILE)).toBe(text);
  });

  it("flush writes pending edits at once", async () => {
    const { api, entry } = await setup();
    entry.edit("now");
    await entry.flush();
    expect(api.mock.files.get(FILE)).toBe("now");
    expect(entry.dirty).toBe(false);
    // The debounced write was cancelled, not merely raced.
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS);
    expect(api.mock.calls).toEqual(["writeEntry"]);
  });

  it("keeps the edits when a write fails, reports it from flush, and retries", async () => {
    const { api, entry } = await setup();
    api.mock.failNext("writeEntry", "io", "Disk full.");
    entry.edit("precious");
    await expect(entry.flush()).rejects.toBeInstanceOf(SaveError);
    expect(entry.content).toBe("precious");
    expect(entry.dirty).toBe(true);
    expect(entry.saveError).toBe("Disk full.");

    await vi.advanceTimersByTimeAsync(3000);
    expect(api.mock.files.get(FILE)).toBe("precious");
    expect(entry.dirty).toBe(false);
    expect(entry.saveError).toBeNull();
  });

  it("writes nothing while paused and catches up after resume", async () => {
    const { api, entry } = await setup();
    entry.pause();
    entry.edit("typed during a pull");
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS * 3);
    await entry.flush();
    expect(api.mock.calls).toEqual([]);

    entry.resume();
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS);
    expect(api.mock.files.get(FILE)).toBe("typed during a pull");
  });
});

describe("reloadIfClean", () => {
  it("reloads a clean buffer whose file changed on disk", async () => {
    const { api, entry } = await setup();
    const version = entry.version;
    api.mock.files.set(FILE, "from elsewhere");
    await entry.reloadIfClean();
    expect(entry.content).toBe("from elsewhere");
    expect(entry.version).toBe(version + 1);
  });

  it("leaves a dirty buffer alone", async () => {
    const { api, entry } = await setup();
    entry.edit("mine");
    api.mock.files.set(FILE, "from elsewhere");
    await entry.reloadIfClean();
    expect(entry.content).toBe("mine");
    expect(api.mock.calls).toEqual([]);
  });

  it("does not reset the editor when the file is unchanged", async () => {
    const { entry } = await setup();
    const version = entry.version;
    await entry.reloadIfClean();
    expect(entry.version).toBe(version);
  });
});
