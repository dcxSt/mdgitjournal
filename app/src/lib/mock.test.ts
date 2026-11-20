// The mock backend's sync model, against the rules of docs/architecture.md, so the UI tests and
// Playwright drive the same flows the real backend has.

import { describe, expect, it } from "vitest";
import { ApiError } from "./api";
import { createMockApi, type MockApi } from "./mock";
import type { PullPreview, PullResult, Resolved } from "./types";

const RUN = "2026-11-16-1402-morning-run.md";
const IDEAS = "2026-11-14-2130-ideas.md";
const LIST = "2026-10-28-0815-reading-list.md";
const PHONE = "2026-11-15-0700-from-my-phone.md";
const DAWN = "2026-11-16-0630-dawn.md";

const at1530 = () => new Date(2026, 10, 16, 15, 30);

async function connected(options: Parameters<typeof createMockApi>[0] = {}): Promise<MockApi> {
  const api = createMockApi({ now: at1530, ...options });
  await api.githubSetPat("token");
  await api.githubConnect("octocat/journal");
  return api;
}

function snapshot(api: MockApi): [string, string][] {
  return [...api.mock.files.entries()].sort(([a], [b]) => a.localeCompare(b));
}

async function rejects(promise: Promise<unknown>, kind: string): Promise<ApiError> {
  const err = await promise.then(
    () => null,
    (e: unknown) => e,
  );
  expect(err).toBeInstanceOf(ApiError);
  expect((err as ApiError).kind).toBe(kind);
  return err as ApiError;
}

function reviewOf(result: PullResult): PullPreview {
  expect(result.status).toBe("review");
  if (result.status !== "review") throw new Error("not a review");
  return result.preview;
}

describe("connecting", () => {
  it("applies the one-sided history at once, with a summary Undo can reverse", async () => {
    const api = createMockApi({ now: at1530 });
    await api.githubSetPat("token");
    const result = await api.githubConnect("octocat/journal");
    expect(result.status).toBe("applied");
    if (result.status !== "applied") return;
    expect(result.summary).toMatchObject({ plan: "fast_forward", incomingCount: 1, rebased: 0, mergeCommit: null });
    expect(result.summary.incoming[0]).toMatchObject({ message: "From my phone", author: "octocat" });
    expect(result.summary.files).toEqual([{ path: PHONE, kind: "added" }]);
    expect(result.view).toMatchObject({ connected: true, ahead: 0, behind: 0, merge: null });
    expect(result.view.lastSync).toEqual(result.summary);
    expect(api.mock.files.get(PHONE)).toBe("Written on the phone.\n");
  });

  it("opens a review when a file differs on both sides", async () => {
    const api = createMockApi({ now: at1530 });
    api.mock.remoteCommit(RUN, "GitHub's run\n");
    api.mock.editOnDisk(RUN, "my run\n");
    await api.githubSetPat("token");
    const preview = reviewOf(await api.githubConnect("octocat/journal"));
    expect(preview.overlaps.map((o) => [o.path, o.kind])).toEqual([[RUN, "both_modified"]]);
    expect(preview.auto).toEqual([{ path: PHONE, kind: "added" }]);
    expect((await api.syncState()).connected).toBe(true);
    expect(api.mock.files.get(RUN)).toBe("my run\n");
  });

  it("pushes this journal's history to an empty repository", async () => {
    const api = createMockApi({ samples: false, now: at1530 });
    api.mock.editOnDisk("a.md", "a\n");
    await api.commit("First", ["a.md"]);
    await api.githubSetPat("token");
    const result = await api.githubConnect("octocat/journal");
    expect(result.status).toBe("up_to_date");
    expect(result.view).toMatchObject({ ahead: 0, behind: 0 });
  });

  it("queues sample activity on GitHub, seen after a fetch", async () => {
    const api = await connected();
    expect((await api.syncState()).behind).toBe(0);
    expect((await api.fetch()).behind).toBe(2);
  });
});

