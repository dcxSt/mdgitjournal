import { describe, expect, it } from "vitest";
import {
  choiceLabel,
  defaultPick,
  initialPicks,
  mergeMessage,
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
  base: "a\n\nb\n",
  mine: "A\n\nb\n",
  theirs: "a\n\nB\n",
  choices: ["mine", "theirs", "both", "merged"],
  copy: "2026-11-16-1402-morning-run.from-github-1530.md",
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
  copy: "2026-10-28-0815-reading-list.from-github-1530.md",
  mine: "x\n",
  theirs: "y\n",
  suggestion: { clean: false, chunks: [{ kind: "clash", base: "a\n", mine: "x\n", theirs: "y\n" }] },
};

const PHOTO: Overlap = {
  path: "assets/run-5f3a09c2.jpg",
  kind: "both_added",
  binary: true,
  base: null,
  mine: null,
  theirs: null,
  choices: ["mine", "theirs", "both"],
  copy: "assets/run-5f3a09c2.from-github-1530.jpg",
  suggestion: null,
};

const GONE: Overlap = {
  path: "notes.md",
  kind: "modified_deleted",
  binary: false,
  base: "n\n",
  mine: "N\n",
  theirs: null,
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

  it("prefills the merge commit's message with each choice", () => {
    const p = preview([CLEAN, CLASH, PHOTO, GONE], "merge");
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
  });
});

describe("Review", () => {
  it("keeps the message in step with the choices until the user edits it", () => {
    const review = new Review(preview([CLEAN, CLASH], "merge"));
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
