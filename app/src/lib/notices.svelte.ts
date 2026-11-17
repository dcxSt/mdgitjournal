// Banners and short-lived toasts.

export interface Notice {
  id: number;
  /** Notices with a key replace an earlier notice with the same key. */
  key?: string;
  tone: "info" | "error";
  text: string;
  /** Entries the notice links to, e.g. conflict copies. */
  files?: { file: string; label: string }[];
  action?: { label: string; run: () => void };
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

  /** Removes the notice with this id, or every notice with this key. */
  dismiss(idOrKey: number | string): void {
    this.list = this.list.filter((n) => n.id !== idOrKey && n.key !== idOrKey);
  }
}