describe("one-sided pull", () => {
  it("applies at once and Undo puts back exactly what was there", async () => {
    const api = await connected();
    api.mock.editOnDisk(RUN, "an unrelated draft\n");
    const before = snapshot(api);
    const entriesChanged: number[] = [];
    await api.onEntriesChanged(() => entriesChanged.push(1));
    const result = await api.pull();
    expect(result.status).toBe("applied");
    if (result.status !== "applied") return;
    expect(result.summary.incomingCount).toBe(2);
    expect(result.summary.incoming.map((c) => c.message)).toEqual(["Ideas from the phone", "Dawn"]);
    expect(result.summary.files).toEqual([
      { path: IDEAS, kind: "modified" },
      { path: DAWN, kind: "added" },
    ].sort((a, b) => a.path.localeCompare(b.path)));
    expect(api.mock.files.get(RUN)).toBe("an unrelated draft\n");
    expect(entriesChanged).toHaveLength(1);

    const undone = await api.undoSync();
    expect(undone.restored).toEqual([IDEAS, DAWN].sort());
    expect(snapshot(api)).toEqual(before);
    expect(undone.view).toMatchObject({ behind: 2, lastSync: null });
    await rejects(api.undoSync(), "invalid");
  });

  it("refuses Undo, naming the file, once a pulled file was edited", async () => {
    const api = await connected();
    await api.pull();
    api.mock.editOnDisk(DAWN, "edited after the pull\n");
    const err = await rejects(api.undoSync(), "undo_refused");
    expect(err.message).toContain(DAWN);
    expect(api.mock.files.get(DAWN)).toBe("edited after the pull\n");
  });

  it("ends Undo at the next commit", async () => {
    const api = await connected();
    await api.pull();
    api.mock.editOnDisk(RUN, "x\n");
    await api.commit("Run", [RUN]);
    expect((await api.syncState()).lastSync).toBeNull();
    await rejects(api.undoSync(), "invalid");
  });

  it("places unpushed commits after GitHub's", async () => {
    const api = await connected();
    api.mock.editOnDisk(RUN, "committed here\n");
    await api.commit("Run here", [RUN]);
    const result = await api.pull();
    expect(result.status).toBe("applied");
    if (result.status !== "applied") return;
    expect(result.summary).toMatchObject({ plan: "rebase", rebased: 1 });
    expect(result.view).toMatchObject({ ahead: 1, behind: 0 });
    expect(api.mock.files.get(RUN)).toBe("committed here\n");
    expect(api.mock.files.get(DAWN)).toBe("Up before the sun.\n");
    await api.undoSync();
    expect((await api.syncState()).ahead).toBe(1);
  });
});

