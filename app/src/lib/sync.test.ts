import { describe, expect, it } from "vitest";
import { EMPTY_VIEW } from "./defaults";
import {
  commitButton,
  commitMessageValid,
  isPushOnly,
  isSyncFailure,
  MERGE_TITLE,
  pullButton,
  rebasedText,
  relativeTime,
  summaryText,
} from "./sync";
import type { FileChange, SyncSummary, SyncView } from "./types";

const change: FileChange = { path: "2026-11-16-1402-morning-run.md", kind: "modified" };
const view = (patch: Partial<SyncView>): SyncView => ({ ...EMPTY_VIEW, connected: true, repo: "o/r", ...patch });

describe("pull button", () => {
  it("is grey with no badge when up to date", () => {
    expect(pullButton(view({}), [])).toMatchObject({ accent: false, badge: null, busy: false });
  });

  it("is accented with the number of new commits when behind", () => {
    expect(pullButton(view({ behind: 2 }), [])).toMatchObject({
      accent: true,
      badge: 2,
      title: "2 new commits on GitHub",
    });
  });

  it("spins while pulling or fetching", () => {
    expect(pullButton(view({}), ["pull"]).busy).toBe(true);
    expect(pullButton(view({}), ["fetch"]).busy).toBe(true);
    expect(pullButton(view({}), ["commit"]).busy).toBe(false);
  });
});

describe("commit button", () => {
  it("is a disabled, grey Commit & Push with nothing to do", () => {
    expect(commitButton(view({}), [])).toMatchObject({
      label: "Commit & Push",
      accent: false,
      badge: null,
      disabled: true,
      hint: null,
    });
  });

  it("is highlighted with the number of changed files", () => {
    const changes = [change, { path: "b.md", kind: "added" } as const];
    expect(commitButton(view({ changes }), [])).toMatchObject({ accent: true, badge: 2, disabled: false });
  });

  it("is enabled for local commits waiting to be pushed", () => {
    expect(commitButton(view({ ahead: 1 }), [])).toMatchObject({ accent: true, badge: null, disabled: false });
  });

  it("is disabled with the hint “Pull first” when GitHub is ahead", () => {
    const button = commitButton(view({ behind: 1, ahead: 1, changes: [change] }), []);
    expect(button).toMatchObject({ disabled: true, accent: false, hint: "Pull first", badge: 1 });
  });

  it("is just Commit, never blocked by the remote, when not connected", () => {
    const local = { ...EMPTY_VIEW, changes: [change], behind: 3, ahead: 3 };
    expect(commitButton(local, [])).toMatchObject({ label: "Commit", disabled: false, hint: null, badge: 1 });
    expect(commitButton({ ...EMPTY_VIEW, ahead: 2 }, []).disabled).toBe(true);
  });

  it("spins while committing", () => {
    expect(commitButton(view({ changes: [change] }), ["commit"]).busy).toBe(true);
  });
});

describe("commit message", () => {
  it("is required", () => {
    expect(commitMessageValid("")).toBe(false);
    expect(commitMessageValid(" \n\t ")).toBe(false);
    expect(commitMessageValid("Run notes")).toBe(true);
  });

  it("may be left out only when there is nothing to commit but commits to push", () => {
    expect(isPushOnly(view({ ahead: 1 }))).toBe(true);
    expect(isPushOnly(view({ ahead: 1, changes: [change] }))).toBe(false);
    expect(isPushOnly(view({}))).toBe(false);
    expect(isPushOnly({ ...EMPTY_VIEW, ahead: 1 })).toBe(false);
  });
});

describe("a merge in progress", () => {
  const merging = view({ behind: 2, changes: [change], merge: { conflicted: [change.path], merged: [], startedByApp: true, precommit: false, autostash: false } });

  it("disables Pull and Commit & Push, saying why", () => {
    expect(pullButton(merging, [])).toMatchObject({ disabled: true, accent: false, title: MERGE_TITLE });
    expect(commitButton(merging, [])).toMatchObject({ disabled: true, accent: false, hint: null, title: MERGE_TITLE });
  });

  it("Pull is enabled again once it is over", () => {
    expect(pullButton({ ...merging, merge: null }, []).disabled).toBe(false);
  });

  it("Pull is disabled while a pull or another git change runs", () => {
    expect(pullButton(view({}), ["pull"]).disabled).toBe(true);
    expect(pullButton(view({}), ["apply"])).toMatchObject({ disabled: true, busy: true, title: "Updating files…" });
    expect(pullButton(view({}), ["fetch"]).disabled).toBe(false);
  });
});

describe("summary words", () => {
  const summary = (patch: Partial<SyncSummary>): SyncSummary => ({
    at: 0,
    plan: "fast_forward",
    incoming: [],
    incomingCount: 3,
    rebased: 0,
    mergeCommit: null,
    files: [change, { path: "b.md", kind: "added" }],
    resolved: [],
    ...patch,
  });

  it("counts commits and files, and says where unpushed commits went", () => {
    expect(summaryText(summary({}))).toBe("Pulled 3 commits from GitHub · 2 files changed");
    expect(summaryText(summary({ incomingCount: 1, files: [], rebased: 2, plan: "rebase" }))).toBe(
      "Pulled 1 commit from GitHub · your 2 unpushed commits were placed after them",
    );
    // What is left to do: a merge commit to push, resolutions still to commit.
    const resolved = { path: change.path, kind: "both_modified" as const, choice: "merged" as const, copy: null };
    expect(summaryText(summary({ mergeCommit: "c".repeat(40), plan: "merge", files: [] }))).toBe(
      "Pulled 3 commits from GitHub · merge commit ready to push",
    );
    expect(summaryText(summary({ files: [change], resolved: [resolved] }), [change])).toBe(
      "Pulled 3 commits from GitHub · 1 file changed · 1 resolved file ready to commit",
    );
    expect(summaryText(summary({ files: [change], resolved: [resolved] }), [])).toBe("Pulled 3 commits from GitHub · 1 file changed");
    expect(rebasedText({ rebased: 2, incomingCount: 3 })).toBe("Your 2 unpushed commits were placed after the 3 from GitHub.");
    expect(rebasedText({ rebased: 1, incomingCount: 1 })).toBe("Your 1 unpushed commit was placed after the 1 from GitHub.");
  });

  it("says how long ago", () => {
    const now = new Date(2026, 10, 19, 15, 0).getTime();
    expect(relativeTime(now - 20_000, now)).toBe("just now");
    expect(relativeTime(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(relativeTime(now - 3 * 3600_000, now)).toBe("3 h ago");
    expect(relativeTime(new Date(2026, 10, 18, 23, 0).getTime(), now)).toBe("yesterday");
  });
});

describe("errors", () => {
  it("only real sync failures light the red dot", () => {
    for (const kind of ["auth", "reauth", "network", "not_connected", "io", "git"]) expect(isSyncFailure(kind)).toBe(true);
    for (const kind of ["stale_review", "merge_in_progress", "undo_refused", "invalid", "empty_message"]) {
      expect(isSyncFailure(kind)).toBe(false);
    }
  });
});
