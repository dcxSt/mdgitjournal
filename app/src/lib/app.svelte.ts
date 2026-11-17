// App-wide state and the orchestration the frontend owns (docs/architecture.md, "Frontend owns
// timing"): fetch on launch, focus and an interval; flush autosave before git work; reload the open
// entry after a pull. Operations that touch the working tree run one at a time.

import { imageLink } from "../editor/commands";
import { ApiError, onHidden, toApiError, type Api, type Unlisten } from "./api";
import { DEFAULT_SETTINGS, EMPTY_VIEW, SIGNED_OUT } from "./defaults";
import { CurrentEntry, SaveError } from "./entry.svelte";
import { conflictCopyName, isEntryFile } from "./journal";
import { Notices, type Notice } from "./notices.svelte";
import { Search } from "./search.svelte";
import type { Action } from "./shortcuts";
import { commitButton, commitMessageValid, isPushOnly, plural, type SyncOp } from "./sync";
import type {
  AppInfo,
  AuthStatus,
  CommandError,
  Conflict,
  EntryMeta,
  Mode,
  Platform,
  PullResult,
  Settings,
  SyncView,
} from "./types";

export type Overlay = "commit" | "settings" | "quickOpen" | "delete";

/** Below this width a browser preview uses the phone layout. */
export const MOBILE_MAX_WIDTH = 700;
const FOCUS_REFRESH_GAP_MS = 2000;
const TOAST_MS = 3000;

export const NEEDS_PULL_TEXT = "GitHub has new changes — Pull first. Your commit is saved locally.";

/** iOS always gets the phone layout; desktop windows never do (their minimum width fits both panes). */
export function mobileLayout(platform: Platform, width: number, native: boolean): boolean {
  return platform === "ios" || (!native && width < MOBILE_MAX_WIDTH);
}

export class AppState {
  info = $state.raw<AppInfo | null>(null);
  settings = $state.raw<Settings>(DEFAULT_SETTINGS);
  entries = $state.raw<EntryMeta[]>([]);
  auth = $state.raw<AuthStatus>(SIGNED_OUT);
  sync = $state.raw<SyncView>(EMPTY_VIEW);
  /** Error of the last sync operation run from this window. */
  syncError = $state.raw<CommandError | ApiError | null>(null);
  ops = $state<SyncOp[]>([]);
  ready = $state(false);
  initError = $state<string | null>(null);

  /** Window width in CSS pixels, kept current by the root component. */
  width = $state(0);
  systemDark = $state(false);
  mode = $state<Mode>("source");
  overlay = $state<Overlay | null>(null);
  sidebarOpen = $state(true);
  newPrompt = $state(false);
  /** Focus requests, cleared by the component that takes focus (it may not be mounted yet). */
  focusEditor = $state(false);
  focusSearch = $state(false);

  readonly current: CurrentEntry;
  readonly search: Search;
  readonly notices = new Notices();

  mobile = $derived.by(() => this.info !== null && mobileLayout(this.info.platform, this.width, this.api.native));
  dark = $derived(this.settings.theme === "dark" || (this.settings.theme === "system" && this.systemDark));
  currentMeta = $derived(this.entries.find((e) => e.file === this.current.file) ?? null);
  lastError = $derived(this.syncError ?? this.sync.error);
  mac = $derived(this.info?.platform === "macos" || this.info?.platform === "ios");

  #lock: Promise<unknown> = Promise.resolve();
  #unlisten: Unlisten[] = [];
  #fetchTimer: ReturnType<typeof setInterval> | undefined;
  #lastFocusRefresh = 0;
  #syncRefresh: Promise<void> | null = null;
  #syncRefreshAgain = false;
  /** Bumped whenever a git operation or the backend reports the sync state. */
  #syncReports = 0;
  /** The fetch in flight, if any; pull and commit wait for it before pausing autosave. */
  #fetching: Promise<unknown> = Promise.resolve();
  #settingsWrites: Promise<void> = Promise.resolve();

  constructor(readonly api: Api) {
    this.current = new CurrentEntry(
      api,
      () => this.#saved(),
      (file, message, retrying) =>
        this.notices.show({
          key: "save-error",
          tone: "error",
          text: `Couldn't save ${file}: ${message}. Your text is kept here${retrying ? ", and saving is retried" : ""}.`,
        }),
    );
    this.search = new Search(api);
  }