describe("Sync review", () => {
  it("writes nothing until Apply, and offers each choice", async () => {
    const api = await connected();
    const { clean, clash } = api.mock.seedOverlaps();
    const before = snapshot(api);
    const preview = reviewOf(await api.pull());
    expect(snapshot(api)).toEqual(before);
    expect(preview).toMatchObject({ plan: "fast_forward", automatic: false, incomingCount: 4 });
    expect(preview.auto.map((c) => c.path)).toEqual([IDEAS, DAWN].sort());
    const [c1, c2] = [clash, clean].sort();
    expect(preview.overlaps.map((o) => o.path)).toEqual([c1, c2]);
    const byPath = new Map(preview.overlaps.map((o) => [o.path, o]));
    expect(byPath.get(clean)?.suggestion?.clean).toBe(true);
    expect(byPath.get(clash)?.suggestion?.clean).toBe(false);
    expect(byPath.get(clean)).toMatchObject({
      kind: "both_modified",
      binary: false,
      choices: ["mine", "theirs", "both", "merged"],
      copy: "2026-11-16-1402-morning-run.from-github-1530.md",
    });
    // The same state gives the same review.
    expect(reviewOf(await api.pullPreview()).id).toBe(preview.id);

    const merged = byPath.get(clean)!.suggestion!.chunks.map((c) => (c.kind === "clash" ? "" : c.text)).join("");
    expect(merged).toContain("Six miles");
    expect(merged).toContain("- [x] new shoes");
    const { summary, view } = await api.applyPull(
      preview.id,
      [
        { path: clean, choice: "merged", text: merged },
        { path: clash, choice: "both" },
      ],
      null,
    );
    expect(api.mock.files.get(clean)).toBe(merged);
    const copy = byPath.get(clash)!.copy!;
    expect(copy).toBe("2026-10-28-0815-reading-list.from-github-1530.md");
    expect(api.mock.files.get(clash)).toContain("(again)");
    expect(api.mock.files.get(copy)).toContain("H. D. Thoreau");
    expect(summary.resolved).toEqual([
      { path: clash, choice: "both", copy },
      { path: clean, choice: "merged", copy: null },
    ]);
    expect(summary.files.map((f) => f.path)).toContain(copy);
    // Left as uncommitted changes on top of GitHub's commits.
    expect(view.behind).toBe(0);
    expect(view.changes.map((c) => c.path)).toEqual([clash, copy, clean].sort());
  });

  it("keeps mine or takes GitHub's", async () => {
    const api = await connected();
    const { clean, clash } = api.mock.seedOverlaps();
    const mine = api.mock.files.get(clash);
    const preview = reviewOf(await api.pull());
    const theirs = preview.overlaps.find((o) => o.path === clean)!.theirs;
    await api.applyPull(
      preview.id,
      [
        { path: clash, choice: "mine" },
        { path: clean, choice: "theirs" },
      ],
      null,
    );
    expect(api.mock.files.get(clash)).toBe(mine);
    expect(api.mock.files.get(clean)).toBe(theirs);
    expect((await api.syncState()).changes.map((c) => c.path)).toEqual([clash]);
  });

  it("refuses resolutions that don't match the review", async () => {
    const api = await connected();
    const { clean, clash } = api.mock.seedOverlaps();
    const preview = reviewOf(await api.pull());
    const before = snapshot(api);
    const bad: Resolved[][] = [
      [{ path: clean, choice: "mine" }],
      [
        { path: clean, choice: "mine" },
        { path: clash, choice: "mine" },
        { path: RUN === clean ? IDEAS : RUN, choice: "mine" },
      ],
      [
        { path: clean, choice: "mine" },
        { path: clean, choice: "theirs" },
        { path: clash, choice: "mine" },
      ],
    ];
    for (const resolutions of bad) await rejects(api.applyPull(preview.id, resolutions, null), "invalid");
    expect(snapshot(api)).toEqual(before);
  });

  it("is stale after an edit on disk, and the fresh one applies", async () => {
    const api = await connected();
    const { clean, clash } = api.mock.seedOverlaps();
    const preview = reviewOf(await api.pull());
    api.mock.editOnDisk(clash, "typed in vim\n");
    const resolutions: Resolved[] = [
      { path: clean, choice: "mine" },
      { path: clash, choice: "mine" },
    ];
    await rejects(api.applyPull(preview.id, resolutions, null), "stale_review");
    expect(api.mock.files.get(clash)).toBe("typed in vim\n");
    const fresh = reviewOf(await api.pullPreview());
    expect(fresh.id).not.toBe(preview.id);
    await api.applyPull(fresh.id, resolutions, null);
    expect(api.mock.files.get(clash)).toBe("typed in vim\n");
    expect(api.mock.files.get(DAWN)).toBe("Up before the sun.\n");
  });

  it("is stale after GitHub moved and a fetch saw it", async () => {
    const api = await connected();
    api.mock.seedOverlaps();
    const preview = reviewOf(await api.pull());
    api.mock.remoteCommit("2026-11-16-2200-late.md", "late\n");
    await api.fetch();
    await rejects(api.applyPull(preview.id, [], null), "stale_review");
  });

  it("makes a merge commit when committed work clashes, with the message required", async () => {
    const api = await connected();
    api.mock.editOnDisk(IDEAS, "# Ideas\n\nCommitted here.\n");
    await api.commit("Ideas here", [IDEAS]);
    const preview = reviewOf(await api.pull());
    expect(preview).toMatchObject({ plan: "merge", localCount: 1, incomingCount: 2 });
    const resolutions: Resolved[] = [{ path: IDEAS, choice: "mine" }];
    await rejects(api.applyPull(preview.id, resolutions, "  "), "empty_message");
    const { summary, view } = await api.applyPull(preview.id, resolutions, "Merge from GitHub\n\n- ideas: kept mine");
    expect(summary.plan).toBe("merge");
    expect(summary.mergeCommit).toMatch(/^[0-9a-f]{40}$/);
    expect(view).toMatchObject({ ahead: 2, behind: 0, changes: [] });
    expect(api.mock.files.get(DAWN)).toBe("Up before the sun.\n");
  });

  it("shows binary and delete/modify overlaps with their own choices", async () => {
    const api = await connected();
    api.mock.remoteCommit("assets/x.jpg", "blob:github");
    api.mock.editOnDisk("assets/x.jpg", "blob:mine");
    api.mock.remoteCommit(LIST, null);
    api.mock.editOnDisk(LIST, "edited here\n");
    api.mock.editOnDisk(RUN, null);
    api.mock.remoteCommit(RUN, "edited on GitHub\n");
    const preview = reviewOf(await api.pull());
    const byPath = new Map(preview.overlaps.map((o) => [o.path, o]));
    expect(byPath.get("assets/x.jpg")).toMatchObject({
      kind: "both_added",
      binary: true,
      mine: null,
      theirs: null,
      choices: ["mine", "theirs", "both"],
      copy: "assets/x.from-github-1530.jpg",
      suggestion: null,
    });
    expect(byPath.get(LIST)).toMatchObject({ kind: "modified_deleted", choices: ["mine", "theirs"], copy: null });
    expect(byPath.get(RUN)).toMatchObject({ kind: "deleted_modified", choices: ["mine", "theirs"] });
    await api.applyPull(
      preview.id,
      [
        { path: "assets/x.jpg", choice: "both" },
        { path: LIST, choice: "theirs" },
        { path: RUN, choice: "theirs" },
      ],
      null,
    );
    expect(api.mock.files.get("assets/x.jpg")).toBe("blob:mine");
    expect(api.mock.files.get("assets/x.from-github-1530.jpg")).toBe("blob:github");
    expect(api.mock.files.has(LIST)).toBe(false);
    expect(api.mock.files.get(RUN)).toBe("edited on GitHub\n");
  });
});

