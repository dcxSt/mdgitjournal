// App-wide state and the orchestration the frontend owns (docs/architecture.md, "Frontend owns
// timing"): fetch on launch, focus and an interval; flush autosave before git work; reload the open
// entry after a pull. Operations that touch the working tree run one at a time.

import { imageLink } from "../editor/commands";
import type { MergeSnapshot } from "../editor/merge";
import { ApiError, onHidden, toApiError, type Api, type Unlisten } from "./api";
import { DEFAULT_SETTINGS, EMPTY_VIEW, SIGNED_OUT } from "./defaults";
import { CurrentEntry, SaveError } from "./entry.svelte";
import { isEntryFile } from "./journal";
import { Notices } from "./notices.svelte";
import { Review } from "./review.svelte";
import { Search } from "./search.svelte";
import type { Action } from "./shortcuts";
import {
  changedPaths,
  commitButton,
  commitMessageValid,
  isPushOnly,
  isSyncFailure,
  MERGE_TITLE,
  plural,
  summaryText,
  type SyncOp,
} from "./sync";
import type {
  AppInfo,
  AuthStatus,
  CommandError,
  EntryMeta,
  FileChange,
  MergeState,
  Mode,
  Platform,
  PullPreview,
  PullResult,
  Settings,
  SyncSummary,
  SyncView,
} from "./types";

/**
 * The sheet (or full-window view) in front of everything. "merge" is the merge view, on top of
 * the review it belongs to; "summary" is a pull's details; "abortMerge" confirms Abort merge.
 */
export type Overlay = "commit" | "settings" | "quickOpen" | "delete" | "review" | "merge" | "summary" | "abortMerge";

/** Below this width a browser preview uses the phone layout. */
export const MOBILE_MAX_WIDTH = 700;
const FOCUS_REFRESH_GAP_MS = 2000;
const TOAST_MS = 3000;
/** While a merge is in progress, how often to look whether it was finished in a terminal. */
export const MERGE_POLL_MS = 5000;

