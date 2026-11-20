import { describe, expect, it } from "vitest";
import {
  choiceLabel,
  defaultPick,
  initialPicks,
  mergeMessage,
  overlapName,
  overlapNote,
  resolutions,
  Review,
  reviewReady,
} from "./review.svelte";
import type { Overlap, PullPreview } from "./types";

const CLEAN: Overlap = {
  path: "2026-11-16-1402-morning-run.md",
  kind: "both_modified",
  binary: false,
  minePath: "2026-11-16-1402-morning-run.md",
  theirsPath: "2026-11-16-1402-morning-run.md",
  committed: false,
  versions: "1:2:3",
  choices: ["mine", "theirs", "both", "merged"],
  copy: "2026-11-16-1402-morning-run.from-github-15h30.md",
  suggestion: {
    clean: true,
    chunks: [
      { kind: "mine", text: "A\n", base: "a\n" },
      { kind: "same", text: "\n" },
      { kind: "theirs", text: "B\n", base: "b\n" },
    ],
  },
};

const CLASH: Overlap = {
  ...CLEAN,
  path: "2026-10-28-0815-reading-list.md",
  minePath: "2026-10-28-0815-reading-list.md",
  theirsPath: "2026-10-28-0815-reading-list.md",
  versions: "4:5:6",
  copy: "2026-10-28-0815-reading-list.from-github-15h30.md",
  suggestion: { clean: false, chunks: [{ kind: "clash", base: "a\n", mine: "x\n", theirs: "y\n" }] },
};

const PHOTO: Overlap = {
  path: "assets/run-5f3a09c2.jpg",
  kind: "both_added",
  binary: true,
  minePath: "assets/run-5f3a09c2.jpg",
  theirsPath: "assets/run-5f3a09c2.jpg",
  committed: false,
  versions: "0:7:8",
  choices: ["mine", "theirs", "both"],
  copy: "assets/run-5f3a09c2.from-github-15h30.jpg",
  suggestion: null,
};

const GONE: Overlap = {
  path: "notes.md",
  kind: "modified_deleted",
  binary: false,
  minePath: "notes.md",
  theirsPath: null,
  committed: false,
  versions: "9:10:0",
  choices: ["mine", "theirs"],
  copy: null,
  suggestion: null,
};

function preview(overlaps: Overlap[], plan: PullPreview["plan"] = "fast_forward"): PullPreview {
  return {
    id: "main:1:2:3",
    branch: "main",
    plan,
    automatic: false,
    incoming: [{ id: "2", message: "From the phone\n\nDetails.", author: "octocat", time: 0 }],
    incomingCount: 1,
    local: [],
    localCount: plan === "fast_forward" ? 0 : 1,
    auto: [],
    overlaps,
    gitMerge: { conflicted: [], uncommitted: [] },
  };
}

describe("defaults", () => {
  it("a clean merge starts out merged, prefilled; a clash, a binary file and a deletion wait", () => {
    expect(defaultPick(CLEAN)).toEqual({ choice: "merged", text: "A\n\nB\n" });
    expect(defaultPick(CLASH)).toBeNull();
    expect(defaultPick(PHOTO)).toBeNull();
    expect(defaultPick(GONE)).toBeNull();
    expect(initialPicks(preview([CLEAN, CLASH]))).toEqual({
      [CLEAN.path]: { choice: "merged", text: "A\n\nB\n" },
      [CLASH.path]: null,
    });
  });
});

describe("Apply", () => {
  it("is enabled once every file has a choice", () => {
    const p = preview([CLEAN, CLASH]);
    const picks = initialPicks(p);
    expect(reviewReady(p, picks, "")).toBe(false);
    expect(reviewReady(p, { ...picks, [CLASH.path]: { choice: "theirs" } }, "")).toBe(true);
  });

  it("with only clean merges, is enabled at once (one tap to accept)", () => {
    const p = preview([CLEAN]);
    expect(reviewReady(p, initialPicks(p), "")).toBe(true);
  });

  it("for a merge commit, also needs a message", () => {
    const p = preview([CLEAN], "merge");
    expect(reviewReady(p, initialPicks(p), "  \n")).toBe(false);
    expect(reviewReady(p, initialPicks(p), "Merge from GitHub")).toBe(true);
    // A merge with nothing changed on both sides only asks for the message.
    expect(reviewReady(preview([], "merge"), {}, "Merge")).toBe(true);
  });

  it("sends exactly one resolution per file, in order, with the merged text", () => {
    const p = preview([CLEAN, CLASH, PHOTO, GONE]);
    const picks = {
      ...initialPicks(p),
      [CLASH.path]: { choice: "both" as const },
      [PHOTO.path]: { choice: "mine" as const },
      [GONE.path]: { choice: "theirs" as const },
    };
    expect(resolutions(p, picks)).toEqual([
      { path: CLEAN.path, choice: "merged", text: "A\n\nB\n" },
      { path: CLASH.path, choice: "both" },
      { path: PHOTO.path, choice: "mine" },
      { path: GONE.path, choice: "theirs" },
    ]);
    expect(() => resolutions(p, initialPicks(p))).toThrow();
  });
});