describe("Resolve with git", () => {
  async function inReview() {
    const api = await connected();
    const { clash } = api.mock.seedOverlaps();
    const preview = reviewOf(await api.pull());
    return { api, clash, preview };
  }

  it("commits uncommitted edits first, leaves markers, and Abort brings it all back", async () => {
    const { api, clash, preview } = await inReview();
    const before = snapshot(api);
    const changesBefore = (await api.syncState()).changes;
    await rejects(api.startExternalMerge(preview.id, null), "empty_message");
    expect(snapshot(api)).toEqual(before);

    const view = await api.startExternalMerge(preview.id, "My edits");
    expect(view.merge).toEqual({ conflicted: [clash], startedByApp: true });
    const marked = api.mock.files.get(clash)!;
    expect(marked).toContain("<<<<<<< HEAD\n- *Walden* (again)\n=======\n- *Walden*, H. D. Thoreau\n>>>>>>> origin/main\n");
    // The clean file was merged; one-sided files arrived.
    expect(api.mock.files.get(DAWN)).toBe("Up before the sun.\n");

    await rejects(api.pull(), "merge_in_progress");
    await rejects(api.pullPreview(), "merge_in_progress");
    await rejects(api.commit("x", [clash]), "merge_in_progress");
    await rejects(api.commitAndPush("x", [clash]), "merge_in_progress");
    await rejects(api.undoSync(), "merge_in_progress");
    expect((await api.fetch()).merge).not.toBeNull();
    expect((await api.diff(clash)).hunks.length).toBeGreaterThan(0);

    const aborted = await api.abortMerge();
    expect(aborted.restored).toContain(clash);
    expect(aborted.view.merge).toBeNull();
    expect(snapshot(api)).toEqual(before);
    expect(aborted.view.changes).toEqual(changesBefore);
    await rejects(api.abortMerge(), "invalid");
  });

  it("is finished in a terminal; the app then pushes the merge", async () => {
    const { api, clash, preview } = await inReview();
    await api.startExternalMerge(preview.id, "My edits");
    api.mock.editOnDisk(clash, "resolved in vim\n");
    api.mock.finishMergeWithGit();
    const view = await api.syncState();
    expect(view).toMatchObject({ merge: null, changes: [], behind: 0 });
    expect(view.ahead).toBe(2);
    expect((await api.commitAndPush("", [])).result).toBe("pushed");
  });
});

