import { afterEach, describe, expect, it, vi } from "vitest";
import { AppState, NEEDS_PULL_TEXT, mobileLayout } from "./app.svelte";
import { createMockApi, type MockApi } from "./mock";

const RUN = "2026-11-16-1402-morning-run.md";
const IDEAS = "2026-11-14-2130-ideas.md";

const apps: AppState[] = [];

/** A started app; `connected` signs in with a token and connects, which queues two remote commits. */
async function start({ connected = false, api = createMockApi({ platform: "macos" }) } = {}) {
  const app = new AppState(api);
  apps.push(app);
  app.width = 1200;
  await app.init();
  if (connected) {
    await api.githubSetPat("token");
    await app.connect("octocat/journal");
  }
  api.mock.calls.length = 0;
  return { api, app };
}

/** Positions of `names` in the recorded calls, in the order given. */
const order = (api: MockApi, ...names: string[]) => names.map((n) => api.mock.calls.indexOf(n));

afterEach(() => {
  for (const app of apps.splice(0)) app.dispose();
  vi.useRealTimers();
});

describe("launch", () => {
  it("opens the newest entry on the desktop", async () => {
    const { app } = await start();
    expect(app.ready).toBe(true);
    expect(app.current.file).toBe(RUN);
    expect(app.mode).toBe("source");
  });

  it("fetches when connected", async () => {
    const { api } = await start({ connected: true });
    const relaunched = new AppState(api);
    apps.push(relaunched);
    await relaunched.init();
    expect(api.mock.calls.at(-1)).toBe("fetch");
    expect(relaunched.sync.behind).toBe(2);
  });

  it("does not fetch a local journal", async () => {
    const api = createMockApi({ platform: "macos" });
    await start({ api });
    expect(api.mock.calls).not.toContain("fetch");
  });
});

describe("layout", () => {
  it("uses the phone layout on iOS and in narrow browser windows only", () => {
    expect(mobileLayout("ios", 1024, true)).toBe(true);
    expect(mobileLayout("macos", 640, true)).toBe(false);
    expect(mobileLayout("linux", 699, false)).toBe(true);
    expect(mobileLayout("linux", 700, false)).toBe(false);
  });

  it("starts the phone in its own default mode with no entry open", async () => {
    const { app } = await start({ api: createMockApi({ platform: "ios" }) });
    expect(app.mobile).toBe(true);
    expect(app.mode).toBe("styled");
    expect(app.current.file).toBeNull();
  });
});

describe("autosave is flushed before git work", () => {
  it("before pull", async () => {
    const { api, app } = await start({ connected: true });
    app.current.edit("unsaved");
    await app.pull();
    const [write, pull] = order(api, "writeEntry", "pull");
    expect(write).toBeGreaterThanOrEqual(0);
    expect(write).toBeLessThan(pull);
  });

  it("before commit", async () => {
    const { api, app } = await start();
    app.current.edit("unsaved");
    expect(await app.commit("Morning")).toBeNull();
    const [write, commit] = order(api, "writeEntry", "commit");
    expect(write).toBeGreaterThanOrEqual(0);
    expect(write).toBeLessThan(commit);
    expect(app.sync.changes).toEqual([]);
  });

  it("before commit_and_push", async () => {
    const { api, app } = await start({ connected: true });
    await app.pull();
    app.current.edit("unsaved");
    expect(await app.commit("Morning")).toBeNull();
    const [write, push] = order(api, "writeEntry", "commitAndPush");
    expect(write).toBeGreaterThanOrEqual(0);
    expect(write).toBeLessThan(push);
  });

  it("before opening the commit sheet, so it lists the edit", async () => {
    const { app } = await start();
    app.current.edit("unsaved");
    await app.openCommit();
    expect(app.overlay).toBe("commit");
    expect(app.sync.changes).toEqual([{ path: RUN, kind: "modified" }]);
  });

  it("and git work waits when the save fails", async () => {
    const { api, app } = await start({ connected: true });
    api.mock.failNext("writeEntry", "io", "Disk full.");
    app.current.edit("unsaved");
    await app.pull();
    expect(api.mock.calls).not.toContain("pull");
    expect(app.current.content).toBe("unsaved");
    expect(app.notices.list.at(-1)?.text).toContain("Disk full.");
  });
});

describe("commit", () => {
  it("requires a message", async () => {
    const { api, app } = await start();
    app.current.edit("unsaved");
    const error = await app.commit("  \n ");
    expect(error?.kind).toBe("empty_message");
    expect(api.mock.calls).not.toContain("commit");
  });

  it("pushes local commits without a message when nothing is uncommitted", async () => {
    const { api, app } = await start({ connected: true });
    app.current.edit("x");
    await app.commit("first"); // needs_pull: the commit stays local
    await app.pull();
    expect(app.sync).toMatchObject({ ahead: 1, behind: 0, changes: [] });
    expect(await app.commit("")).toBeNull();
    expect(api.mock.calls.at(-1)).toBe("commitAndPush");
    expect(app.sync.ahead).toBe(0);
  });

  it("asks to pull first when GitHub has new commits, keeping the commit", async () => {
    const { app } = await start({ connected: true });
    app.current.edit("x");
    expect(await app.commit("Morning")).toBeNull();
    expect(app.sync).toMatchObject({ ahead: 1, behind: 2, changes: [] });
    expect(app.notices.list.map((n) => n.text)).toContain(NEEDS_PULL_TEXT);

    await app.pull();
    expect(app.notices.list.map((n) => n.text)).not.toContain(NEEDS_PULL_TEXT);
  });

  it("won't open the sheet while GitHub is ahead", async () => {
    const { app } = await start({ connected: true });
    await app.fetch();
    app.current.edit("x");
    await app.openCommit();
    expect(app.overlay).toBeNull();
    expect(app.notices.list.at(-1)?.text).toMatch(/Pull first/);
  });
});