export const NEEDS_PULL_TEXT = "GitHub has new changes — Pull first. Your commit is saved locally.";
export const STALE_TEXT = "Something changed since you opened the review.";
export const MERGE_TEXT = "Merge in progress — finish it with git, or abort it.";

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
  /** The editor takes no edits: git is rewriting the working tree. */
  readOnly = $state(false);
  /** The Sync review in progress (overlay "review", or "merge" on top of it). */
  review = $state.raw<Review | null>(null);
  /** The pull the summary notice and its Details sheet describe. */
  summary = $state.raw<SyncSummary | null>(null);

  readonly current: CurrentEntry;
  readonly search: Search;
  readonly notices = new Notices();

  mobile = $derived.by(() => this.info !== null && mobileLayout(this.info.platform, this.width, this.api.native));
  dark = $derived(this.settings.theme === "dark" || (this.settings.theme === "system" && this.systemDark));
  currentMeta = $derived(this.entries.find((e) => e.file === this.current.file) ?? null);
  lastError = $derived(this.syncError ?? this.sync.error);
  mac = $derived(this.info?.platform === "macos" || this.info?.platform === "ios");
  /** Undo can still reverse the pull the summary describes. */
  canUndo = $derived(this.summary !== null && this.sync.lastSync?.at === this.summary.at);

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
  #mergePoll: ReturnType<typeof setInterval> | undefined;
  /** Abort merge is running: the merge ending is no news. */
  #aborting = false;
  /** Nested working-tree operations holding the editor read-only. */
  #writing = 0;

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
    clearInterval(this.#mergePoll);
    this.#mergePoll = undefined;
  }

  // Entries

  async refreshEntries(): Promise<void> {
    this.entries = await this.api.listEntries();
  }

  /**
   * Re-reads the list and the open entry (if its buffer is clean) after changes on disk. While
   * git rewrites the working tree, that operation takes care of the open entry itself.
   */
  async refresh(): Promise<void> {
    try {
      await this.refreshEntries();
      if (this.#writing === 0) await this.#followOpenEntry();
      this.refreshSync();
    } catch (e) {
      this.#report(e);
    }
  }

  /** The open entry after changes on disk: reloaded if its buffer is clean, closed if it is gone. */
  async #followOpenEntry(): Promise<void> {
    const file = this.current.file;
    if (file !== null && !this.entries.some((e) => e.file === file)) {
      // Gone from disk. Unsaved text stays and autosave writes it back.
      if (!this.current.dirty) this.current.close();
    } else {
      await this.current.reloadIfClean();
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

  /** The review or the merge view is open; a fetch now would only make it stale. */
  get reviewing(): boolean {
    return this.overlay === "review" || this.overlay === "merge";
  }

  async fetch(userInitiated = false): Promise<void> {
    if (!this.sync.connected || this.ops.length || this.reviewing) return;
    const fetched = this.#track("fetch", async () => {
      this.#setSync(await this.api.fetch());
    });
    this.#fetching = fetched;
    const error = await fetched;
    if (error && userInitiated) this.#report(error);
  }

  /**
   * Pull: applied at once when only one side changed each file (then the summary notice, with
   * Undo), otherwise the Sync review opens and nothing has changed on disk.
   */
  async pull(): Promise<void> {
    if (!this.sync.connected || this.ops.includes("pull") || this.ops.includes("apply") || this.reviewing) return;
    if (this.sync.merge) {
      this.#toast(`${MERGE_TITLE}.`);
      return;
    }
    let result: PullResult | undefined;
    const error = await this.#track("pull", () =>
      this.#writeWork(async () => {
        result = await this.api.pull();
        this.#setSync(result.view);
        this.notices.dismiss("needs-pull");
        if (result.status === "applied") await this.#afterWrite(result.summary.files);
      }),
    );
    if (!result) {
      this.#report(error);
      return;
    }
    if (result.status === "up_to_date") this.#toast("Up to date.");
    else if (result.status === "applied") this.#showSummary(result.summary);
    else this.openReview(result.preview);
  }

  /** Opens the Sync review of `preview`; `notice` says why it opened (connect, a stale review). */
  openReview(preview: PullPreview, notice: string | null = null): void {
    const review = new Review(preview);
    review.notice = notice;
    this.review = review;
    this.overlay = "review";
  }

  /** Cancel: nothing was written, so there is nothing to tell the backend. */
  cancelReview(): void {
    if (this.review?.busy) return;
    this.review = null;
    if (this.reviewing) this.overlay = null;
  }

  openMerge(path: string): void {
    const review = this.review;
    if (!review?.overlap(path)?.suggestion || review.busy) return;
    review.merging = path;
    this.overlay = "merge";
  }

  /** Done in the merge view (with its text), or Cancel (without): back to the review. */
  closeMerge(done?: { text: string; snapshot: MergeSnapshot }): void {
    const review = this.review;
    if (!review) return;
    if (done && review.merging) review.merged(review.merging, done.text, done.snapshot);
    review.merging = null;
    this.overlay = "review";
  }

  /** Apply: writes the review's choices. A stale review is replaced by a fresh one. */
  async applyReview(): Promise<void> {
    const review = this.review;
    if (!review || review.busy || !review.ready) return;
    review.busy = true;
    review.error = null;
    let summary: SyncSummary | undefined;
    const error = await this.#track("apply", () =>
      this.#writeWork(async () => {
        const result = await this.api.applyPull(review.preview.id, review.resolutions(), review.commitMessage());
        this.#setSync(result.view);
        summary = result.summary;
        this.#closeReview(review);
        await this.#afterWrite(result.summary.files);
      }),
    );
    review.busy = false;
    if (summary) this.#showSummary(summary);
    else if (error?.kind === "stale_review") await this.#freshReview();
    else if (error) review.error = error.message;
  }

  /**
   * Resolve with git: leaves a standard git merge in progress. Uncommitted changes are committed
   * first, with the message the review asked for.
   */
  async resolveWithGit(): Promise<void> {
    const review = this.review;
    if (!review || review.busy) return;
    const needsMessage = this.sync.changes.length > 0;
    if (needsMessage && !commitMessageValid(review.gitMessage)) {
      review.error = "Write a commit message for your changes first.";
      return;
    }
    review.busy = true;
    review.error = null;
    let started = false;
    const error = await this.#track("apply", () =>
      this.#writeWork(async () => {
        const view = await this.api.startExternalMerge(review.preview.id, needsMessage ? review.gitMessage.trim() : null);
        this.#setSync(view);
        started = true;
        this.#closeReview(review);
        await this.#afterWrite(null);
      }),
    );
    review.busy = false;
    if (started) return;
    if (error?.kind === "stale_review") {
      await this.#freshReview();
      return;
    }
    // There were changes the review didn't know of: the message box appears.
    if (error?.kind === "empty_message") this.refreshSync();
    review.error = error?.message ?? null;
  }

  /** Undo of the last pull. Returns why it couldn't, for whoever asked to show. */
  async undoSync(): Promise<ApiError | null> {
    let restored = false;
    const error = await this.#track("apply", () =>
      this.#writeWork(async () => {
        const result = await this.api.undoSync();
        this.#setSync(result.view);
        restored = true;
        await this.#afterWrite(result.restored);
      }),
    );
    if (!restored) return error;
    this.#dropSummary();
    this.#toast("Pull undone.");
    return null;
  }

  openSummary(): void {
    if (this.summary && this.overlay === null) this.overlay = "summary";
  }

  /** Abort merge (after its confirmation): back to how things were before the merge. */
  async abortMerge(): Promise<ApiError | null> {
    this.#aborting = true;
    try {
      return await this.#track("apply", () =>
        this.#writeWork(async () => {
          const result = await this.api.abortMerge();
          this.#setSync(result.view);
          await this.#afterWrite(result.restored);
        }),
      );
    } finally {
      this.#aborting = false;
    }
  }

  /** Opens the commit sheet with an up-to-date list of changes. */
  async openCommit(): Promise<void> {
    if (this.overlay) return;
    if (this.sync.merge) {
      this.#toast(`${MERGE_TITLE}.`);
      return;
    }
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
    if (commitButton(this.sync, this.ops).disabled) this.#toast(this.sync.merge ? `${MERGE_TITLE}.` : "Nothing to commit.");
    else this.overlay = "commit";
  }

  /**
   * Commits the ticked `paths` (and pushes, when connected). Returns an error for the sheet to
   * show, or null. The message may only be blank, with nothing ticked, when there is nothing
   * to commit and local commits to push.
   */
  async commit(message: string, paths: string[]): Promise<ApiError | null> {
    const pushOnly = isPushOnly(this.sync) && paths.length === 0;
    if (!pushOnly && !commitMessageValid(message)) return new ApiError("empty_message", "Write a commit message.");
    if (!pushOnly && paths.length === 0) return new ApiError("invalid", "Tick at least one file to commit.");
    const text = message.trim();
    const error = await this.#track("commit", () =>
      this.#gitWork(async () => {
        if (!this.sync.connected) {
          this.#setSync(await this.api.commit(text, paths));
          this.#toast("Committed.");
          return;
        }
        const { view, result } = await this.api.commitAndPush(text, paths);
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
        actions: [{ label: "Close anyway", run: () => void this.api.closeWindow() }],
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

  /**
   * Connects the journal to `owner/repo`; errors are thrown for the connect flow to show. If files
   * differ on both sides, the Sync review opens in front of Settings.
   */
  async connect(fullName: string): Promise<PullResult> {
    const result = await this.#writeWork(async () => {
      const result = await this.api.githubConnect(fullName);
      this.#setSync(result.view);
      this.syncError = null;
      if (result.status === "applied") await this.#afterWrite(result.summary.files);
      return result;
    });
    // The backend records the remote in settings.
    [this.settings, this.auth] = await Promise.all([this.api.getSettings(), this.api.authStatus()]);
    if (result.status === "applied") this.#showSummary(result.summary);
    else if (result.status === "review") {
      this.openReview(result.preview, `Connected to ${fullName}. Some files differ here and on GitHub: choose what to keep.`);
    }
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
   * A commit: after earlier operations and any fetch in flight (which holds the backend's git
   * lock, so autosave isn't paused while waiting for it), with autosave paused.
   */
  #gitWork<T>(fn: () => Promise<T>): Promise<T> {
    return this.#serial(async () => {
      await this.#fetching;
      return this.#withSavesPaused(fn);
    });
  }

  /**
   * Git work that rewrites the working tree (pull, apply, undo, Resolve with git, abort,
   * connect): as `#gitWork`, and the editor is read-only from before the last save until it is
   * done, so the buffer can't drift from a file the operation replaces.
   */
  #writeWork<T>(fn: () => Promise<T>): Promise<T> {
    return this.#serial(async () => {
      await this.#fetching;
      this.#writing++;
      this.readOnly = true;
      try {
        return await this.#withSavesPaused(fn);
      } finally {
        if (--this.#writing === 0) this.readOnly = false;
      }
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
      // A failed save is not a sync failure, and neither is an answer such as a stale review.
      if (!(e instanceof SaveError) && isSyncFailure(error.kind)) this.syncError = error;
      return error;
    } finally {
      this.ops.splice(this.ops.indexOf(op), 1);
    }
  }

  #setSync(view: SyncView): void {
    const before = this.sync;
    this.#syncReports++;
    this.sync = view;
    this.#followMerge(before.merge, view.merge);
    // The summary's pull can't be undone any more (a commit, a push, another pull): it goes.
    const shown = this.summary?.at;
    if (shown !== undefined && before.lastSync?.at === shown && view.lastSync?.at !== shown) this.#dropSummary();
  }

  /** The merge notice follows the merge; finishing it in a terminal is noticed and said. */
  #followMerge(before: MergeState | null, after: MergeState | null): void {
    if (after) {
      const same = before && before.conflicted.join("\0") === after.conflicted.join("\0");
      if (!same || !this.notices.has("merge")) this.notices.update(this.#mergeNotice(after));
      this.#mergePoll ??= setInterval(() => this.refreshSync(), MERGE_POLL_MS);
      return;
    }
    clearInterval(this.#mergePoll);
    this.#mergePoll = undefined;
    if (!before) return;
    this.notices.dismiss("merge");
    if (this.overlay === "abortMerge") this.overlay = null;
    if (this.#aborting) return;
    this.#toast("Merge finished.");
    // git changed files: the list and the open entry follow.
    void this.refresh();
  }

  #mergeNotice(merge: MergeState) {
    return {
      key: "merge",
      tone: "info" as const,
      persistent: true,
      text: merge.conflicted.length
        ? `${MERGE_TEXT} ${merge.conflicted.length === 1 ? "This file has" : "These files have"} conflict markers:`
        : `${MERGE_TEXT} No conflicts are left: commit with git to finish.`,
      files: merge.conflicted.map((path) => ({ label: path, file: isEntryFile(path) ? path : undefined })),
      actions: [{ label: "Abort merge", keep: true, run: () => this.#openOverlay("abortMerge") }],
    };
  }

  #showSummary(summary: SyncSummary): void {
    this.summary = summary;
    const undoable = this.sync.lastSync?.at === summary.at;
    this.notices.show({
      key: "sync-summary",
      tone: "info",
      text: `${summaryText(summary)}.`,
      actions: [
        { label: "Details", keep: true, run: () => this.openSummary() },
        ...(undoable ? [{ label: "Undo", keep: true, run: () => void this.#undoFromNotice() }] : []),
      ],
    });
  }

  async #undoFromNotice(): Promise<void> {
    const error = await this.undoSync();
    if (error) this.notices.show({ key: "undo-error", tone: "error", text: error.message });
  }

  #dropSummary(): void {
    this.summary = null;
    this.notices.dismiss("sync-summary");
    this.notices.dismiss("undo-error");
    if (this.overlay === "summary") this.overlay = null;
  }

  #openOverlay(overlay: Overlay): void {
    if (this.overlay === null) this.overlay = overlay;
  }

  #closeReview(review: Review): void {
    if (this.review !== review) return;
    this.review = null;
    if (this.reviewing) this.overlay = null;
  }

  /** After `stale_review`: a fresh review, from a preview that never applies anything. */
  async #freshReview(): Promise<void> {
    try {
      const result = await this.#serial(async () => {
        await this.current.flush();
        return this.api.pullPreview();
      });
      this.#setSync(result.view);
      if (result.status === "review") {
        this.openReview(result.preview, STALE_TEXT);
        return;
      }
      this.review = null;
      if (this.reviewing) this.overlay = null;
      this.#toast(`${STALE_TEXT} There is nothing to pull now.`);
    } catch (e) {
      if (this.review) this.review.error = toApiError(e).message;
      else this.#report(e);
    }
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

  /**
   * After git rewrote files (`changes`, or any file when null): re-lists entries and reloads the
   * open one if it changed, follows it if renamed, or closes it if it is gone. Runs with autosave
   * paused and the editor read-only, so the buffer is what is on disk.
   */
  async #afterWrite(changes: readonly FileChange[] | readonly string[] | null): Promise<void> {
    await this.refreshEntries();
    const file = this.current.file;
    if (file === null) return;
    const paths = changes?.flatMap((c: FileChange | string) => (typeof c === "string" ? [c] : changedPaths([c])));
    if (paths && !paths.includes(file)) return;
    if (this.entries.some((e) => e.file === file)) {
      await this.current.reloadIfClean();
      return;
    }
    if (this.current.dirty) return;
    const renamed = changes?.find((c: FileChange | string): c is FileChange => typeof c !== "string" && c.oldPath === file)?.path;
    if (renamed && this.entries.some((e) => e.file === renamed)) this.current.load(await this.api.readEntry(renamed));
    else this.current.close();
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
    // Someone started a merge with the git CLI: the state shows it.
    if (error.kind === "merge_in_progress") this.refreshSync();
    if (error.kind === "reauth") {
      this.notices.show({
        key: "reauth",
        tone: "error",
        text: "Your GitHub sign-in has expired. Sign in again to keep syncing.",
        actions: [{ label: "Settings", run: () => (this.overlay = "settings") }],
      });
    } else {
      // A failed save replaces the entry's own notice about it, which a later save dismisses.
      this.notices.show({ key: e instanceof SaveError ? "save-error" : undefined, tone: "error", text: error.message });
    }
  }
}
