// Banners and short-lived toasts.

export interface NoticeAction {
  label: string;
  run: () => void;
  /** The notice stays after the action runs (it opens something, or may fail and be retried). */
  keep?: boolean;
}

export interface Notice {
  id: number;
  /** Notices with a key replace an earlier notice with the same key. */
  key?: string;
  tone: "info" | "error";
  text: string;
  /** Files the notice names; those with `file` set are entries it opens. */
  files?: { label: string; file?: string }[];
  /** At most two; the last is the main one. */
  actions?: NoticeAction[];
  /** No dismiss button: it goes when what it reports is over (a merge in progress). */
  persistent?: boolean;
}

export class Notices {
  list = $state.raw<Notice[]>([]);
  #next = 1;

  show(notice: Omit<Notice, "id">, timeoutMs?: number): number {
    const id = this.#next++;
    const rest = notice.key ? this.list.filter((n) => n.key !== notice.key) : this.list;
    this.list = [...rest, { ...notice, id }];
    if (timeoutMs) setTimeout(() => this.dismiss(id), timeoutMs);
    return id;
  }

  /** Replaces the notice with this key where it is, or shows it if there is none. */
  update(notice: Omit<Notice, "id"> & { key: string }): void {
    const at = this.list.findIndex((n) => n.key === notice.key);
    if (at < 0) {
      this.show(notice);
      return;
    }
    const next = [...this.list];
    next[at] = { ...notice, id: this.list[at].id };
    this.list = next;
  }

  has(key: string): boolean {
    return this.list.some((n) => n.key === key);
  }

  /** Removes the notice with this id, or every notice with this key. */
  dismiss(idOrKey: number | string): void {
    const rest = this.list.filter((n) => n.id !== idOrKey && n.key !== idOrKey);
    if (rest.length < this.list.length) this.list = rest;
  }
}
