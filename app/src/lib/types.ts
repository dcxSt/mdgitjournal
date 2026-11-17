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

export type ErrorKind = "auth" | "reauth" | "network" | "not_connected" | "empty_message" | "invalid" | "io" | "git";

export interface CommandError {
  kind: ErrorKind;
  message: string;
}

export interface SyncView {
  connected: boolean;
  repo: string | null;
  branch: string;
  ahead: number;
  behind: number;
  changes: FileChange[];
  lastFetch: number | null;
  error: CommandError | null;
}

export interface Conflict {
  path: string;
  /** Where the local version was saved; null if it was deleted locally. */
  copy: string | null;
}

export interface PullResult {
  view: SyncView;
  updated: string[];
  merged: string[];
  conflicts: Conflict[];
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
