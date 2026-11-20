// Typed wrappers around the Tauri commands, plus the platform glue the UI needs.
// Outside Tauri (plain browser, vitest) the in-memory mock backend is used instead.

import { convertFileSrc, invoke, isTauri, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import type {
  AppInfo,
  ApplyResult,
  AuthStatus,
  CommandError,
  CommitPushResult,
  DevicePoll,
  DeviceStart,
  EntryContent,
  EntryMeta,
  ErrorKind,
  FileDiff,
  GithubUrls,
  ImportedImage,
  PullResult,
  RepoInfo,
  Resolved,
  RestoreResult,
  SearchHit,
  Settings,
  SyncView,
} from "./types";

export type Unlisten = () => void;

export interface Api {
  /** True inside the Tauri shell, false for the mock backend. */
  readonly native: boolean;

  appInfo(): Promise<AppInfo>;
  getSettings(): Promise<Settings>;
  setSettings(settings: Settings): Promise<Settings>;

  listEntries(): Promise<EntryMeta[]>;
  readEntry(file: string): Promise<EntryContent>;
  writeEntry(file: string, content: string): Promise<{ hash: string }>;
  createEntry(title: string): Promise<EntryMeta>;
  renameEntry(file: string, newTitle: string): Promise<EntryMeta>;
  deleteEntry(file: string): Promise<null>;
  search(query: string): Promise<SearchHit[]>;
  importImage(entryFile: string, bytes: Uint8Array<ArrayBuffer>): Promise<ImportedImage>;

  syncState(): Promise<SyncView>;
  fetch(): Promise<SyncView>;
  /** Fetches, then applies at once only if nothing changed on both sides; otherwise a review. */
  pull(): Promise<PullResult>;
  /** The review for what was last fetched; local only and never applies (`up_to_date` or `review`). */
  pullPreview(): Promise<PullResult>;
  /** Writes the review's choices; `stale_review` if anything changed since it was computed. */
  applyPull(id: string, resolutions: Resolved[], message: string | null): Promise<ApplyResult>;
  /** Reverses the last pull (`view.lastSync`), or fails with `undo_refused`. */
  undoSync(): Promise<RestoreResult>;
  /** Resolve with git: leaves a standard merge in progress. `message` commits uncommitted edits first. */
  startExternalMerge(id: string, message: string | null): Promise<SyncView>;
  /** `git merge --abort`, and uncommits what Resolve with git committed first. */
  abortMerge(): Promise<RestoreResult>;
  /** A commit on this device only, of the given changed paths. */
  commit(message: string, paths: string[]): Promise<SyncView>;
  /** Commits the given paths and pushes; a blank message with no paths only pushes waiting commits. */
  commitAndPush(message: string, paths: string[]): Promise<CommitPushResult>;
  /** One changed file against HEAD. */
  diff(path: string): Promise<FileDiff>;

  authStatus(): Promise<AuthStatus>;
  githubDeviceStart(): Promise<DeviceStart>;
  githubDevicePoll(): Promise<DevicePoll>;
  githubSetPat(token: string): Promise<{ login: string }>;
  githubListRepos(): Promise<RepoInfo[]>;
  githubConnect(fullName: string): Promise<PullResult>;
  githubDisconnect(signOut: boolean): Promise<SyncView>;
  githubUrls(): Promise<GithubUrls>;
  /** The licences of everything the app is built from, as markdown. */
  thirdPartyNotices(): Promise<string>;

  onSyncState(cb: (view: SyncView) => void): Promise<Unlisten>;
  onEntriesChanged(cb: () => void): Promise<Unlisten>;
  /** The window (or the iOS app) came back to the foreground. */
  onWindowFocus(cb: () => void): Promise<Unlisten>;
  /** The window is about to close (on macOS, also on Quit); it stays open if `cb` says false. */
  onCloseRequested(cb: () => Promise<boolean>): Promise<Unlisten>;
  /** Closes the window without asking `onCloseRequested`. */
  closeWindow(): Promise<void>;
  openUrl(url: string): Promise<void>;
  revealPath(path: string): Promise<void>;
  /** URL the WebView can load for an absolute file path inside the journal. */
  assetUrl(path: string): string;
}

/** Every failure that reaches the UI; `message` is meant to be shown as-is. */
export class ApiError extends Error {
  constructor(
    readonly kind: ErrorKind | "unknown",
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export function toApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e;
  if (isCommandError(e)) return new ApiError(e.kind, e.message);
  if (e instanceof Error) return new ApiError("unknown", e.message);
  return new ApiError("unknown", typeof e === "string" ? e : "Something went wrong.");
}

function isCommandError(e: unknown): e is CommandError {
  return (
    typeof e === "object" &&
    e !== null &&
    typeof (e as CommandError).kind === "string" &&
    typeof (e as CommandError).message === "string"
  );
}

async function call<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  try {
    return await invoke<T>(cmd, args, options);
  } catch (e) {
    throw toApiError(e);
  }
}

/** Runs `cb` when the page becomes visible again, which is also how iOS reports foregrounding. */
export function onVisible(cb: () => void): Unlisten {
  return onVisibility("visible", cb);
}

/** Runs `cb` when the page is hidden: minimised, hidden, or on iOS sent to the background. */
export function onHidden(cb: () => void): Unlisten {
  return onVisibility("hidden", cb);
}

function onVisibility(state: DocumentVisibilityState, cb: () => void): Unlisten {
  const handler = () => {
    if (document.visibilityState === state) cb();
  };
  document.addEventListener("visibilitychange", handler);
  return () => document.removeEventListener("visibilitychange", handler);
}

export function createTauriApi(): Api {
  return {
    native: true,

    appInfo: () => call("app_info"),
    getSettings: () => call("get_settings"),
    setSettings: (settings) => call("set_settings", { settings }),

    listEntries: () => call("list_entries"),
    readEntry: (file) => call("read_entry", { file }),
    writeEntry: (file, content) => call("write_entry", { file, content }),
    createEntry: (title) => call("create_entry", { title }),
    renameEntry: (file, newTitle) => call("rename_entry", { file, newTitle }),
    deleteEntry: (file) => call("delete_entry", { file }),
    search: (query) => call("search", { query }),
    // Header values must be ASCII; app-made names already are, so encoding only matters for
    // hand-made files in "Other".
    importImage: (entryFile, bytes) =>
      call("import_image", bytes, { headers: { "x-entry-file": encodeURIComponent(entryFile) } }),

    syncState: () => call("sync_state"),
    fetch: () => call("fetch"),
    pull: () => call("pull"),
    pullPreview: () => call("pull_preview"),
    applyPull: (id, resolutions, message) => call("apply_pull", { id, resolutions, message }),
    undoSync: () => call("undo_sync"),
    startExternalMerge: (id, message) => call("start_external_merge", { id, message }),
    abortMerge: () => call("abort_merge"),
    commit: (message, paths) => call("commit", { message, paths }),
    commitAndPush: (message, paths) => call("commit_and_push", { message, paths }),
    diff: (path) => call("diff", { path }),

    authStatus: () => call("auth_status"),
    githubDeviceStart: () => call("github_device_start"),
    githubDevicePoll: () => call("github_device_poll"),
    githubSetPat: (token) => call("github_set_pat", { token }),
    githubListRepos: () => call("github_list_repos"),
    githubConnect: (fullName) => call("github_connect", { fullName }),
    githubDisconnect: (signOut) => call("github_disconnect", { signOut }),
    githubUrls: () => call("github_urls"),
    thirdPartyNotices: () => call("third_party_notices"),

    onSyncState: (cb) => listen<SyncView>("sync://state", (e) => cb(e.payload)),
    onEntriesChanged: (cb) => listen("entries://changed", () => cb()),
    async onWindowFocus(cb) {
      const unlistenFocus = await getCurrentWindow().onFocusChanged((e) => {
        if (e.payload) cb();
      });
      const unlistenVisible = onVisible(cb);
      return () => {
        unlistenFocus();
        unlistenVisible();
      };
    },
    onCloseRequested: (cb) =>
      getCurrentWindow().onCloseRequested(async (event) => {
        if (!(await cb())) event.preventDefault();
      }),
    closeWindow: () => getCurrentWindow().destroy(),
    openUrl: async (url) => {
      try {
        await openUrl(url);
      } catch (e) {
        throw toApiError(e);
      }
    },
    revealPath: async (path) => {
      try {
        await revealItemInDir(path);
      } catch (e) {
        throw toApiError(e);
      }
    },
    assetUrl: (path) => convertFileSrc(path),
  };
}

export async function createApi(): Promise<Api> {
  if (isTauri()) return createTauriApi();
  const { createMockApi } = await import("./mock");
  const api = createMockApi({ latencyMs: 120 });
  Object.assign(window, { mdgjMock: api.mock });
  return api;
}
