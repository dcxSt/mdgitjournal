// The Sync review (docs/architecture.md, "Sync flows in the UI"): what the user chose for each
// file changed on both sides, whether Apply may run, and what it sends. The pure functions do
// the deciding; `Review` holds one review's choices while its sheet and merge view are open.

import { joinChunks, type MergeSnapshot } from "../editor/merge";
import { commitMessageValid } from "./sync";
import type { Choice, MergeChunk, Overlap, PullPreview, Resolved } from "./types";

/** One file's choice; `text` is the merged result for `merged`. */
export type Chosen = { choice: "mine" | "theirs" | "both" } | { choice: "merged"; text: string };

export type Choices = Readonly<Record<string, Chosen | null>>;

/** A file that merges without a clash starts out merged; a clash waits for the user. */
export function defaultPick(overlap: Overlap): Chosen | null {
  const suggestion = overlap.suggestion;
  if (!suggestion?.clean || !overlap.choices.includes("merged")) return null;
  const text = joinChunks(suggestion.chunks);
  return text === null ? null : { choice: "merged", text };
}

export function initialPicks(preview: PullPreview): Choices {
  return Object.fromEntries(preview.overlaps.map((o) => [o.path, defaultPick(o)]));
}

/** Every file has a choice and, when Apply makes a merge commit, the message isn't blank. */
export function reviewReady(preview: PullPreview, picks: Choices, message: string): boolean {
  if (!preview.overlaps.every((o) => picks[o.path] != null)) return false;
  return preview.plan !== "merge" || commitMessageValid(message);
}

/** What `apply_pull` gets: one resolution per overlap, in the review's order. */
export function resolutions(preview: PullPreview, picks: Choices): Resolved[] {
  return preview.overlaps.map((o) => {
    const pick = picks[o.path];
    if (!pick) throw new Error(`No choice for ${o.path}`);
    return pick.choice === "merged" ? { path: o.path, choice: "merged", text: pick.text } : { path: o.path, choice: pick.choice };
  });
}

/** The choice's labels: deletions name what each choice does to the file. */
export function choiceLabel(overlap: Overlap, choice: Choice): string {
  if (overlap.kind === "modified_deleted") return choice === "mine" ? "Keep mine" : "Delete";
  if (overlap.kind === "deleted_modified") return choice === "mine" ? "Keep deleted" : "Keep GitHub's";
  return { mine: "Keep mine", theirs: "Keep GitHub's", both: "Keep both", merged: "Merge…" }[choice];
}

/** A line saying how this file was changed on both sides. */
export function overlapNote(overlap: Overlap): string {
  const what = /\.(jpe?g|png|gif|webp|heic|avif)$/i.test(overlap.path) ? "image" : "file";
  switch (overlap.kind) {
    case "modified_deleted":
      return "Changed here, deleted on GitHub";
    case "deleted_modified":
      return "Deleted here, changed on GitHub";
    case "both_added":
      return overlap.binary ? `New ${what} here and on GitHub` : "New here and on GitHub, with different text";
    default:
      return overlap.binary ? `${what === "image" ? "Image" : "File"} changed here and on GitHub` : "Changed here and on GitHub";
  }
}

export interface MergeCounts {
  mine: number;
  theirs: number;
  clashes: number;
}

export function mergeCounts(chunks: readonly MergeChunk[]): MergeCounts {
  const counts = { mine: 0, theirs: 0, clashes: 0 };
  for (const c of chunks) {
    if (c.kind === "mine") counts.mine++;
    else if (c.kind === "theirs") counts.theirs++;
    else if (c.kind === "clash") counts.clashes++;
  }
  return counts;
}

/** How a resolution reads in the merge commit's message and the summary. */
export function resolutionText(overlap: { kind: Overlap["kind"]; copy: Overlap["copy"] }, choice: Choice): string {
  switch (choice) {
    case "mine":
      return overlap.kind === "deleted_modified" ? "kept mine (deleted)" : "kept mine";
    case "theirs":
      return overlap.kind === "modified_deleted" ? "kept GitHub's (deleted)" : "kept GitHub's";
    case "both":
      return overlap.copy ? `kept both (GitHub's saved as ${overlap.copy})` : "kept both";
    case "merged":
      return "merged";
  }
}

/** The merge commit's prefilled message: "Merge from GitHub" and what was chosen for each file. */
export function mergeMessage(preview: PullPreview, picks: Choices): string {
  const lines = preview.overlaps.flatMap((o) => {
    const pick = picks[o.path];
    return pick ? [`- ${o.path}: ${resolutionText(o, pick.choice)}`] : [];
  });
  return lines.length ? `Merge from GitHub\n\n${lines.join("\n")}` : "Merge from GitHub";
}

/** One open review: its choices, the merge commit's message and the merge view's work. */
export class Review {
  readonly preview: PullPreview;
  picks = $state.raw<Choices>({});
  message = $state("");
  /** Set when this review replaced one that went stale. */
  notice = $state<string | null>(null);
  error = $state<string | null>(null);
  busy = $state(false);
  /** Resolve with git: the confirmation is showing, with its commit message. */
  resolveWithGit = $state(false);
  gitMessage = $state("");
  /** The file open in the merge view. */
  merging = $state<string | null>(null);

  /** Apply may run. */
  get ready(): boolean {
    return reviewReady(this.preview, this.picks, this.message);
  }

  #messageEdited = false;
  readonly #snapshots = new Map<string, MergeSnapshot>();

  constructor(preview: PullPreview) {
    this.preview = preview;
    this.picks = initialPicks(preview);
    this.message = mergeMessage(preview, this.picks);
  }

  overlap(path: string): Overlap | undefined {
    return this.preview.overlaps.find((o) => o.path === path);
  }

  setPick(path: string, pick: Chosen | null): void {
    this.picks = { ...this.picks, [path]: pick };
    this.error = null;
    if (!this.#messageEdited) this.message = mergeMessage(this.preview, this.picks);
  }

  editMessage(message: string): void {
    this.message = message;
    this.#messageEdited = true;
  }

  /** What the merge view reopens for a file: the editor as Done left it. */
  snapshot(path: string): MergeSnapshot | undefined {
    return this.#snapshots.get(path);
  }

  /** Done in the merge view: the file is merged, with this text. */
  merged(path: string, text: string, snapshot: MergeSnapshot): void {
    this.#snapshots.set(path, snapshot);
    this.setPick(path, { choice: "merged", text });
  }

  resolutions(): Resolved[] {
    return resolutions(this.preview, this.picks);
  }

  /** Apply's message: only a merge commit has one. */
  commitMessage(): string | null {
    return this.preview.plan === "merge" ? this.message.trim() : null;
  }
}