describe("commit with ticked paths", () => {
  it("commits only the ticked changes; a ticked rename drops the old name", async () => {
    const api = createMockApi({ now: at1530 });
    api.mock.editOnDisk(RUN, "edited\n");
    await api.renameEntry(IDEAS, "thoughts");
    const renamed = "2026-11-14-2130-thoughts.md";
    expect((await api.syncState()).changes).toEqual([
      { path: renamed, kind: "renamed", oldPath: IDEAS },
      { path: RUN, kind: "modified" },
    ]);
    await rejects(api.commit(" ", [RUN]), "empty_message");
    const view = await api.commit("Rename", [renamed, "not-changed.md"]);
    expect(view.changes).toEqual([{ path: RUN, kind: "modified" }]);
    // Nothing ticked that changed: no commit.
    const again = await api.commit("Nothing", []);
    expect(again.changes).toEqual([{ path: RUN, kind: "modified" }]);
  });

  it("pushes the ticked changes; a blank message with nothing ticked only pushes", async () => {
    const api = await connected();
    await api.pull();
    api.mock.editOnDisk(RUN, "a\n");
    api.mock.editOnDisk(LIST, "b\n");
    await rejects(api.commitAndPush("", [RUN]), "empty_message");
    expect((await api.commitAndPush("Run", [RUN])).result).toBe("pushed");
    expect((await api.syncState()).changes.map((c) => c.path)).toEqual([LIST]);
    expect((await api.commitAndPush("", [])).result).toBe("nothing");
    api.mock.remoteCommit(DAWN, "changed on GitHub\n");
    const out = await api.commitAndPush("List", [LIST]);
    expect(out.result).toBe("needs_pull");
    expect(out.view).toMatchObject({ ahead: 1, behind: 1, changes: [] });
  });

  it("diffs a changed file against HEAD", async () => {
    const api = createMockApi({ now: at1530 });
    api.mock.editOnDisk(RUN, (api.mock.files.get(RUN) ?? "").replace("Five miles", "Six miles"));
    const diff = await api.diff(RUN);
    expect(diff).toMatchObject({ path: RUN, oldPath: null, kind: "modified", binary: false });
    expect(diff.hunks[0].lines.filter((l) => l.kind !== "context")).toEqual([
      { kind: "removed", text: "Five miles along the river. Cold and bright, first frost on the path." },
      { kind: "added", text: "Six miles along the river. Cold and bright, first frost on the path." },
    ]);
    api.mock.editOnDisk("assets/run.jpg", "blob:x");
    expect(await api.diff("assets/run.jpg")).toMatchObject({ kind: "added", binary: true, hunks: [] });
    await rejects(api.diff(IDEAS), "invalid");
  });
});
