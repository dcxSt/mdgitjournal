// The open entry: its buffer, autosave (400 ms after the last edit) and reloads from disk.
// Writes are queued so they land in order; while paused (during sync ops) nothing is written.

import type { Api } from "./api";
import { wordCount } from "./journal";
import type { EntryContent } from "./types";

export const AUTOSAVE_MS = 400;
const RETRY_MS = 3000;
const WORDS_MS = 250;
const PAUSED_TEXT = "Your latest edits will be saved when the sync in progress finishes.";
/** Half of a UTF-16 surrogate pair: no file can hold it, so it is never sent to be saved. */
const LONE_SURROGATE = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/;

export class SaveError extends Error {
  override name = "SaveError";
}

export class CurrentEntry {
  file = $state<string | null>(null);
  /** The editor buffer, exactly as typed. */
  content = $state("");
  /** The buffer has edits that are not on disk yet. */
  dirty = $state(false);
  saving = $state(false);
  saveError = $state<string | null>(null);
  /** Bumped whenever `content` is replaced from disk, so the editor knows to reset. */
  version = $state(0);
  /** The last replacement was the same file again (reloaded after it changed on disk). */
  reloaded = false;
  words = $state(0);

  #hash: string | null = null;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #wordTimer: ReturnType<typeof setTimeout> | undefined;
  #writes: Promise<void> = Promise.resolve();
  #paused = 0;

  constructor(
    private readonly api: Api,
    private readonly onSaved: () => void = () => {},
    /**
     * The first failure after a successful save; the retries that follow don't call it again.
     * `retrying` is false when only an edit can fix it.
     */
    private readonly onSaveFailed: (file: string, message: string, retrying: boolean) => void = () => {},
  ) {}

  load(entry: EntryContent): void {
    this.#cancelTimers();
    this.reloaded = entry.file === this.file;
    this.file = entry.file;
    this.content = entry.content;
    this.#hash = entry.hash;
    this.dirty = false;
    this.saveError = null;
    this.words = wordCount(entry.content);
    this.version++;
  }

  /** Forgets the entry, including unsaved edits. Callers flush first unless deleting. */
  close(): void {
    this.#cancelTimers();
    this.reloaded = false;
    this.file = null;
    this.content = "";
    this.#hash = null;
    this.dirty = false;
    this.saveError = null;
    this.words = 0;
    this.version++;
  }

  edit(text: string): void {
    if (this.file === null || text === this.content) return;
    this.content = text;
    this.dirty = true;
    this.#schedule(AUTOSAVE_MS);
    clearTimeout(this.#wordTimer);
    this.#wordTimer = setTimeout(() => (this.words = wordCount(this.content)), WORDS_MS);
  }

  /** The file was renamed on disk; the buffer and hash stay valid. */
  renamed(file: string): void {
    this.file = file;
  }

  /** Writes pending edits now. Throws if they could not be saved. */
  async flush(): Promise<void> {
    clearTimeout(this.#timer);
    await this.#enqueue();
    if (this.dirty && this.saveError) {
      throw new SaveError(`Couldn't save ${this.file}: ${this.saveError}`);
    }
  }

  /**
   * Like `flush`, but also writes edits typed while a write was in flight. Throws unless the
   * buffer is on disk afterwards, which includes while a sync holds autosave back.
   */
  async settle(): Promise<void> {
    do await this.flush();
    while (this.dirty && !this.#paused);
    if (this.dirty) throw new SaveError(PAUSED_TEXT);
  }

  /** Stops autosave until `resume`; edits keep accumulating in the buffer. */
  pause(): void {
    this.#paused++;
  }

  /** Edits held back by the pause are written at once. */
  resume(): void {
    if (--this.#paused > 0 || !this.dirty) return;
    clearTimeout(this.#timer);
    void this.#enqueue();
  }

  /** Reloads from disk if the file changed and the buffer has no unsaved edits. */
  async reloadIfClean(): Promise<void> {
    const file = this.file;
    if (file === null || this.dirty) return;
    const entry = await this.api.readEntry(file);
    if (this.file === file && !this.dirty && entry.hash !== this.#hash) this.load(entry);
  }

  #schedule(ms: number): void {
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => void this.#enqueue(), ms);
  }

  #cancelTimers(): void {
    clearTimeout(this.#timer);
    clearTimeout(this.#wordTimer);
  }

  #enqueue(): Promise<void> {
    this.#writes = this.#writes.then(() => this.#write());
    return this.#writes;
  }

  async #write(): Promise<void> {
    const file = this.file;
    if (this.#paused || !this.dirty || file === null) return;
    const text = this.content;
    const broken = text.search(LONE_SURROGATE);
    if (broken >= 0) {
      const line = text.slice(0, broken).split("\n").length;
      this.#failed(file, `line ${line} has half of a character (such as an emoji); delete it or undo to save`, false);
      return;
    }
    this.saving = true;
    try {
      const { hash } = await this.api.writeEntry(file, text);
      if (this.file === file) {
        this.#hash = hash;
        if (this.content === text) this.dirty = false;
      }
      this.saveError = null;
      this.onSaved();
    } catch (e) {
      this.#failed(file, e instanceof Error ? e.message : String(e), true);
    } finally {
      this.saving = false;
    }
  }

  #failed(file: string, message: string, retry: boolean): void {
    const first = this.saveError === null;
    this.saveError = message;
    if (first) this.onSaveFailed(file, message, retry);
    if (retry && this.file === file) this.#schedule(RETRY_MS);
  }
}
