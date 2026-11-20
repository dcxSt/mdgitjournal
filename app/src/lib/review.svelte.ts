// The Sync review (docs/architecture.md, "Sync flows in the UI"): what the user chose for each
// file changed on both sides, whether Apply may run, and what it sends. The pure functions do
// the deciding; `Review` holds one review's choices while its sheet and merge view are open.

import { joinChunks, type MergeSnapshot } from "../editor/merge";
import { pathLabel } from "./journal";
import { commitMessageValid } from "./sync";
import type { Choice, MergeChunk, Overlap, OverlapKind, PullPreview, Resolved } from "./types";

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

/** The kinds where one side deleted the file: the choices keep it or don't. */
const KEEP_OR_DELETE: Record<string, { mine: string; theirs: string }> = {
  modified_deleted: { mine: "Keep mine", theirs: "Delete" },
  renamed_deleted: { mine: "Keep mine", theirs: "Delete" },
  deleted_modified: { mine: "Keep deleted", theirs: "Keep GitHub's" },
  deleted_renamed: { mine: "Keep deleted", theirs: "Keep GitHub's" },
};

/** The choice's labels: deletions name what each choice does to the file. */
export function choiceLabel(overlap: Overlap, choice: Choice): string {
  const deletion = KEEP_OR_DELETE[overlap.kind];
  if (deletion && (choice === "mine" || choice === "theirs")) return deletion[choice];
  return { mine: "Keep mine", theirs: "Keep GitHub's", both: "Keep both", merged: "Merge…" }[choice];
}

/** Where the row's file is known to the user: its name here, or GitHub's if it is gone here. */
export function overlapName(overlap: Overlap): string {
  return overlap.minePath ?? overlap.theirsPath ?? overlap.path;
}

/** “Long run” (an entry's title) or the path. */
function named(path: string | null): string {
  return `“${path === null ? "" : pathLabel(path).title}”`;
}

/** A line saying how this file was changed on both sides. */
export function overlapNote(overlap: Overlap): string {
  const what = /\.(jpe?g|png|gif|webp|heic|avif)$/i.test(overlap.path) ? "image" : "file";
  switch (overlap.kind) {
    case "modified_deleted":
      return "Changed here, deleted on GitHub";
    case "deleted_modified":
      return "Deleted here, changed on GitHub";
    case "modified_renamed":
      return `Changed here, renamed on GitHub to ${named(overlap.theirsPath)}`;
    case "renamed_modified":
      return `Renamed here, changed on GitHub (where it is still ${named(overlap.theirsPath)})`;
    case "both_renamed":
      return `Renamed here, and on GitHub to ${named(overlap.theirsPath)}`;
    case "renamed_deleted":
      return "Renamed here, deleted on GitHub";
    case "deleted_renamed":
      return "Deleted here, renamed on GitHub";
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

const DELETED_HERE: OverlapKind[] = ["deleted_modified", "deleted_renamed"];
const DELETED_THERE: OverlapKind[] = ["modified_deleted", "renamed_deleted"];

/** How a resolution reads in the merge commit's message and the summary. */
export function resolutionText(overlap: { kind: OverlapKind; copy: string | null }, choice: Choice): string {
  switch (choice) {
    case "mine":
      return DELETED_HERE.includes(overlap.kind) ? "kept mine (deleted)" : "kept mine";
    case "theirs":
      return DELETED_THERE.includes(overlap.kind) ? "kept GitHub's (deleted)" : "kept GitHub's";
    case "both":
      return overlap.copy ? `kept both (GitHub's saved as ${overlap.copy})` : "kept both";
    case "merged":
      return "merged";
  }
}

/**
 * The merge commit's prefilled message: "Merge from GitHub" and what was chosen for each file the
 * commit records (the others' resolutions stay as changes for the next commit).
 */
export function mergeMessage(preview: PullPreview, picks: Choices): string {
  const lines = preview.overlaps.flatMap((o) => {
    const pick = picks[o.path];
    return pick && o.committed ? [`- ${o.path}: ${resolutionText(o, pick.choice)}`] : [];
  });
  return lines.length ? `Merge from GitHub\n\n${lines.join("\n")}` : "Merge from GitHub";
}

/** The same file, changed the same way, in a fresh review: a choice made for it still holds. */
function unchanged(before: Overlap | undefined, after: Overlap): boolean {
  return (
    before !== undefined &&
    before.kind === after.kind &&
    before.versions === after.versions &&
    before.copy === after.copy &&
    before.choices.join() === after.choices.join()
  );
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
  /** Files of a stale review whose choices couldn't carry over (they changed, or are new). */
  readonly reset: string[] = [];

  /** Apply may run. */
  get ready(): boolean {
    return reviewReady(this.preview, this.picks, this.message);
  }

  /** The user has done something a stray Esc shouldn't throw away. */
  get dirty(): boolean {
    const initial = initialPicks(this.preview);
    const picked = this.preview.overlaps.some((o) => JSON.stringify(this.picks[o.path]) !== JSON.stringify(initial[o.path]));
    return picked || this.#snapshots.size > 0 || this.#messageEdited || this.gitMessage.trim() !== "";
  }

  #messageEdited = false;
  readonly #snapshots = new Map<string, MergeSnapshot>();

  /**
   * A review of `preview`. With `previous` (a review that went stale), every choice and merge for
   * a file that is the same in both is kept; the rest start afresh and are listed in `reset`.
   */
  constructor(preview: PullPreview, previous?: Review) {
    this.preview = preview;
    const picks: Record<string, Chosen | null> = { ...initialPicks(preview) };
    if (previous) {
      for (const o of preview.overlaps) {
        if (unchanged(previous.overlap(o.path), o)) {
          picks[o.path] = previous.picks[o.path] ?? null;
          const snapshot = previous.snapshot(o.path);
          if (snapshot) this.#snapshots.set(o.path, snapshot);
        } else {
          this.reset.push(o.path);
        }
      }
      this.gitMessage = previous.gitMessage;
    }
    this.picks = picks;
    if (previous && previous.#messageEdited) {
      this.message = previous.message;
      this.#messageEdited = true;
    } else {
      this.message = mergeMessage(preview, this.picks);
    }
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
