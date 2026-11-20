// TypeScript mirror of the Tauri command types in docs/architecture.md.

export type Platform = "macos" | "linux" | "ios";
export type Mode = "source" | "styled";
export type Theme = "system" | "light" | "dark";
export type AuthKind = "github_app" | "pat";

export interface AppInfo {
  version: string;
  platform: Platform;
  journalDir: string;
  tokenBackend: string;
  clientIdPlaceholder: boolean;
}

export interface RemoteConfig {
  fullName: string;
  url: string;
  branch: string;
  auth: AuthKind;
}

export interface Settings {
  vim: boolean;
  modeDesktop: Mode;
  modeMobile: Mode;
  fetchIntervalMin: number;
  imageMaxEdge: number;
  imageQuality: number;
  theme: Theme;
  authorName: string | null;
  authorEmail: string | null;
  remote: RemoteConfig | null;
}

export interface EntryMeta {
  file: string;
  date: string | null;
  time: string | null;
  title: string;
  mtime: number;
  size: number;
  words: number | null;
}

export interface EntryContent {
  file: string;
  content: string;
  hash: string;
}

export interface SearchHit {
  file: string;
  title: string;
  line: number;
  snippet: string;
  /** Offsets in Unicode code points (Rust chars), not UTF-16 units. */
  matchStart: number;
  matchEnd: number;
}

export interface ImportedImage {
  path: string;
  bytes: number;
  width: number;
  height: number;
}

export type ChangeKind = "added" | "modified" | "deleted" | "renamed";

export interface FileChange {
  path: string;
  kind: ChangeKind;
  oldPath?: string | null;
}

export type ErrorKind =
  | "auth"
  | "reauth"
  | "network"
  | "not_connected"
  | "empty_message"
  | "invalid"
  | "io"
  | "git"
  /** The working tree, HEAD or GitHub's branch moved since the review was computed. */
  | "stale_review"
  /** A git merge is in progress (Resolve with git, or the git CLI): finish or abort it first. */
  | "merge_in_progress"
  /** Undo would overwrite newer text or rewind pushed history; the message says which. */
  | "undo_refused";

export interface CommandError {
  kind: ErrorKind;
  message: string;
}

/** A git merge in progress: `MERGE_HEAD` exists. */
export interface MergeState {
  /** Paths with conflict entries in the index, sorted. */
  conflicted: string[];
  /** Started by Resolve with git (rather than with the git CLI). */
  startedByApp: boolean;
}

export interface SyncView {
  connected: boolean;
  repo: string | null;
  branch: string;
  ahead: number;
  behind: number;
  changes: FileChange[];
  lastFetch: number | null;
  /** Why the last sync operation failed, until one succeeds (the red dot). */
  error: CommandError | null;
  /** A git merge is in progress. */
  merge: MergeState | null;
  /** The pull Undo would reverse; null when there is nothing to undo. */
  lastSync: SyncSummary | null;
}

/** What Apply does to history. */
export type PullPlan = "fast_forward" | "rebase" | "merge";

export interface CommitInfo {
  /** Full object id. */
  id: string;
  /** Full message, trailing whitespace trimmed; the UI shows the first line. */
  message: string;
  /** Author name. */
  author: string;
  /** Unix ms. */
  time: number;
}

export type OverlapKind =
  | "both_modified"
  | "both_added"
  /** Mine modified, GitHub deleted. */
  | "modified_deleted"
  /** Mine deleted, GitHub modified. */
  | "deleted_modified";

export type Choice = "mine" | "theirs" | "both" | "merged";

/** Whole lines; joining every chunk's text (a clash contributing its pick) gives the merged file byte for byte. */
export type MergeChunk =
  /** Unchanged on both sides. */
  | { kind: "same"; text: string }
  /** Changed on one side only (`mine`, `theirs`) or the same way on both (`both`): already applied. `text` "" = lines deleted. */
  | { kind: "mine" | "theirs" | "both"; text: string; base: string }
  /** Changed differently on both sides: the user picks. */
  | { kind: "clash"; base: string; mine: string; theirs: string };