describe("pull", () => {
  it("reloads the open entry when the pull changed it", async () => {
    const { api, app } = await start({ connected: true });
    await app.open(IDEAS);
    await app.pull();
    expect(app.current.content).toBe(api.mock.files.get(IDEAS));
    expect(app.current.content).toContain("Edited on the phone.");
    expect(app.entries.map((e) => e.file)).toContain("2026-11-16-0630-dawn.md");
  });

  it("lists conflict copies in a banner; the local text is in the copy", async () => {
    const { api, app } = await start({ connected: true });
    await app.open(IDEAS);
    app.current.edit("my version");
    await app.pull();
    const banner = app.notices.list.find((n) => n.key === "conflicts");
    const copy = banner?.files?.[0].file ?? "";
    expect(copy).toMatch(/^2026-11-14-2130-ideas\.conflict-\d{4}\.md$/);
    expect(api.mock.files.get(copy)).toBe("my version");
    expect(app.current.content).toContain("Edited on the phone.");
  });

  it("marks the error for the red dot and clears it on success", async () => {
    const { api, app } = await start({ connected: true });
    api.mock.failNext("pull", "network", "Offline.");
    await app.pull();
    expect(app.lastError?.message).toBe("Offline.");
    await app.pull();
    expect(app.lastError).toBeNull();
  });
});

describe("timing", () => {
  it("fetches every fetchIntervalMin minutes, and never when it is 0", async () => {
    vi.useFakeTimers();
    const { api, app } = await start({ connected: true });
    const fetches = () => api.mock.calls.filter((c) => c === "fetch").length;
    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(fetches()).toBe(1);
    await vi.advanceTimersByTimeAsync(5 * 60_000);
    expect(fetches()).toBe(2);

    await app.saveSettings({ fetchIntervalMin: 0 });
    await vi.advanceTimersByTimeAsync(60 * 60_000);
    expect(fetches()).toBe(2);
  });

  it("on focus, re-lists entries, fetches and reloads a clean entry changed on disk", async () => {
    const { api, app } = await start({ connected: true });
    api.mock.files.set(RUN, "changed by git pull in a terminal");
    await app.onFocus();
    expect(api.mock.calls).toEqual(expect.arrayContaining(["listEntries", "readEntry", "fetch"]));
    expect(app.current.content).toBe("changed by git pull in a terminal");
  });

  it("on focus, never replaces unsaved typing", async () => {
    const { api, app } = await start();
    app.current.edit("still typing");
    api.mock.files.set(RUN, "changed elsewhere");
    await app.onFocus();
    expect(app.current.content).toBe("still typing");
  });
});

describe("entries", () => {
  it("creates an entry from the title prompt; an empty title is untitled", async () => {
    const { app } = await start();
    app.perform("new");
    expect(app.newPrompt).toBe(true);
    expect(await app.create("   ")).toBe(true);
    expect(app.current.file).toMatch(/^\d{4}-\d{2}-\d{2}-\d{4}-untitled\.md$/);
    expect(app.newPrompt).toBe(false);
    expect(app.focusEditor).toBe(true);
  });

  it("renames only the title part and keeps the buffer", async () => {
    const { api, app } = await start();
    app.current.edit("unsaved");
    expect(await app.rename("Long Run")).toBe(true);
    expect(app.current.file).toBe("2026-11-16-1402-long-run.md");
    expect(api.mock.files.get("2026-11-16-1402-long-run.md")).toBe("unsaved");
    expect(app.currentMeta?.title).toBe("long-run");
  });

  it("goes back to the list on the phone only once the entry is saved", async () => {
    const { api, app } = await start({ api: createMockApi({ platform: "ios" }) });
    await app.openEntry(RUN);
    expect(app.focusEditor).toBe(false);
    app.current.edit("unsaved");
    api.mock.failNext("writeEntry", "io", "Disk full.");
    await app.back();
    expect(app.current.file).toBe(RUN);
    await app.back();
    expect(app.current.file).toBeNull();
    expect(api.mock.files.get(RUN)).toBe("unsaved");
  });
});

describe("perform", () => {
  it("toggles the mode, the sidebar and overlays", async () => {
    const { app } = await start();
    app.perform("toggleMode");
    expect(app.mode).toBe("styled");
    app.perform("toggleSidebar");
    expect(app.sidebarOpen).toBe(false);
    app.perform("quickOpen");
    expect(app.overlay).toBe("quickOpen");
    app.perform("quickOpen");
    expect(app.overlay).toBeNull();
  });

  it("switches the sidebar to full-text search", async () => {
    const { app } = await start();
    app.perform("search");
    expect(app.search.mode).toBe("text");
    expect(app.focusSearch).toBe(true);
  });

  it("does nothing else while a sheet is open", async () => {
    const { app } = await start();
    app.overlay = "commit";
    app.perform("toggleMode");
    app.perform("new");
    app.perform("settings");
    expect(app).toMatchObject({ mode: "source", newPrompt: false, overlay: "commit" });
  });
});
