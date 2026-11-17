// How the two sync buttons look, derived from the backend's SyncView (PLAN §5).

import type { SyncView } from "./types";

/** Operations the UI shows as busy. "commit" covers both commit and commit_and_push. */
export type SyncOp = "fetch" | "pull" | "commit" | "connect";

export interface PullButton {
  accent: boolean;
  badge: number | null;
  busy: boolean;
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
  const busy = ops.includes("pull") || ops.includes("fetch");
  const behind = view.behind;
  let title = "Up to date";
  if (ops.includes("pull")) title = "Pulling…";
  else if (ops.includes("fetch")) title = "Checking GitHub…";
  else if (behind > 0) title = `${plural(behind, "new commit")} on GitHub`;
  return { accent: behind > 0, badge: behind > 0 ? behind : null, busy, title };
}

export function commitButton(view: SyncView, ops: readonly SyncOp[]): CommitButton {
  const changed = view.changes.length;
  const ahead = view.connected ? view.ahead : 0;
  const pullFirst = view.connected && view.behind > 0;
  const hasWork = changed > 0 || ahead > 0;

  let title = "No changes";
  if (pullFirst) title = "GitHub has new changes. Pull first.";
  else if (changed > 0) title = plural(changed, "changed file");
  else if (ahead > 0) title = `${plural(ahead, "local commit")} to push`;

  return {
    label: view.connected ? "Commit & Push" : "Commit",
    accent: hasWork && !pullFirst,
    badge: changed > 0 ? changed : null,
    disabled: pullFirst || !hasWork,
    hint: pullFirst ? "Pull first" : null,
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