export interface MergeSuggestion {
  /** No clash chunks. */
  clean: boolean;
  chunks: MergeChunk[];
}

/** A path changed on both sides, where mine (the working tree) differs from GitHub's. */
export interface Overlap {
  path: string;
  kind: OverlapKind;
  /** Some present side is not UTF-8 or contains a NUL byte. */
  binary: boolean;
  /** Full text; null if that side is absent or the file is binary. */
  base: string | null;
  mine: string | null;
  theirs: string | null;
  /** Allowed, in this order: text [mine, theirs, both, merged]; binary [mine, theirs, both]; delete/modify [mine, theirs]. */
  choices: Choice[];
  /** Where Keep both puts GitHub's version; set iff `both` is allowed. */
  copy: string | null;
  /** Set iff `merged` is allowed. */
  suggestion: MergeSuggestion | null;
}

export interface PullPreview {
  /** Opaque fingerprint of branch, HEAD, origin's branch and the working tree. */
  id: string;
  branch: string;
  plan: PullPlan;
  /** No overlaps and the plan isn't a merge: Pull applies it without asking. */
  automatic: boolean;
  /** GitHub's new commits, newest first, at most 50. */
  incoming: CommitInfo[];
  incomingCount: number;
  /** Unpushed commits here, newest first, at most 50. */
  local: CommitInfo[];
  localCount: number;
  /** Changes from GitHub to files changed there only, as they will change the working tree. */
  auto: FileChange[];
  /** Sorted by path. */
  overlaps: Overlap[];
}

export type Resolved =
  | { path: string; choice: "mine" | "theirs" | "both" }
  | { path: string; choice: "merged"; text: string };

export interface ResolvedFile {
  path: string;
  choice: Choice;
  copy: string | null;
}

export interface SyncSummary {
  /** Unix ms. */
  at: number;
  plan: PullPlan;
  /** Newest first, at most 50. */
  incoming: CommitInfo[];
  incomingCount: number;
  /** Unpushed commits placed after GitHub's (plan `rebase`), else 0. */
  rebased: number;
  /** The merge commit's id (plan `merge`). */
  mergeCommit: string | null;
  /** What the pull changed in the working tree, relative to before it; includes Keep both copies. */
  files: FileChange[];
  resolved: ResolvedFile[];
}

export type PullResult =
  | { status: "up_to_date"; view: SyncView }
  | { status: "applied"; view: SyncView; summary: SyncSummary }
  | { status: "review"; view: SyncView; preview: PullPreview };

export interface ApplyResult {
  view: SyncView;
  summary: SyncSummary;
}

/** Undo and Abort merge: the paths put back. */
export interface RestoreResult {
  view: SyncView;
  restored: string[];
}

export type DiffLineKind = "context" | "added" | "removed";

export interface DiffLine {
  kind: DiffLineKind;
  /** Without its line break. */
  text: string;
}

export interface DiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

/** One changed file against HEAD. */
export interface FileDiff {
  path: string;
  oldPath: string | null;
  kind: ChangeKind;
  binary: boolean;
  /** Empty if binary. */
  hunks: DiffHunk[];
}

export type CommitPushStatus = "pushed" | "needs_pull" | "nothing";

export interface CommitPushResult {
  view: SyncView;
  result: CommitPushStatus;
}

export interface AuthStatus {
  signedIn: boolean;
  kind: AuthKind | null;
  login: string | null;
}

export interface DeviceStart {
  userCode: string;
  verificationUri: string;
  expiresIn: number;
  interval: number;
}

export type DevicePollStatus = "pending" | "slow_down" | "complete" | "expired" | "denied";

export interface DevicePoll {
  status: DevicePollStatus;
  login?: string | null;
}

export interface RepoInfo {
  fullName: string;
  private: boolean;
  cloneUrl: string;
  defaultBranch: string;
}

export interface GithubUrls {
  newRepo: string;
  install: string;
}
