// Typed wrappers around the Tauri commands, plus the platform glue the UI needs.
// Outside Tauri (plain browser, vitest) the in-memory mock backend is used instead.

import { convertFileSrc, invoke, isTauri, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import type {
  AppInfo,
  AuthStatus,
  CommandError,
  CommitPushResult,
  DevicePoll,
  DeviceStart,
  EntryContent,
  EntryMeta,
  ErrorKind,
  GithubUrls,
  ImportedImage,
  PullResult,
  RepoInfo,
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
  pull(): Promise<PullResult>;
  commit(message: string): Promise<SyncView>;
  commitAndPush(message: string): Promise<CommitPushResult>;

  authStatus(): Promise<AuthStatus>;
  githubDeviceStart(): Promise<DeviceStart>;
  githubDevicePoll(): Promise<DevicePoll>;
  githubSetPat(token: string): Promise<{ login: string }>;
  githubListRepos(): Promise<RepoInfo[]>;
  githubConnect(fullName: string): Promise<PullResult>;
  githubDisconnect(signOut: boolean): Promise<SyncView>;
  githubUrls(): Promise<GithubUrls>;

  onSyncState(cb: (view: SyncView) => void): Promise<Unlisten>;
  onEntriesChanged(cb: () => void): Promise<Unlisten>;
  /** The window (or the iOS app) came back to the foreground. */
  onWindowFocus(cb: () => void): Promise<Unlisten>;
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
  const handler = () => {
    if (document.visibilityState === "visible") cb();
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
    commit: (message) => call("commit", { message }),
    commitAndPush: (message) => call("commit_and_push", { message }),

    authStatus: () => call("auth_status"),
    githubDeviceStart: () => call("github_device_start"),
    githubDevicePoll: () => call("github_device_poll"),
    githubSetPat: (token) => call("github_set_pat", { token }),
    githubListRepos: () => call("github_list_repos"),
    githubConnect: (fullName) => call("github_connect", { fullName }),
    githubDisconnect: (signOut) => call("github_disconnect", { signOut }),
    githubUrls: () => call("github_urls"),

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