describe("words", () => {
  it("labels choices by what they do to the file", () => {
    expect(CLEAN.choices.map((c) => choiceLabel(CLEAN, c))).toEqual(["Keep mine", "Keep GitHub's", "Keep both", "Merge…"]);
    expect(GONE.choices.map((c) => choiceLabel(GONE, c))).toEqual(["Keep mine", "Delete"]);
    const deletedHere = { ...GONE, kind: "deleted_modified" as const };
    expect(deletedHere.choices.map((c) => choiceLabel(deletedHere, c))).toEqual(["Keep deleted", "Keep GitHub's"]);
  });

  it("says how each file changed", () => {
    expect(overlapNote(CLEAN)).toBe("Changed here and on GitHub");
    expect(overlapNote(PHOTO)).toBe("New image here and on GitHub");
    expect(overlapNote(GONE)).toBe("Changed here, deleted on GitHub");
    expect(overlapNote({ ...PHOTO, kind: "both_modified" })).toBe("Image changed here and on GitHub");
  });

  it("prefills the merge commit's message with each choice it records", () => {
    const recorded = (o: Overlap) => ({ ...o, committed: true });
    const p = preview([CLEAN, CLASH, PHOTO, GONE].map(recorded), "merge");
    const picks = {
      ...initialPicks(p),
      [CLASH.path]: { choice: "mine" as const },
      [PHOTO.path]: { choice: "both" as const },
      [GONE.path]: { choice: "theirs" as const },
    };
    expect(mergeMessage(p, picks)).toBe(
      [
        "Merge from GitHub",
        "",
        `- ${CLEAN.path}: merged`,
        `- ${CLASH.path}: kept mine`,
        `- ${PHOTO.path}: kept both (GitHub's saved as ${PHOTO.copy})`,
        `- ${GONE.path}: kept GitHub's (deleted)`,
      ].join("\n"),
    );
    expect(mergeMessage(preview([], "merge"), {})).toBe("Merge from GitHub");
    // A resolution of uncommitted edits stays a change to commit: the merge commit doesn't say it.
    const mixed = preview([recorded(CLEAN), CLASH], "merge");
    expect(mergeMessage(mixed, { ...initialPicks(mixed), [CLASH.path]: { choice: "mine" } })).toBe(
      `Merge from GitHub\n\n- ${CLEAN.path}: merged`,
    );
  });

  it("names renames, and what each choice does to them", () => {
    const renamed: Overlap = {
      ...CLEAN,
      path: "2026-11-16-1402-long-run.md",
      kind: "modified_renamed",
      minePath: "2026-11-16-1402-run.md",
      theirsPath: "2026-11-16-1402-long-run.md",
      copy: "2026-11-16-1402-long-run.md",
    };
    expect(overlapNote(renamed)).toBe("Changed here, renamed on GitHub to “long-run”");
    expect(overlapName(renamed)).toBe("2026-11-16-1402-run.md");
    expect(overlapNote({ ...renamed, kind: "renamed_modified", theirsPath: "2026-11-16-1402-run.md" })).toBe(
      "Renamed here, changed on GitHub (where it is still “run”)",
    );
    const gone = { ...GONE, kind: "deleted_renamed" as const, minePath: null, theirsPath: renamed.path };
    expect(gone.choices.map((c) => choiceLabel(gone, c))).toEqual(["Keep deleted", "Keep GitHub's"]);
    expect(overlapName(gone)).toBe(renamed.path);
  });
});

describe("Review", () => {
  it("keeps the message in step with the choices until the user edits it", () => {
    const review = new Review(preview([CLEAN, CLASH].map((o) => ({ ...o, committed: true })), "merge"));
    expect(review.message).not.toContain(CLASH.path);
    review.setPick(CLASH.path, { choice: "theirs" });
    expect(review.message).toContain(`- ${CLASH.path}: kept GitHub's`);
    review.editMessage("My own words");
    review.setPick(CLASH.path, { choice: "mine" });
    expect(review.message).toBe("My own words");
    expect(review.commitMessage()).toBe("My own words");
  });

  it("has no commit message unless Apply makes a merge commit", () => {
    expect(new Review(preview([CLEAN])).commitMessage()).toBeNull();
  });

  it("Done in the merge view sets the merged text and keeps the editor to reopen", () => {
    const review = new Review(preview([CLASH]));
    expect(review.ready).toBe(false);
    const snapshot = { json: { doc: "y\n" } };
    review.merged(CLASH.path, "y\n", snapshot);
    expect(review.picks[CLASH.path]).toEqual({ choice: "merged", text: "y\n" });
    expect(review.snapshot(CLASH.path)).toBe(snapshot);
    expect(review.ready).toBe(true);
    expect(review.resolutions()).toEqual([{ path: CLASH.path, choice: "merged", text: "y\n" }]);
  });
});

describe("a fresh review after a stale one", () => {
  it("keeps the choices and merges of files that are the same, and resets the rest", () => {
    const stale = new Review(preview([CLEAN, CLASH, PHOTO]));
    const snapshot = { json: { doc: "hand-merged\n" } };
    stale.merged(CLASH.path, "hand-merged\n", snapshot);
    stale.setPick(PHOTO.path, { choice: "both" });
    expect(stale.dirty).toBe(true);

    const changed = { ...PHOTO, versions: "0:7:99" };
    const fresh = new Review(preview([CLEAN, CLASH, changed, GONE]), stale);
    expect(fresh.picks[CLASH.path]).toEqual({ choice: "merged", text: "hand-merged\n" });
    expect(fresh.snapshot(CLASH.path)).toBe(snapshot);
    expect(fresh.picks[CLEAN.path]).toEqual({ choice: "merged", text: "A\n\nB\n" });
    expect(fresh.picks[PHOTO.path]).toBeNull();
    expect(fresh.reset).toEqual([PHOTO.path, GONE.path]);
  });

  it("keeps a message the user wrote", () => {
    const stale = new Review(preview([CLEAN], "merge"));
    stale.editMessage("Merged on the train");
    expect(new Review(preview([CLEAN], "merge"), stale).message).toBe("Merged on the train");
  });

  it("is not dirty until the user does something", () => {
    const review = new Review(preview([CLEAN, CLASH]));
    expect(review.dirty).toBe(false);
    review.setPick(CLASH.path, { choice: "mine" });
    expect(review.dirty).toBe(true);
  });
});
