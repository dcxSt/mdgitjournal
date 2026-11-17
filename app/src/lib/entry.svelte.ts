// The open entry: its buffer, autosave (400 ms after the last edit) and reloads from disk.
// Writes are queued so they land in order; while paused (during sync ops) nothing is written.

import type { Api } from "./api";
import { wordCount } from "./journal";
import type { EntryContent } from "./types";

export const AUTOSAVE_MS = 400;
const RETRY_MS = 3000;
const WORDS_MS = 250;

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
  words = $state(0);

  #hash: string | null = null;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #wordTimer: ReturnType<typeof setTimeout> | undefined;
  #writes: Promise<void> = Promise.resolve();
  #paused = 0;

  constructor(
    private readonly api: Api,
    private readonly onSaved: () => void = () => {},
  ) {}

  load(entry: EntryContent): void {
    this.#cancelTimers();
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

  /** Like `flush`, but also writes edits typed while a write was in flight. */
  async settle(): Promise<void> {
    do await this.flush();
    while (this.dirty && !this.#paused);
  }

  /** Stops autosave until `resume`; edits keep accumulating in the buffer. */
  pause(): void {
    this.#paused++;
  }

  resume(): void {
    if (--this.#paused === 0 && this.dirty) this.#schedule(AUTOSAVE_MS);
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
      this.saveError = e instanceof Error ? e.message : String(e);
      if (this.file === file) this.#schedule(RETRY_MS);
    } finally {
      this.saving = false;
    }
  }
}
