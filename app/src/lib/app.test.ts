import { afterEach, describe, expect, it, vi } from "vitest";
import { AppState, NEEDS_PULL_TEXT, mobileLayout } from "./app.svelte";
import { AUTOSAVE_MS } from "./entry.svelte";
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

/** Makes the next pull run `during` while it is in progress, i.e. while autosave is paused. */
function duringNextPull(api: MockApi, during: () => void) {
  const pull = api.pull.bind(api);
  vi.spyOn(api, "pull").mockImplementationOnce(async () => {
    during();
    return pull();
  });
}

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

  it("keeps both when the open entry is typed in while the pull changes it", async () => {
    const { api, app } = await start({ connected: true });
    await app.open(IDEAS);
    const typed = `${app.current.content}typed during the pull\n`;
    duringNextPull(api, () => app.current.edit(typed));
    await app.pull();
    await app.current.settle();

    // GitHub's version keeps the name; the buffer, still open, is saved next to it.
    expect(api.mock.files.get(IDEAS)).toContain("Edited on the phone.");
    expect(api.mock.files.get(IDEAS)).not.toContain("typed during the pull");
    const copy = app.current.file!;
    expect(copy).toMatch(/^2026-11-14-2130-ideas\.conflict-\d{4}\.md$/);
    expect(api.mock.files.get(copy)).toBe(typed);
    expect(app.current.dirty).toBe(false);
    await vi.waitFor(() => expect(app.currentMeta?.file).toBe(copy));
    const notice = app.notices.list.find((n) => n.key === "kept-buffer");
    expect(notice?.text).toContain(copy);
    expect(notice?.files?.map((f) => f.file)).toEqual([IDEAS]);
  });

  it("keeps a saved edit and the typing after it when the pull conflicts", async () => {
    const { api, app } = await start({ connected: true });
    await app.open(IDEAS);
    app.current.edit("my saved edit\n");
    await app.current.flush();
    duringNextPull(api, () => app.current.edit("my saved edit\nand a line typed while pulling\n"));
    await app.pull();
    await app.current.settle();

    const conflictCopy = app.notices.list.find((n) => n.key === "conflicts")?.files?.[0].file ?? "";
    expect(api.mock.files.get(conflictCopy)).toBe("my saved edit\n");
    expect(app.current.file).not.toBe(conflictCopy);
    expect(api.mock.files.get(app.current.file!)).toBe("my saved edit\nand a line typed while pulling\n");
    expect(api.mock.files.get(IDEAS)).toContain("Edited on the phone.");
  });

  it("autosaves typing as usual when the pull leaves the open entry alone", async () => {
    const { api, app } = await start({ connected: true });
    duringNextPull(api, () => app.current.edit("typed during the pull"));
    await app.pull();
    await app.current.settle();
    expect(app.current.file).toBe(RUN);
    expect(api.mock.files.get(RUN)).toBe("typed during the pull");
    expect(app.notices.list.map((n) => n.key)).not.toContain("kept-buffer");
  });

  it("writes the typing back, and lists the entry again, when the pull removed it", async () => {
    const { api, app } = await start({ connected: true });
    vi.spyOn(api, "pull").mockImplementationOnce(async () => {
      app.current.edit("typed during the pull");
      api.mock.files.delete(RUN);
      return { view: await api.syncState(), updated: [RUN], merged: [], conflicts: [] };
    });
    await app.pull();
    await app.current.settle();
    expect(app.current.file).toBe(RUN);
    expect(api.mock.files.get(RUN)).toBe("typed during the pull");
    await vi.waitFor(() => expect(app.currentMeta?.file).toBe(RUN));
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

/** A promise the test resolves. */
function gate<T = void>() {
  let open!: (value: T) => void;
  const closed = new Promise<T>((resolve) => (open = resolve));
  return { closed, open };
}

const png = () => new File([new Uint8Array([137, 80, 78, 71])], "shot.png", { type: "image/png" });

describe("sync state", () => {
  it("pull waits for a fetch in flight, and autosave keeps working meanwhile", async () => {
    const { api, app } = await start({ connected: true });
    const fetchDone = gate();
    const fetch = api.fetch.bind(api);
    vi.spyOn(api, "fetch").mockImplementationOnce(async () => {
      await fetchDone.closed;
      return fetch();
    });
    const fetching = app.fetch();
    const pulling = app.pull();
    app.current.edit("typed while GitHub is checked");
    await vi.waitFor(() => expect(api.mock.files.get(RUN)).toBe("typed while GitHub is checked"));
    expect(api.mock.calls).not.toContain("pull");

    fetchDone.open();
    await Promise.all([fetching, pulling]);
    expect(order(api, "fetch", "pull")).toEqual([...order(api, "fetch", "pull")].sort((a, b) => a - b));
    expect(app.sync.behind).toBe(0);
  });

  it("a sync_state reply overtaken by a fetch doesn't undo what the fetch found", async () => {
    const { api, app } = await start({ connected: true });
    const stale = await api.syncState();
    expect(stale.behind).toBe(0);
    const reply = gate<typeof stale>();
    vi.spyOn(api, "syncState").mockImplementationOnce(() => reply.closed);
    app.refreshSync();
    await app.fetch();
    expect(app.sync.behind).toBe(2);

    reply.open(stale);
    await vi.waitFor(() => expect(api.syncState).toHaveBeenCalledTimes(2));
    await new Promise((resolve) => setTimeout(resolve));
    expect(app.sync.behind).toBe(2);
  });
});

describe("images", () => {
  it("an import waits for a pull in progress", async () => {
    const { api, app } = await start({ connected: true });
    const pullDone = gate();
    const pull = api.pull.bind(api);
    vi.spyOn(api, "pull").mockImplementationOnce(async () => {
      await pullDone.closed;
      return pull();
    });
    const pulling = app.pull();
    const importing = app.importImages([png()]);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(api.mock.calls).not.toContain("importImage");

    pullDone.open();
    await pulling;
    const [path] = await importing;
    expect(path).toMatch(/^assets\/2026-11-16-1402-morning-run-[0-9a-f]{8}\.jpg$/);
    expect(order(api, "pull", "importImage")).toEqual([...order(api, "pull", "importImage")].sort((a, b) => a - b));
  });

  it("names images by content, and the same image twice is one file", async () => {
    const { app } = await start();
    const [first, again] = await app.importImages([png(), png()]);
    expect(again).toBe(first);
  });

  it("stops importing once another entry is open, and says so", async () => {
    const { api, app } = await start();
    const importImage = api.importImage.bind(api);
    vi.spyOn(api, "importImage").mockImplementationOnce(async (entry, bytes) => {
      void app.open(IDEAS);
      return importImage(entry, bytes);
    });
    const paths = await app.importImages([png(), png()]);
    expect(paths).toHaveLength(1);
    expect(app.current.file).toBe(IDEAS);
    expect(api.mock.calls.filter((c) => c === "importImage")).toHaveLength(1);
    expect(app.notices.list.at(-1)?.text).toBe(
      "1 image not added: the open entry changed while importing. Add it again.",
    );
  });

  it("names the links it couldn't insert, as text to copy", async () => {
    const { app } = await start();
    app.imagesUnlinked(["assets/run-1a2b3c4d.jpg", "assets/my run-5e6f7a8b.jpg"]);
    const notice = app.notices.list.at(-1);
    expect(notice?.text).toMatch(/^Saved 2 images but didn't link them/);
    expect(notice?.files).toEqual([
      { label: "![](assets/run-1a2b3c4d.jpg)" },
      { label: "![](<assets/my run-5e6f7a8b.jpg>)" },
    ]);
  });

  it("a conflict banner links entries only; an image copy is just named", async () => {
    const { api, app } = await start({ connected: true });
    const copy = "assets/run-1.conflict-1402.jpg";
    vi.spyOn(api, "pull").mockImplementationOnce(async () => ({
      view: await api.syncState(),
      updated: [],
      merged: [],
      conflicts: [
        { path: "assets/run-1.jpg", copy },
        { path: IDEAS, copy: "2026-11-14-2130-ideas.conflict-1402.md" },
      ],
    }));
    await app.pull();
    expect(app.notices.list.find((n) => n.key === "conflicts")?.files).toEqual([
      { label: copy, file: undefined },
      { label: "2026-11-14-2130-ideas.conflict-1402.md", file: "2026-11-14-2130-ideas.conflict-1402.md" },
    ]);
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

  it("deletes only the entry the user confirmed", async () => {
    const { api, app } = await start();
    await app.remove(IDEAS);
    expect(api.mock.files.has(IDEAS)).toBe(true);
    expect(api.mock.calls).not.toContain("deleteEntry");
    await app.remove(RUN);
    expect(api.mock.files.has(RUN)).toBe(false);
  });

  it("on the phone, Back during a pull waits for it, then saves what was typed", async () => {
    const { api, app } = await start({ connected: true, api: createMockApi({ platform: "ios" }) });
    await app.openEntry(RUN);
    let back: Promise<void> | undefined;
    duringNextPull(api, () => {
      app.current.edit("typed during the pull");
      back = app.back();
    });
    await app.pull();
    await back;
    expect(api.mock.files.get(RUN)).toBe("typed during the pull");
    expect(app.current.file).toBeNull();
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

describe("closing the window", () => {
  it("saves the last edit first", async () => {
    const { api, app } = await start();
    app.current.edit("typed just before closing");
    expect(await app.beforeClose()).toBe(true);
    expect(api.mock.files.get(RUN)).toBe("typed just before closing");
  });

  it("stays open, saying why, when the save fails; Close anyway closes", async () => {
    const { api, app } = await start();
    const closeWindow = vi.spyOn(api, "closeWindow").mockResolvedValue();
    app.current.edit("unsaved");
    api.mock.failNext("writeEntry", "io", "Disk full.");
    expect(await app.beforeClose()).toBe(false);
    const notice = app.notices.list.find((n) => n.key === "close")!;
    expect(notice.text).toContain("Disk full.");
    notice.action!.run();
    expect(closeWindow).toHaveBeenCalledOnce();
  });

  it("stays open while a sync holds back autosave", async () => {
    const { app } = await start();
    app.current.edit("typed during a pull");
    app.current.pause();
    expect(await app.beforeClose()).toBe(false);
    expect(app.notices.list.find((n) => n.key === "close")?.text).toMatch(/sync in progress/);
    app.current.resume();
    expect(await app.beforeClose()).toBe(true);
  });
});

describe("a failed autosave", () => {
  it("is explained in a notice, which goes once a retry succeeds", async () => {
    vi.useFakeTimers();
    const { api, app } = await start();
    const notices = () => app.notices.list.filter((n) => n.key === "save-error");
    api.mock.failNext("writeEntry", "io", "Disk full.");
    app.current.edit("precious");
    await vi.advanceTimersByTimeAsync(AUTOSAVE_MS);
    expect(notices()).toHaveLength(1);
    expect(notices()[0]).toMatchObject({ tone: "error", text: expect.stringContaining("Disk full.") });

    // Leaving the entry reports the failure again; it replaces the notice rather than adding one.
    api.mock.failNext("writeEntry", "io", "Disk full.");
    await app.back();
    expect(app.current.file).toBe(RUN);
    expect(notices()).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(3000);
    expect(api.mock.files.get(RUN)).toBe("precious");
    expect(notices()).toHaveLength(0);
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
    for (const overlay of ["commit", "delete"] as const) {
      app.overlay = overlay;
      app.perform("toggleMode");
      app.perform("new");
      app.perform("settings");
      app.perform("quickOpen");
      expect(app).toMatchObject({ mode: "source", newPrompt: false, overlay });
    }
  });
});
