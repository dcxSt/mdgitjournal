// How the two sync buttons look, derived from the backend's SyncView (PLAN §5), and the words
// the sync notices use.

import type { ChangeKind, CommitInfo, FileChange, SyncSummary, SyncView } from "./types";

/**
 * Operations the UI shows as busy. "commit" covers both commit and commit_and_push; "apply" is
 * any other change to the working tree by git: applying a review, Undo, Resolve with git, Abort.
 */
export type SyncOp = "fetch" | "pull" | "commit" | "connect" | "apply";

export const MERGE_TITLE = "Merge in progress — finish it with git";

export interface PullButton {
  accent: boolean;
  badge: number | null;
  busy: boolean;
  disabled: boolean;
  title: string;
}

export interface CommitButton {
  label: "Commit & Push" | "Commit";
  accent: boolean;
  /** Number of changed files. */
  badge: number | null;
  disabled: boolean;
  /** Shown next to the button when it is disabled because GitHub is ahead. */
  hint: "Pull first" | null;
  busy: boolean;
  title: string;
}

export function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}

export function pullButton(view: SyncView, ops: readonly SyncOp[]): PullButton {
  const pulling = ops.includes("pull") || ops.includes("apply");
  const busy = pulling || ops.includes("fetch");
  const behind = view.behind;
  let title = "Up to date";
  if (view.merge) title = MERGE_TITLE;
  else if (ops.includes("pull")) title = "Pulling…";
  else if (ops.includes("apply")) title = "Updating files…";
  else if (ops.includes("fetch")) title = "Checking GitHub…";
  else if (behind > 0) title = `${plural(behind, "new commit")} on GitHub`;
  return {
    accent: behind > 0 && !view.merge,
    badge: behind > 0 ? behind : null,
    busy,
    disabled: pulling || view.merge !== null,
    title,
  };
}

export function commitButton(view: SyncView, ops: readonly SyncOp[]): CommitButton {
  const changed = view.changes.length;
  const ahead = view.connected ? view.ahead : 0;
  const pullFirst = view.connected && view.behind > 0;
  const merging = view.merge !== null;
  const hasWork = changed > 0 || ahead > 0;

  let title = "No changes";
  if (merging) title = MERGE_TITLE;
  else if (pullFirst) title = "GitHub has new changes. Pull first.";
  else if (changed > 0) title = plural(changed, "changed file");
  else if (ahead > 0) title = `${plural(ahead, "local commit")} to push`;

  return {
    label: view.connected ? "Commit & Push" : "Commit",
    accent: hasWork && !pullFirst && !merging,
    badge: changed > 0 ? changed : null,
    disabled: merging || pullFirst || !hasWork,
    hint: pullFirst && !merging ? "Pull first" : null,
    busy: ops.includes("commit"),
    title,
  };
}

/** Connected, nothing uncommitted, but local commits waiting: the sheet only pushes. */
export function isPushOnly(view: SyncView): boolean {
  return view.connected && view.changes.length === 0 && view.ahead > 0;
}

export function commitMessageValid(message: string): boolean {
  return message.trim().length > 0;
}

/** Error kinds that mean the sync itself failed (the red dot); the rest answer the sheet that asked. */
export function isSyncFailure(kind: string): boolean {
  return ["auth", "reauth", "network", "not_connected", "io", "git", "unknown"].includes(kind);
}

/**
 * "Pulled 3 commits from GitHub · 4 files changed · your 2 unpushed commits were placed after them",
 * and what is left to do: a merge commit to push, resolved files still uncommitted (in `changes`).
 */
export function summaryText(summary: SyncSummary, changes: readonly FileChange[] = []): string {
  const parts = [`Pulled ${plural(summary.incomingCount, "commit")} from GitHub`];
  if (summary.files.length) parts.push(`${plural(summary.files.length, "file")} changed`);
  if (summary.rebased > 0) {
    parts.push(`your ${plural(summary.rebased, "unpushed commit")} ${summary.rebased === 1 ? "was" : "were"} placed after them`);
  }
  if (summary.mergeCommit) parts.push("merge commit ready to push");
  const changed = new Set(changedPaths(changes));
  const waiting = summary.resolved.filter((r) => changed.has(r.path) || (r.copy !== null && changed.has(r.copy))).length;
  if (waiting) parts.push(`${plural(waiting, "resolved file")} ready to commit`);
  return parts.join(" · ");
}

/** "Your 2 unpushed commits were placed after the 3 from GitHub." */
export function rebasedText(summary: { rebased: number; incomingCount: number }): string {
  const n = summary.rebased;
  return `Your ${plural(n, "unpushed commit")} ${n === 1 ? "was" : "were"} placed after the ${summary.incomingCount} from GitHub.`;
}

const CHANGE_WORDS: Record<ChangeKind, string> = {
  added: "new",
  modified: "updated",
  deleted: "deleted",
  renamed: "renamed",
};

export function changeWord(kind: ChangeKind): string {
  return CHANGE_WORDS[kind];
}

/** Every path a set of changes touched, the old names of renames included. */
export function changedPaths(changes: readonly FileChange[]): string[] {
  return changes.flatMap((c) => (c.oldPath ? [c.path, c.oldPath] : [c.path]));
}

/** The first line of a commit message. */
export function subject(commit: CommitInfo): string {
  return commit.message.split("\n", 1)[0];
}

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;

/** "just now", "5 min ago", "3 h ago", "yesterday", then a date. */
export function relativeTime(ms: number, now = Date.now()): string {
  const ago = now - ms;
  if (ago < MINUTE) return "just now";
  if (ago < 60 * MINUTE) return `${Math.floor(ago / MINUTE)} min ago`;
  const then = new Date(ms);
  const today = new Date(now);
  const days = Math.round(
    (new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime() -
      new Date(then.getFullYear(), then.getMonth(), then.getDate()).getTime()) /
      DAY,
  );
  if (days === 0) return `${Math.floor(ago / (60 * MINUTE))} h ago`;
  if (days === 1) return "yesterday";
  const sameYear = then.getFullYear() === today.getFullYear();
  return then.toLocaleDateString(undefined, { day: "numeric", month: "short", year: sameYear ? undefined : "numeric" });
}
