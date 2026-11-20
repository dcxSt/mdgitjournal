import { afterEach, describe, expect, it, vi } from "vitest";
import { AppState, MERGE_POLL_MS, MERGE_TEXT, NEEDS_PULL_TEXT, STALE_TEXT, mobileLayout } from "./app.svelte";
import { AUTOSAVE_MS } from "./entry.svelte";
import { createMockApi, type MockApi } from "./mock";
import { commitButton, pullButton } from "./sync";
import type { Overlap } from "./types";

/** GitHub's text of an overlap, put back together from its suggestion's chunks. */
function theirsOf(overlap: Overlap): string {
  return overlap
    .suggestion!.chunks.map((c) => (c.kind === "same" ? c.text : c.kind === "clash" ? c.theirs : c.kind === "mine" ? c.base : c.text))
    .join("");
}

const RUN = "2026-11-16-1402-morning-run.md";
const IDEAS = "2026-11-14-2130-ideas.md";
const LIST = "2026-10-28-0815-reading-list.md";
const NOTES = "notes.md";
const DAWN = "2026-11-16-0630-dawn.md";

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
    expect(await app.commit("Morning", [RUN])).toBeNull();
    const [write, commit] = order(api, "writeEntry", "commit");
    expect(write).toBeGreaterThanOrEqual(0);
    expect(write).toBeLessThan(commit);
    expect(app.sync.changes).toEqual([]);
  });

  it("before commit_and_push", async () => {
    const { api, app } = await start({ connected: true });
    await app.pull();
    app.current.edit("unsaved");
    expect(await app.commit("Morning", [RUN])).toBeNull();
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
    const error = await app.commit("  \n ", [RUN]);
    expect(error?.kind).toBe("empty_message");
    expect(api.mock.calls).not.toContain("commit");
  });

  it("commits only the ticked files; the rest stay changed", async () => {
    const { api, app } = await start();
    api.mock.editOnDisk(IDEAS, "changed too\n");
    app.current.edit("unsaved");
    await app.openCommit();
    expect(app.sync.changes.map((c) => c.path)).toEqual([IDEAS, RUN]);
    const commit = vi.spyOn(api, "commit");
    expect(await app.commit("Run only", [RUN])).toBeNull();
    expect(commit).toHaveBeenCalledWith("Run only", [RUN]);
    expect(app.sync.changes).toEqual([{ path: IDEAS, kind: "modified" }]);
  });

  it("won't commit with nothing ticked", async () => {
    const { api, app } = await start();
    app.current.edit("unsaved");
    const error = await app.commit("Message", []);
    expect(error?.kind).toBe("invalid");
    expect(api.mock.calls).not.toContain("commit");
    expect(app.lastError).toBeNull();
  });

  it("pushes local commits without a message when nothing is uncommitted", async () => {
    const { api, app } = await start({ connected: true });
    app.current.edit("x");
    await app.commit("first", [RUN]); // needs_pull: the commit stays local
    await app.pull();
    expect(app.sync).toMatchObject({ ahead: 1, behind: 0, changes: [] });
    const push = vi.spyOn(api, "commitAndPush");
    expect(await app.commit("", [])).toBeNull();
    expect(push).toHaveBeenCalledWith("", []);
    expect(app.sync.ahead).toBe(0);
  });

  it("asks to pull first when GitHub has new commits, keeping the commit", async () => {
    const { app } = await start({ connected: true });
    app.current.edit("x");
    expect(await app.commit("Morning", [RUN])).toBeNull();
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

/** Lets GitHub and this device both change files, so the next pull opens the review. */
async function overlapping(api: MockApi, app: AppState) {
  const seeded = api.mock.seedOverlaps();
  await app.refresh();
  await vi.waitFor(() => expect(app.sync.changes).toHaveLength(2));
  return seeded;
}

describe("pull", () => {
  it("applies one-sided changes at once and says what came, with Undo", async () => {
    const { api, app } = await start({ connected: true });
    await app.open(IDEAS);
    await app.pull();
    expect(app.current.content).toBe(api.mock.files.get(IDEAS));
    expect(app.current.content).toContain("Edited on the phone.");
    expect(app.entries.map((e) => e.file)).toContain("2026-11-16-0630-dawn.md");
    const notice = app.notices.list.find((n) => n.key === "sync-summary")!;
    expect(notice.text).toBe("Pulled 2 commits from GitHub · 2 files changed.");
    expect(notice.actions?.map((a) => a.label)).toEqual(["Details", "Undo"]);
    expect(app.summary?.incoming.map((c) => c.message)).toEqual(["Ideas from the phone", "Dawn"]);
    expect(app.canUndo).toBe(true);
  });

  it("says Up to date when there is nothing new", async () => {
    const { app } = await start({ connected: true });
    await app.pull();
    await app.pull();
    expect(app.notices.list.at(-1)?.text).toBe("Up to date.");
  });

  it("Undo puts back every file the pull touched and the commits it brought", async () => {
    const { api, app } = await start({ connected: true });
    await app.open(IDEAS);
    const before = new Map(api.mock.files);
    await app.pull();
    const undo = app.notices.list.find((n) => n.key === "sync-summary")!.actions!.find((a) => a.label === "Undo")!;
    undo.run();
    await vi.waitFor(() => expect(app.notices.list.at(-1)?.text).toBe("Pull undone."));
    expect(new Map(api.mock.files)).toEqual(before);
    expect(app.current.content).toBe(before.get(IDEAS));
    expect(app.sync).toMatchObject({ behind: 2, lastSync: null });
    expect(app.canUndo).toBe(false);
    expect(app.notices.list.map((n) => n.key)).not.toContain("sync-summary");
  });

  it("Undo refuses, naming the file, once a pulled file was edited; nothing changes", async () => {
    const { api, app } = await start({ connected: true });
    await app.pull();
    await app.open(IDEAS);
    app.current.edit("written after the pull\n");
    await app.current.flush();
    const files = new Map(api.mock.files);
    const error = await app.undoSync();
    expect(error?.kind).toBe("undo_refused");
    expect(error?.message).toContain(IDEAS);
    expect(new Map(api.mock.files)).toEqual(files);
    expect(app.lastError).toBeNull();
  });

  it("the summary goes when the pull can't be undone any more", async () => {
    const { app } = await start({ connected: true });
    await app.pull();
    app.openSummary();
    expect(app.overlay).toBe("summary");
    app.current.edit("x");
    await app.commit("After the pull", [RUN]);
    expect(app.summary).toBeNull();
    expect(app.overlay).toBeNull();
    expect(app.notices.list.map((n) => n.key)).not.toContain("sync-summary");
  });

  it("places unpushed commits after GitHub's, and says so", async () => {
    const { api, app } = await start({ connected: true });
    api.mock.editOnDisk(NOTES, "Notes from here.\n");
    await app.refresh();
    await app.commit("Notes", [NOTES]);
    await app.pull();
    expect(app.summary).toMatchObject({ plan: "rebase", rebased: 1 });
    expect(app.notices.list.find((n) => n.key === "sync-summary")?.text).toBe(
      "Pulled 2 commits from GitHub · 2 files changed · your 1 unpushed commit was placed after them.",
    );
    expect(app.sync).toMatchObject({ ahead: 1, behind: 0 });
  });

  it("the editor is read-only while the pull runs, and writable after", async () => {
    const { api, app } = await start({ connected: true });
    let during: boolean | undefined;
    duringNextPull(api, () => (during = app.readOnly));
    await app.pull();
    expect(during).toBe(true);
    expect(app.readOnly).toBe(false);
  });

  it("closes the open entry when the pull deleted it", async () => {
    const { api, app } = await start({ connected: true });
    api.mock.remoteCommit(RUN, null, "Delete the run");
    await app.pull();
    expect(app.current.file).toBeNull();
    expect(api.mock.files.has(RUN)).toBe(false);
  });

  it("follows the open entry when the pull renamed it", async () => {
    const { api, app } = await start({ connected: true });
    const renamed = "2026-11-16-1402-long-run.md";
    api.mock.remoteCommit(RUN, null, "Rename");
    api.mock.remoteCommit(renamed, api.mock.files.get(RUN)!, "Rename");
    await app.pull();
    expect(app.current.file).toBe(renamed);
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

describe("sync review", () => {
  it("opens when a file changed on both sides, and nothing changes on disk", async () => {
    const { api, app } = await start({ connected: true });
    const { clean, clash } = await overlapping(api, app);
    const before = new Map(api.mock.files);
    await app.pull();
    expect(app.overlay).toBe("review");
    expect(new Map(api.mock.files)).toEqual(before);
    const review = app.review!;
    expect(review.preview.overlaps.map((o) => o.path)).toEqual([clash, clean]);
    // The clean merge is prefilled; the clash waits, so Apply does too.
    expect(review.picks[clean]?.choice).toBe("merged");
    expect(review.picks[clash]).toBeNull();
    expect(review.ready).toBe(false);
    expect(app.readOnly).toBe(false);
  });

  it("Cancel leaves everything as it was, without a call to the backend", async () => {
    const { api, app } = await start({ connected: true });
    await overlapping(api, app);
    await app.pull();
    const before = new Map(api.mock.files);
    const calls = api.mock.calls.length;
    app.cancelReview();
    expect(app.overlay).toBeNull();
    expect(app.review).toBeNull();
    expect(api.mock.calls.length).toBe(calls);
    expect(new Map(api.mock.files)).toEqual(before);
  });

  for (const choice of ["mine", "theirs", "both"] as const) {
    it(`Apply with “${choice}” writes that, merges the clean file, and shows the summary`, async () => {
      const { api, app } = await start({ connected: true });
      const { clean, clash } = await overlapping(api, app);
      const mine = api.mock.files.get(clash)!;
      await app.pull();
      const review = app.review!;
      const overlap = review.overlap(clash)!;
      const merged = (review.picks[clean] as { text: string }).text;
      review.setPick(clash, { choice });
      await app.applyReview();
      expect(app.overlay).toBeNull();
      expect(api.mock.files.get(clash)).toBe(choice === "theirs" ? theirsOf(overlap) : mine);
      if (choice === "both") expect(api.mock.files.get(overlap.copy!)).toBe(theirsOf(overlap));
      expect(api.mock.files.get(clean)).toBe(merged);
      expect(merged).toContain("Six miles");
      expect(merged).toContain("- [x] new shoes");
      expect(app.summary?.resolved.map((r) => r.choice)).toEqual([choice, "merged"]);
      // The resolved files stay as changes for the next commit.
      expect(app.sync.changes.map((c) => c.path)).toContain(clean);
      expect(app.canUndo).toBe(true);
    });
  }

  it("Done in the merge view applies its text byte for byte", async () => {
    const { api, app } = await start({ connected: true });
    const { clash } = await overlapping(api, app);
    await app.pull();
    app.openMerge(clash);
    expect(app.overlay).toBe("merge");
    app.closeMerge({ text: "My own merge\r\nno final newline", snapshot: { json: null } });
    expect(app.overlay).toBe("review");
    await app.applyReview();
    expect(api.mock.files.get(clash)).toBe("My own merge\r\nno final newline");
  });

  it("Cancel in the merge view leaves the file's choice alone", async () => {
    const { api, app } = await start({ connected: true });
    const { clash } = await overlapping(api, app);
    await app.pull();
    app.openMerge(clash);
    app.closeMerge();
    expect(app.overlay).toBe("review");
    expect(app.review!.picks[clash]).toBeNull();
  });

  it("a stale review writes nothing and is replaced by a fresh one, keeping choices that still hold", async () => {
    const { api, app } = await start({ connected: true });
    const { clean, clash } = await overlapping(api, app);
    await app.pull();
    const first = app.review!;
    app.openMerge(clash);
    app.closeMerge({ text: "My careful hand-merged paragraph\n", snapshot: { json: { doc: 1 } } });
    // An edit to a file the review isn't about: the choices still hold.
    api.mock.editOnDisk(NOTES, "Edited in vim meanwhile.\n");
    const before = new Map(api.mock.files);
    await app.applyReview();
    expect(new Map(api.mock.files)).toEqual(before);
    expect(api.mock.calls).toContain("pullPreview");
    expect(app.overlay).toBe("review");
    expect(app.review).not.toBe(first);
    expect(app.review!.notice).toBe(`${STALE_TEXT} Your choices are kept.`);
    expect(app.review!.picks[clash]).toEqual({ choice: "merged", text: "My careful hand-merged paragraph\n" });
    expect(app.review!.snapshot(clash)).toEqual({ json: { doc: 1 } });
    expect(app.lastError).toBeNull();

    // An edit to the clashing file itself: only its choice starts afresh, and the banner says so.
    const second = app.review!;
    api.mock.editOnDisk(clash, "typed in vim\n");
    await app.applyReview();
    expect(app.review).not.toBe(second);
    expect(app.review!.notice).toMatch(/^Something changed since you opened the review\. “reading-list” is new or changed: check it\. Your other choices are kept\.$/);
    expect(app.review!.picks[clash]).toBeNull();
    expect(app.review!.picks[clean]).not.toBeNull();

    app.review!.setPick(clash, { choice: "theirs" });
    await app.applyReview();
    expect(app.overlay).toBeNull();
    expect(api.mock.files.get(NOTES)).toBe("Edited in vim meanwhile.\n");
  });

  it("doesn't fetch while the review or the merge view is open", async () => {
    const { api, app } = await start({ connected: true });
    const { clash } = await overlapping(api, app);
    await app.pull();
    api.mock.calls.length = 0;
    await app.fetch();
    app.openMerge(clash);
    await app.fetch();
    await app.onFocus();
    expect(api.mock.calls).not.toContain("fetch");
    app.closeMerge();
    app.cancelReview();
    await app.fetch();
    expect(api.mock.calls).toContain("fetch");
  });

  it("opens from connect when the journal and GitHub both have a file, differently", async () => {
    const api = createMockApi({ platform: "macos" });
    api.mock.remoteCommit(IDEAS, "# Ideas\n\nWritten on GitHub.\n", "Ideas on GitHub");
    const { app } = await start({ api });
    api.mock.editOnDisk(IDEAS, "# Ideas\n\nWritten here.\n");
    await api.githubSetPat("token");
    const result = await app.connect("octocat/journal");
    expect(result.status).toBe("review");
    expect(app.sync.connected).toBe(true);
    expect(app.overlay).toBe("review");
    expect(app.review!.notice).toMatch(/^Connected to octocat\/journal\./);
    expect(api.mock.files.get(IDEAS)).toBe("# Ideas\n\nWritten here.\n");
  });
});

describe("resolve with git", () => {
  async function merging() {
    const started = await start({ connected: true });
    const { api, app } = started;
    await overlapping(api, app);
    const before = new Map(api.mock.files);
    const changes = app.sync.changes;
    await app.pull();
    app.review!.resolveWithGit = true;
    app.review!.gitMessage = "My edits";
    await app.resolveWithGit();
    return { ...started, before, changes };
  }

  it("needs a commit message when there are uncommitted changes", async () => {
    const { api, app } = await start({ connected: true });
    await overlapping(api, app);
    await app.pull();
    await app.resolveWithGit();
    expect(app.review?.error).toMatch(/commit message/);
    expect(api.mock.calls).not.toContain("startExternalMerge");
  });

  it("leaves a merge in progress: markers in the files, a notice, Pull and Commit off", async () => {
    const { api, app } = await merging();
    expect(app.overlay).toBeNull();
    expect(app.sync.merge?.conflicted).toEqual([LIST]);
    expect(api.mock.files.get(LIST)).toMatch(/^<{7} HEAD/m);
    const notice = app.notices.list.find((n) => n.key === "merge")!;
    expect(notice).toMatchObject({ persistent: true, text: expect.stringContaining(MERGE_TEXT) });
    expect(notice.files).toEqual([{ label: LIST, file: LIST }]);
    expect(pullButton(app.sync, app.ops).disabled).toBe(true);
    expect(commitButton(app.sync, app.ops).disabled).toBe(true);
    await app.pull();
    expect(api.mock.calls.filter((c) => c === "pull")).toHaveLength(1);
  });

  it("Abort merge puts back the files and the uncommitted edits, and says so", async () => {
    const { api, app, before, changes } = await merging();
    notice(app, "merge").actions![0].run();
    expect(app.overlay).toBe("abortMerge");
    expect(await app.abortMerge()).toBeNull();
    expect(new Map(api.mock.files)).toEqual(before);
    expect(app.sync.changes).toEqual(changes);
    expect(app.sync.merge).toBeNull();
    expect(app.overlay).toBeNull();
    expect(app.notices.list.map((n) => n.key)).not.toContain("merge");
    expect(app.notices.list.map((n) => n.text)).not.toContain("Merge finished.");
    expect(app.notices.list.at(-1)?.text).toBe("Merge aborted.");
  });

  it("Abort merge says why when it can't, changing nothing", async () => {
    const { api, app } = await merging();
    api.mock.editOnDisk(DAWN, "Up before the sun.\nTyped during the merge.\n");
    const during = new Map(api.mock.files);
    const error = await app.abortMerge();
    expect(error?.kind).toBe("invalid");
    expect(error?.message).toContain(DAWN);
    expect(new Map(api.mock.files)).toEqual(during);
    expect(app.sync.merge).not.toBeNull();
  });

  it("tells a merge aborted in a terminal from one finished there", async () => {
    vi.useFakeTimers();
    const { api, app } = await merging();
    api.mock.abortMergeWithGit();
    await vi.advanceTimersByTimeAsync(MERGE_POLL_MS);
    expect(app.sync.merge).toBeNull();
    expect(app.notices.list.at(-1)?.text).toBe("Merge aborted with git. Your edits stay committed, as an unpushed commit.");
  });

  it("notices when the merge is finished in a terminal", async () => {
    vi.useFakeTimers();
    const { api, app } = await merging();
    api.mock.files.set(LIST, "# Reading list\n\nResolved in vim.\n");
    api.mock.finishMergeWithGit();
    await vi.advanceTimersByTimeAsync(MERGE_POLL_MS);
    expect(app.sync.merge).toBeNull();
    expect(app.notices.list.map((n) => n.key)).not.toContain("merge");
    expect(app.notices.list.at(-1)?.text).toBe("Merge finished.");
    expect(pullButton(app.sync, app.ops).disabled).toBe(false);
  });
});

function notice(app: AppState, key: string) {
  return app.notices.list.find((n) => n.key === key)!;
}

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
    notice.actions![0].run();
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
