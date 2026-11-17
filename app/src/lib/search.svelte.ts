// The sidebar search: a filename filter applied as you type, or full-text search in the backend.

import { toApiError, type Api } from "./api";
import type { SearchHit } from "./types";

export type SearchMode = "name" | "text";

export const SEARCH_DEBOUNCE_MS = 150;

export class Search {
  query = $state("");
  mode = $state<SearchMode>("name");
  hits = $state.raw<SearchHit[]>([]);
  pending = $state(false);
  error = $state<string | null>(null);

  #timer: ReturnType<typeof setTimeout> | undefined;
  /** Identifies the latest request, so slower earlier answers are dropped. */
  #seq = 0;

  constructor(private readonly api: Api) {}

  setQuery(query: string): void {
    this.query = query;
    if (this.mode === "text") this.#schedule(SEARCH_DEBOUNCE_MS);
  }

  setMode(mode: SearchMode): void {
    if (mode === this.mode) return;
    this.mode = mode;
    if (mode === "text") this.#schedule(0);
    else this.#reset();
  }

  clear(): void {
    this.query = "";
    this.mode = "name";
    this.#reset();
  }

  #reset(): void {
    clearTimeout(this.#timer);
    this.#seq++;
    this.hits = [];
    this.pending = false;
    this.error = null;
  }

  #schedule(ms: number): void {
    clearTimeout(this.#timer);
    const seq = ++this.#seq;
    const query = this.query.trim();
    if (!query) {
      this.hits = [];
      this.pending = false;
      this.error = null;
      return;
    }
    this.pending = true;
    this.#timer = setTimeout(() => void this.#run(seq, query), ms);
  }

  async #run(seq: number, query: string): Promise<void> {
    try {
      const hits = await this.api.search(query);
      if (seq !== this.#seq) return;
      this.hits = hits;
      this.error = null;
    } catch (e) {
      if (seq !== this.#seq) return;
      this.hits = [];
      this.error = toApiError(e).message;
    }
    this.pending = false;
  }
}