  async init(): Promise<void> {
    try {
      const [info, settings, entries, sync, auth] = await Promise.all([
        this.api.appInfo(),
        this.api.getSettings(),
        this.api.listEntries(),
        this.api.syncState(),
        this.api.authStatus(),
      ]);
      this.info = info;
      this.settings = settings;
      this.entries = entries;
      this.#setSync(sync);
      this.auth = auth;
      this.mode = this.mobile ? settings.modeMobile : settings.modeDesktop;
      this.#unlisten = await Promise.all([
        this.api.onSyncState((view) => this.#setSync(view)),
        this.api.onEntriesChanged(() => void this.refresh()),
        this.api.onWindowFocus(() => void this.onFocus()),
        this.api.onCloseRequested(() => this.beforeClose()),
      ]);
      // A failed save is reported by the entry and retried.
      this.#unlisten.push(onHidden(() => void this.current.flush().catch(() => {})));
    } catch (e) {
      this.initError = toApiError(e).message;
      return;
    }
    this.#armFetchTimer();
    this.ready = true;
    if (!this.mobile && this.entries.length) await this.open(this.entries[0].file);
    await this.fetch();
  }

  dispose(): void {
    for (const unlisten of this.#unlisten) unlisten();
    this.#unlisten = [];
    clearInterval(this.#fetchTimer);
  }

  // Entries

  async refreshEntries(): Promise<void> {
    this.entries = await this.api.listEntries();
  }

  /** Re-reads the list and the open entry (if its buffer is clean) after changes on disk. */
  async refresh(): Promise<void> {
    try {
      await this.refreshEntries();
      const file = this.current.file;
      if (file !== null && !this.entries.some((e) => e.file === file)) {
        // Gone from disk. Unsaved text stays and autosave writes it back.
        if (!this.current.dirty) this.current.close();
      } else {
        await this.current.reloadIfClean();
      }
      this.refreshSync();
    } catch (e) {
      this.#report(e);
    }
  }

  async open(file: string): Promise<boolean> {
    if (file === this.current.file) return true;
    try {
      await this.#serial(async () => {
        const entry = await this.api.readEntry(file);
        await this.current.settle();
        this.current.load(entry);
      });
      return true;
    } catch (e) {
      this.#report(e);
      return false;
    }
  }

  /** Opens an entry picked from a list; on the desktop the editor takes focus. */
  async openEntry(file: string): Promise<void> {
    if ((await this.open(file)) && !this.mobile) this.focusEditor = true;
  }

  /**
   * Phone layout: leaves the editor for the list once the entry is safely on disk. It waits for
   * a pull or commit in progress, which holds autosave back.
   */
  async back(): Promise<void> {
    try {
      await this.#serial(async () => {
        await this.current.settle();
        this.current.close();
      });
    } catch (e) {
      this.#report(e);
    }
  }

  async create(title: string): Promise<boolean> {
    try {
      await this.#serial(async () => {
        const meta = await this.api.createEntry(title.trim() || "untitled");
        const entry = await this.api.readEntry(meta.file);
        await this.refreshEntries();
        await this.current.settle();
        this.current.load(entry);
      });
    } catch (e) {
      this.#report(e);
      return false;
    }
    this.newPrompt = false;
    this.focusEditor = true;
    this.refreshSync();
    return true;
  }

  /** Renames the open entry; only the title part of the file name changes. */
  async rename(title: string): Promise<boolean> {
    const file = this.current.file;
    if (file === null || !title.trim()) return false;
    try {
      await this.#serial(() =>
        this.#withSavesPaused(async () => {
          const meta = await this.api.renameEntry(file, title);
          if (this.current.file === file) this.current.renamed(meta.file);
          await this.refreshEntries();
        }),
      );
    } catch (e) {
      this.#report(e);
      return false;
    }
    this.refreshSync();
    return true;
  }

  /** Deletes `file` if it is still the open entry. */
  async remove(file: string): Promise<void> {
    if (file !== this.current.file) return;
    try {
      await this.#serial(() =>
        this.#withSavesPaused(async () => {
          await this.api.deleteEntry(file);
          this.current.close();
          this.notices.dismiss("save-error");
          await this.refreshEntries();
        }),
      );
    } catch (e) {
      this.#report(e);
      return;
    }
    this.refreshSync();
    if (!this.mobile && this.entries.length) await this.open(this.entries[0].file);
  }

  /**
   * Imports pasted, dropped or picked images into assets/ and returns their relative paths. Each
   * import waits for a pull or commit in progress, and none starts once another entry is open.
   */
  async importImages(files: File[]): Promise<string[]> {
    const entry = this.current.file;
    if (entry === null) return [];
    const paths: string[] = [];
    let skipped = 0;
    for (const file of files) {
      try {
        const bytes = new Uint8Array(await file.arrayBuffer());
        const path = await this.#serial(async () =>
          this.current.file === entry ? (await this.api.importImage(entry, bytes)).path : null,
        );
        if (path === null) skipped++;
        else paths.push(path);
      } catch (e) {
        this.notices.show({
          tone: "error",
          text: `Couldn't add ${file.name || "the image"}: ${toApiError(e).message}`,
        });
      }
    }
    if (skipped) {
      this.notices.show({
        tone: "info",
        text: `${plural(skipped, "image")} not added: the open entry changed while importing. Add ${skipped === 1 ? "it" : "them"} again.`,
      });
    }
    if (paths.length) this.refreshSync();
    return paths;
  }

  /** Images that were imported, but whose entry left the editor before they could be linked. */
  imagesUnlinked(paths: string[]): void {
    const one = paths.length === 1;
    this.notices.show({
      tone: "info",
      text: one
        ? `Saved ${paths[0]} but didn't link it: the entry changed while it was importing. Add this link where you want it, or delete the file:`
        : `Saved ${paths.length} images but didn't link them: the entry changed while they were importing. Add these links where you want them, or delete the files:`,
      files: paths.map((path) => ({ label: imageLink(path) })),
    });
  }

  imageUrl(relPath: string): string {
    return this.info ? this.api.assetUrl(`${this.info.journalDir}/${relPath}`) : "";
  }

  // Sync

  async fetch(userInitiated = false): Promise<void> {
    if (!this.sync.connected || this.ops.length) return;
    const fetched = this.#track("fetch", async () => {
      this.#setSync(await this.api.fetch());
    });
    this.#fetching = fetched;
    const error = await fetched;
    if (error && userInitiated) this.#report(error);
  }

  async pull(): Promise<void> {
    if (!this.sync.connected || this.ops.includes("pull")) return;
    const error = await this.#track("pull", () =>
      this.#gitWork(async () => {
        const result = await this.api.pull();
        this.#setSync(result.view);
        this.notices.dismiss("needs-pull");
        await this.#afterIncoming(result);
      }),
    );
    if (error) this.#report(error);
  }

  /** Opens the commit sheet with an up-to-date list of changes. */
  async openCommit(): Promise<void> {
    if (this.overlay) return;
    if (this.sync.connected && this.sync.behind > 0) {
      this.#toast("GitHub has new changes. Pull first.");
      return;
    }
    try {
      await this.current.flush();
      await this.#readSync();
    } catch (e) {
      this.#report(e);
      return;
    }
    if (commitButton(this.sync, this.ops).disabled) this.#toast("Nothing to commit.");
    else this.overlay = "commit";
  }

  /**
   * Commits (and pushes, when connected). Returns an error for the sheet to show, or null.
   * The message may only be blank when there is nothing to commit and local commits to push.
   */
  async commit(message: string): Promise<ApiError | null> {
    if (!commitMessageValid(message) && !isPushOnly(this.sync)) {
      return new ApiError("empty_message", "Write a commit message.");
    }
    const text = message.trim();
    const error = await this.#track("commit", () =>
      this.#gitWork(async () => {
        if (!this.sync.connected) {
          this.#setSync(await this.api.commit(text));
          this.#toast("Committed.");
          return;
        }
        const { view, result } = await this.api.commitAndPush(text);
        this.#setSync(view);
        if (result === "needs_pull") this.notices.show({ key: "needs-pull", tone: "info", text: NEEDS_PULL_TEXT });
        else this.#toast(result === "pushed" ? `Pushed to ${view.repo ?? "GitHub"}.` : "Nothing to commit.");
      }),
    );
    if (error?.kind === "reauth") this.#report(error);
    return error;
  }

  /** Refreshes the local sync state (changed files) after edits; calls are coalesced. */
  refreshSync(): void {
    if (this.#syncRefresh) {
      this.#syncRefreshAgain = true;
      return;
    }
    this.#syncRefresh = (async () => {
      do {
        this.#syncRefreshAgain = false;
        try {
          // A reply overtaken by a git operation's report may be older; read again after it.
          if (!(await this.#readSync())) this.#syncRefreshAgain = true;
        } catch (e) {
          this.syncError = toApiError(e);
        }
      } while (this.#syncRefreshAgain);
      this.#syncRefresh = null;
    })();
  }

  /** Saves pending edits before the window closes; it stays open, saying why, if that fails. */
  async beforeClose(): Promise<boolean> {
    try {
      await this.current.settle();
      return true;
    } catch (e) {
      this.notices.show({
        key: "close",
        tone: "error",
        text: toApiError(e).message,
        action: { label: "Close anyway", run: () => void this.api.closeWindow() },
      });
      return false;
    }
  }

  async onFocus(): Promise<void> {
    const now = Date.now();
    if (now - this.#lastFocusRefresh < FOCUS_REFRESH_GAP_MS) return;
    this.#lastFocusRefresh = now;
    await this.refresh();
    await this.fetch();
  }

  // GitHub

  async refreshAuth(): Promise<void> {
    this.auth = await this.api.authStatus();
  }

  /** Connects the journal to `owner/repo`; errors are thrown for the connect flow to show. */
  async connect(fullName: string): Promise<PullResult> {
    const result = await this.#gitWork(async () => {
      const result = await this.api.githubConnect(fullName);
      this.#setSync(result.view);
      this.syncError = null;
      await this.#afterIncoming(result);
      return result;
    });
    // The backend records the remote in settings.
    [this.settings, this.auth] = await Promise.all([this.api.getSettings(), this.api.authStatus()]);
    return result;
  }

  async disconnect(signOut: boolean): Promise<void> {
    try {
      this.#setSync(await this.api.githubDisconnect(signOut));
      [this.settings, this.auth] = await Promise.all([this.api.getSettings(), this.api.authStatus()]);
      this.syncError = null;
      this.notices.dismiss("needs-pull");
    } catch (e) {
      this.#report(e);
    }
  }

  // Settings

  saveSettings(patch: Partial<Settings>): Promise<void> {
    const before = this.settings;
    const next = { ...before, ...patch };
    this.settings = next;
    this.#settingsWrites = this.#settingsWrites.then(async () => {
      try {
        const saved = await this.api.setSettings(next);
        if (this.settings === next) this.settings = saved;
      } catch (e) {
        if (this.settings === next) this.settings = before;
        this.#report(e);
      }
      this.#armFetchTimer();
    });
    return this.#settingsWrites;
  }

  /** Closes the open sheet, first committing an edit in its focused field (its `change` event). */
  closeOverlay(): void {
    const field = document.activeElement;
    if (field instanceof HTMLElement && field.closest("dialog")) field.blur();
    this.overlay = null;
  }

  // Keyboard

  perform(action: Action): void {
    // With a sheet open, only the shortcuts that toggle overlays do anything.
    if (this.overlay && action !== "quickOpen" && action !== "settings") return;
    switch (action) {
      case "new":
        this.sidebarOpen = true;
        this.newPrompt = true;
        break;
      case "toggleMode":
        if (this.current.file !== null) this.mode = this.mode === "source" ? "styled" : "source";
        break;
      case "quickOpen":
      case "settings":
        if (this.overlay === action) this.closeOverlay();
        else if (this.overlay === null) this.overlay = action;
        break;
      case "search":
        this.sidebarOpen = true;
        this.search.setMode("text");
        this.focusSearch = true;
        break;
      case "pull":
        void this.pull();
        break;
      case "commit":
        void this.openCommit();
        break;
      case "toggleSidebar":
        if (!this.mobile) this.sidebarOpen = !this.sidebarOpen;
        break;
    }
  }

  showError(e: unknown): void {
    this.#report(e);
  }

  // Internals

  /** Runs `fn` after every earlier working-tree operation has finished. */
  #serial<T>(fn: () => Promise<T>): Promise<T> {
    const run = this.#lock.then(fn);
    this.#lock = run.catch(() => {});
    return run;
  }

  /**
   * A pull, commit or connect: after earlier operations and any fetch in flight (which holds the
   * backend's git lock, so autosave isn't paused while waiting for it), with autosave paused.
   */
  #gitWork<T>(fn: () => Promise<T>): Promise<T> {
    return this.#serial(async () => {
      await this.#fetching;
      return this.#withSavesPaused(fn);
    });
  }

  /** Writes pending edits, then runs `fn` with autosave paused so no write races the git work. */
  async #withSavesPaused<T>(fn: () => Promise<T>): Promise<T> {
    await this.current.flush();
    this.current.pause();
    try {
      return await fn();
    } finally {
      this.current.resume();
    }
  }

  /** Marks `op` busy while `fn` runs; returns its error instead of throwing. */
  async #track(op: SyncOp, fn: () => Promise<void>): Promise<ApiError | null> {
    this.ops.push(op);
    try {
      await fn();
      this.syncError = null;
      this.notices.dismiss("sync-error");
      return null;
    } catch (e) {
      const error = toApiError(e);
      // A failed save is not a sync failure; it is reported on its own.
      if (!(e instanceof SaveError)) this.syncError = error;
      return error;
    } finally {
      this.ops.splice(this.ops.indexOf(op), 1);
    }
  }

  #setSync(view: SyncView): void {
    this.#syncReports++;
    this.sync = view;
  }

  /** Reads the sync state; false if a git operation reported it meanwhile, which then stands. */
  async #readSync(): Promise<boolean> {
    const before = this.#syncReports;
    const view = await this.api.syncState();
    if (this.#syncReports !== before) return false;
    this.#setSync(view);
    return true;
  }

  /** After every successful save. A save can create the file, so the list may need it. */
  #saved(): void {
    this.notices.dismiss("save-error");
    if (this.currentMeta) this.refreshSync();
    else void this.refresh();
  }

  /** Runs with autosave paused; the open entry is reloaded, or kept next to the pulled version. */
  async #afterIncoming({ updated, merged, conflicts }: PullResult): Promise<void> {
    await this.refreshEntries();
    const file = this.current.file;
    const touched =
      file !== null && (updated.includes(file) || merged.includes(file) || conflicts.some((c) => c.path === file));
    if (touched) {
      const exists = this.entries.some((e) => e.file === file);
      if (!this.current.dirty) {
        if (exists) await this.current.reloadIfClean();
        else this.current.close();
      } else if (exists) {
        // Typed during the pull. As in any conflict, GitHub's version keeps the name and the local
        // one goes next to it: the buffer moves to a copy, which autosave writes when the pause ends.
        const names = new Set(this.entries.map((e) => e.file));
        const copy = conflictCopyName(file, (name) => names.has(name), new Date());
        this.current.renamed(copy);
        this.notices.show({
          key: "kept-buffer",
          tone: "info",
          text: `This entry changed on GitHub while you were typing. GitHub's version keeps the name; what you have open is saved next to it as ${copy}.`,
          files: [{ label: `${file} (GitHub's version)`, file }],
        });
      } else {
        // Removed on GitHub. A changed file is kept, so autosave writes the buffer back.
        this.notices.show({
          key: "kept-buffer",
          tone: "info",
          text: `${file} was removed on GitHub while you were typing. Your text is kept and saved under its name again.`,
        });
      }
    }
    if (conflicts.length) this.notices.show(conflictNotice(conflicts));
  }

  #armFetchTimer(): void {
    clearInterval(this.#fetchTimer);
    const minutes = this.settings.fetchIntervalMin;
    if (minutes > 0) this.#fetchTimer = setInterval(() => void this.fetch(), minutes * 60_000);
  }

  #toast(text: string): void {
    this.notices.show({ key: "toast", tone: "info", text }, TOAST_MS);
  }

  #report(e: unknown): void {
    const error = toApiError(e);
    if (error.kind === "reauth") {
      this.notices.show({
        key: "reauth",
        tone: "error",
        text: "Your GitHub sign-in has expired. Sign in again to keep syncing.",
        action: { label: "Settings", run: () => (this.overlay = "settings") },
      });
    } else {
      // A failed save replaces the entry's own notice about it, which a later save dismisses.
      this.notices.show({ key: e instanceof SaveError ? "save-error" : undefined, tone: "error", text: error.message });
    }
  }
}

/** Entries in the list link to them; other files (images) are only named. */
function conflictNotice(conflicts: Conflict[]): Omit<Notice, "id"> {
  const named = (path: string, label = path) => ({ label, file: isEntryFile(path) ? path : undefined });
  return {
    key: "conflicts",
    tone: "info",
    text: `${plural(conflicts.length, "file")} changed on both sides. GitHub's version keeps the name; yours is saved next to it, so nothing was lost:`,
    files: conflicts.map((c) =>
      c.copy ? named(c.copy) : named(c.path, `${c.path} (deleted here, kept GitHub's version)`),
    ),
  };
}
