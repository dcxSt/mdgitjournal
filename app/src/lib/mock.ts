// In-memory backend for `pnpm dev` in a plain browser and for tests. It models just enough of
// the backend (changes, ahead/behind, needs-pull, keep-both conflicts, device flow) to drive the UI.
// In the browser it is exposed as `window.mdgjMock`, e.g. `mdgjMock.remoteCommit(file, text)`.

import { ApiError, onVisible, type Api, type Unlisten } from "./api";
import { DEFAULT_SETTINGS, SIGNED_OUT } from "./defaults";
import { slugPreview, wordCount } from "./journal";
import type {
  AuthStatus,
  Conflict,
  EntryMeta,
  ErrorKind,
  FileChange,
  Platform,
  PullResult,
  SearchHit,
  Settings,
  SyncView,
} from "./types";

export interface MockOptions {
  /** Delay per call in ms; network calls take four times as long. */
  latencyMs?: number;
  /** Seed sample entries, and sample GitHub activity after connecting. */
  samples?: boolean;
  platform?: Platform;
  clientIdPlaceholder?: boolean;
  now?: () => Date;
}

export interface MockControls {
  /** Working tree: path → content. Images are stored under "assets/" as object URLs. */
  readonly files: Map<string, string>;
  /** Method names in call order. */
  readonly calls: string[];
  /** A commit lands on GitHub; it counts as `behind` after the next fetch. */
  remoteCommit(file: string, content: string): void;
  /** Makes the next call of `method` fail with this error. */
  failNext(method: keyof Api, kind: ErrorKind, message: string): void;
}

export type MockApi = Api & { mock: MockControls };

const JOURNAL_DIR = "/mock/journal";
const ENTRY_NAME = /^(\d{4}-\d{2}-\d{2})-(\d{2})(\d{2})-(.+)\.md$/;

const SAMPLES: Record<string, string> = {
  "2026-11-16-1402-morning-run.md":
    "# Morning run\n\nFive miles along the river. Cold and bright, first frost on the path.\n\n- [x] stretch\n- [ ] new shoes\n",
  "2026-11-14-2130-ideas.md":
    "# Ideas\n\nA journal that is just a folder of markdown files, kept in git.\n\n**Bold** thoughts and _quiet_ ones.\n",
  "2026-10-28-0815-reading-list.md": "# Reading list\n\n- *The Peregrine*, J. A. Baker\n- *Walden*\n",
  "notes.md": "Loose notes that don't follow the naming scheme.\n",
};

function detectPlatform(): Platform {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/iPhone|iPad|iPod/.test(ua)) return "ios";
  return /Mac/.test(ua) ? "macos" : "linux";
}

const pad = (n: number) => String(n).padStart(2, "0");

/** FNV-1a over UTF-16 units; the real backend's hash is opaque to the UI anyway. */
function hash(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193);
  return (h >>> 0).toString(16).padStart(8, "0");
}

function validName(file: string): boolean {
  return !file.includes("/") && !file.includes("..") && file.endsWith(".md");
}

export function createMockApi(options: MockOptions = {}): MockApi {
  const { latencyMs = 0, samples = true, clientIdPlaceholder = false } = options;
  const now = options.now ?? (() => new Date());

  const files = new Map<string, string>(samples ? Object.entries(SAMPLES) : []);
  let head = new Map(files);
  const mtimes = new Map<string, number>();
  let settings: Settings = structuredClone(DEFAULT_SETTINGS);
  let auth: AuthStatus = { ...SIGNED_OUT };
  let remote: { fullName: string; incoming: { file: string; content: string }[] } | null = null;
  let seen = 0;
  let ahead = 0;
  let lastFetch: number | null = null;
  let deviceStarted = false;
  let polls = 0;

  const calls: string[] = [];
  const failures = new Map<string, ApiError>();
  const syncListeners = new Set<(view: SyncView) => void>();
  const entryListeners = new Set<() => void>();

  async function step(method: keyof Api, factor = 1): Promise<void> {
    calls.push(method);
    if (latencyMs) await new Promise((resolve) => setTimeout(resolve, latencyMs * factor));
    const failure = failures.get(method);
    if (failure) {
      failures.delete(method);
      throw failure;
    }
  }

  function changes(): FileChange[] {
    const result: FileChange[] = [];
    const deleted = [...head.keys()].filter((f) => !files.has(f));
    for (const [path, content] of files) {
      const base = head.get(path);
      if (base === undefined) {
        const from = deleted.findIndex((d) => head.get(d) === content);
        if (from >= 0) result.push({ path, kind: "renamed", oldPath: deleted.splice(from, 1)[0] });
        else result.push({ path, kind: "added" });
      } else if (base !== content) {
        result.push({ path, kind: "modified" });
      }
    }
    for (const path of deleted) result.push({ path, kind: "deleted" });
    return result.sort((a, b) => a.path.localeCompare(b.path));
  }

  function view(): SyncView {
    return {
      connected: remote !== null,
      repo: remote?.fullName ?? null,
      branch: "main",
      ahead: remote ? ahead : 0,
      behind: remote ? seen : 0,
      changes: changes(),
      lastFetch,
      error: null,
    };
  }

  function emitState(): SyncView {
    const v = view();
    for (const cb of syncListeners) cb(v);
    return v;
  }

  function meta(file: string): EntryMeta {
    const m = ENTRY_NAME.exec(file);
    const content = files.get(file) ?? "";
    return {
      file,
      date: m?.[1] ?? null,
      time: m ? `${m[2]}:${m[3]}` : null,
      title: m?.[4] ?? file.replace(/\.md$/, ""),
      mtime: mtimes.get(file) ?? 0,
      size: new TextEncoder().encode(content).length,
      words: wordCount(content),
    };
  }

  function entryFiles(): string[] {
    const md = [...files.keys()].filter((f) => validName(f) && !f.startsWith("."));
    const dated = md.filter((f) => ENTRY_NAME.test(f)).sort((a, b) => b.localeCompare(a));
    const other = md.filter((f) => !ENTRY_NAME.test(f)).sort();
    return [...dated, ...other];
  }

  function existing(file: string): string {
    if (!validName(file)) throw new ApiError("invalid", `“${file}” is not an entry name.`);
    const content = files.get(file);
    if (content === undefined) throw new ApiError("io", `${file} does not exist.`);
    return content;
  }

  function dedupe(stem: string, ext: string): string {
    let name = `${stem}${ext}`;
    for (let n = 2; files.has(name); n++) name = `${stem}-${n}${ext}`;
    return name;
  }

  function write(file: string, content: string): void {
    files.set(file, content);
    mtimes.set(file, now().getTime());
  }

  function applyIncoming(): Omit<PullResult, "view"> {
    const updated: string[] = [];
    const conflicts: Conflict[] = [];
    const stamp = now();
    for (const { file, content } of remote?.incoming ?? []) {
      const local = files.get(file);
      const base = head.get(file);
      if (local === undefined && base !== undefined) {
        conflicts.push({ path: file, copy: null });
      } else if (local !== undefined && local !== base && local !== content) {
        const stem = file.replace(/\.md$/, "");
        const copy = dedupe(`${stem}.conflict-${pad(stamp.getHours())}${pad(stamp.getMinutes())}`, ".md");
        write(copy, local);
        conflicts.push({ path: file, copy });
      }
      write(file, content);
      head.set(file, content);
      updated.push(file);
    }
    if (remote) remote.incoming = [];
    seen = 0;
    lastFetch = now().getTime();
    return { updated, merged: [], conflicts };
  }

  function requireRemote(): void {
    if (!remote) throw new ApiError("not_connected", "This journal is not connected to GitHub.");
  }

  function requireMessage(message: string): void {
    if (!message.trim()) throw new ApiError("empty_message", "Write a commit message first.");
  }

  function commitAll(): void {
    head = new Map(files);
    if (remote) ahead++;
  }

  const api: MockApi = {
    native: false,

    async appInfo() {
      await step("appInfo");
      return {
        version: "0.1.0",
        platform: options.platform ?? detectPlatform(),
        journalDir: JOURNAL_DIR,
        tokenBackend: "memory (browser preview)",
        clientIdPlaceholder,
      };
    },
    async getSettings() {
      await step("getSettings");
      return structuredClone(settings);
    },
    async setSettings(next) {
      await step("setSettings");
      settings = { ...structuredClone(next), remote: settings.remote };
      return structuredClone(settings);
    },

    async listEntries() {
      await step("listEntries");
      return entryFiles().map(meta);
    },
    async readEntry(file) {
      await step("readEntry");
      const content = existing(file);
      return { file, content, hash: hash(content) };
    },
    async writeEntry(file, content) {
      await step("writeEntry");
      if (!validName(file)) throw new ApiError("invalid", `“${file}” is not an entry name.`);
      write(file, content);
      return { hash: hash(content) };
    },
    async createEntry(title) {
      await step("createEntry");
      const d = now();
      const prefix = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}-${pad(d.getHours())}${pad(d.getMinutes())}`;
      const file = dedupe(`${prefix}-${slugPreview(title) ?? "untitled"}`, ".md");
      write(file, "");
      return meta(file);
    },
    async renameEntry(file, newTitle) {
      await step("renameEntry");
      const content = existing(file);
      const m = ENTRY_NAME.exec(file);
      if (!m) throw new ApiError("invalid", "Only dated entries can be renamed.");
      const slug = slugPreview(newTitle) ?? "untitled";
      if (slug === m[4]) return meta(file);
      const next = dedupe(`${m[1]}-${m[2]}${m[3]}-${slug}`, ".md");
      files.delete(file);
      write(next, content);
      return meta(next);
    },
    async deleteEntry(file) {
      await step("deleteEntry");
      existing(file);
      files.delete(file);
      return null;
    },
    async search(query) {
      await step("search");
      const q = query.toLowerCase();
      const hits: SearchHit[] = [];
      if (!q) return hits;
      const hit = (file: string, line: number, text: string) => {
        const snippet = text.trim();
        const at = snippet.toLowerCase().indexOf(q);
        if (at < 0) return;
        const matchStart = Array.from(snippet.slice(0, at)).length;
        const matchEnd = matchStart + Array.from(snippet.slice(at, at + q.length)).length;
        hits.push({ file, title: meta(file).title, line, snippet, matchStart, matchEnd });
      };
      for (const file of entryFiles()) {
        hit(file, 0, file);
        (files.get(file) ?? "").split(/\r?\n/).forEach((text, i) => hit(file, i + 1, text));
        if (hits.length >= 200) break;
      }
      return hits.slice(0, 200);
    },
    async importImage(entryFile, bytes) {
      await step("importImage", 2);
      existing(entryFile);
      const stem = entryFile.replace(/\.md$/, "");
      let n = 1;
      while (files.has(`assets/${stem}-${n}.jpg`)) n++;
      const path = `assets/${stem}-${n}.jpg`;
      const url = typeof URL.createObjectURL === "function" ? URL.createObjectURL(new Blob([bytes])) : "";
      files.set(path, url);
      return { path, bytes: bytes.length, width: 0, height: 0 };
    },

    async syncState() {
      await step("syncState");
      return view();
    },
    async fetch() {
      await step("fetch", 4);
      if (remote) {
        seen = remote.incoming.length;
        lastFetch = now().getTime();
      }
      return emitState();
    },
    async pull() {
      await step("pull", 4);
      requireRemote();
      const outcome = applyIncoming();
      const v = emitState();
      if (outcome.updated.length) for (const cb of entryListeners) cb();
      return { view: v, ...outcome };
    },
    async commit(message) {
      await step("commit");
      requireMessage(message);
      if (changes().length) commitAll();
      return emitState();
    },
    async commitAndPush(message) {
      await step("commitAndPush", 4);
      requireRemote();
      if (changes().length) {
        requireMessage(message);
        commitAll();
      } else if (ahead === 0) {
        return { view: emitState(), result: "nothing" };
      }
      seen = remote!.incoming.length;
      lastFetch = now().getTime();
      if (seen > 0) return { view: emitState(), result: "needs_pull" };
      ahead = 0;
      return { view: emitState(), result: "pushed" };
    },

    async authStatus() {
      await step("authStatus");
      return { ...auth };
    },
    async githubDeviceStart() {
      await step("githubDeviceStart", 4);
      deviceStarted = true;
      polls = 0;
      return { userCode: "WDJB-MJHT", verificationUri: "https://github.com/login/device", expiresIn: 900, interval: 1 };
    },
    async githubDevicePoll() {
      await step("githubDevicePoll", 2);
      if (!deviceStarted) throw new ApiError("invalid", "Start signing in first.");
      if (++polls < 3) return { status: "pending" };
      deviceStarted = false;
      auth = { signedIn: true, kind: "github_app", login: "octocat" };
      return { status: "complete", login: "octocat" };
    },
    async githubSetPat(token) {
      await step("githubSetPat", 4);
      if (!token.trim()) throw new ApiError("invalid", "Paste a token first.");
      auth = { signedIn: true, kind: "pat", login: "octocat" };
      return { login: "octocat" };
    },
    async githubListRepos() {
      await step("githubListRepos", 4);
      if (!auth.signedIn) throw new ApiError("auth", "Sign in to GitHub first.");
      if (auth.kind === "pat") return [];
      return [
        {
          fullName: "octocat/journal",
          private: true,
          cloneUrl: "https://github.com/octocat/journal.git",
          defaultBranch: "main",
        },
      ];
    },
    async githubConnect(fullName) {
      await step("githubConnect", 8);
      if (!auth.signedIn) throw new ApiError("auth", "Sign in to GitHub first.");
      if (!/^[\w.-]+\/[\w.-]+$/.test(fullName)) throw new ApiError("invalid", "Use the form owner/repo.");
      remote = { fullName, incoming: [] };
      settings.remote = {
        fullName,
        url: `https://github.com/${fullName}.git`,
        branch: "main",
        auth: auth.kind ?? "github_app",
      };
      if (samples) {
        remote.incoming.push({ file: "2026-11-15-0700-from-my-phone.md", content: "Written on the phone.\n" });
      }
      const outcome = applyIncoming();
      if (samples) {
        api.mock.remoteCommit("2026-11-16-0630-dawn.md", "Up before the sun.\n");
        api.mock.remoteCommit(
          "2026-11-14-2130-ideas.md",
          `${files.get("2026-11-14-2130-ideas.md") ?? ""}\nEdited on the phone.\n`,
        );
      }
      const v = emitState();
      if (outcome.updated.length) for (const cb of entryListeners) cb();
      return { view: v, ...outcome };
    },
    async githubDisconnect(signOut) {
      await step("githubDisconnect");
      remote = null;
      settings.remote = null;
      seen = 0;
      ahead = 0;
      if (signOut) auth = { ...SIGNED_OUT };
      return emitState();
    },
    async githubUrls() {
      await step("githubUrls");
      return {
        newRepo: "https://github.com/new?name=journal&visibility=private",
        install: "https://github.com/apps/mdgitjournal/installations/new",
      };
    },

    async onSyncState(cb) {
      syncListeners.add(cb);
      return () => syncListeners.delete(cb);
    },
    async onEntriesChanged(cb) {
      entryListeners.add(cb);
      return () => entryListeners.delete(cb);
    },
    async onWindowFocus(cb): Promise<Unlisten> {
      window.addEventListener("focus", cb);
      const unlistenVisible = onVisible(cb);
      return () => {
        window.removeEventListener("focus", cb);
        unlistenVisible();
      };
    },
    async openUrl(url) {
      window.open(url, "_blank", "noopener");
    },
    async revealPath() {
      throw new ApiError("invalid", "Revealing files is not available in the browser preview.");
    },
    assetUrl(path) {
      return files.get(path.slice(JOURNAL_DIR.length + 1)) ?? "";
    },

    mock: {
      files,
      calls,
      remoteCommit(file, content) {
        if (!remote) throw new Error("mock: connect before adding remote commits");
        remote.incoming.push({ file, content });
      },
      failNext(method, kind, message) {
        failures.set(method, new ApiError(kind, message));
      },
    },
  };
  return api;
}
