// In-memory backend for `pnpm dev` in a plain browser and for tests. It is a small model of a git
// journal and its GitHub repository: commits with trees, HEAD, origin/main as last fetched, and
// GitHub's branch, so the sync flows of docs/architecture.md behave as the real backend's do:
// one-sided pulls applied at once with a summary and Undo, the Sync review for files changed on
// both sides (mine = the working tree), stale reviews, Resolve with git and Abort, selective commits
// and diffs. In the browser it is `window.mdgjMock`, e.g.:
//
//   mdgjMock.remoteCommit("2026-11-14-2130-ideas.md", "# Ideas\n\nFrom the phone.\n")  // lands on GitHub
//   mdgjMock.editOnDisk("2026-11-14-2130-ideas.md", "# Ideas\n\nTyped in vim.\n")      // as another program
//   mdgjMock.seedOverlaps()   // after connecting: one entry that merges cleanly, one that clashes
//   mdgjMock.finishMergeWithGit()                                                     // `git commit` in a terminal
//   mdgjMock.abortMergeWithGit()                                                      // `git merge --abort` in one

import { ApiError, onVisible, type Api, type Unlisten } from "./api";
import { DEFAULT_SETTINGS, SIGNED_OUT } from "./defaults";
import { slugPreview, wordCount } from "./journal";
import { cleanMerge, diff3, unifiedHunks, withConflictMarkers } from "./mockmerge";
import type {
  AuthStatus,
  Choice,
  CommitInfo,
  EntryMeta,
  ErrorKind,
  FileChange,
  MergeState,
  Overlap,
  OverlapKind,
  Platform,
  PullPlan,
  PullPreview,
  PullResult,
  Resolved,
  SearchHit,
  Settings,
  SyncSummary,
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
  /**
   * A commit lands on GitHub (null deletes the file); it counts as `behind` after the next fetch.
   * Also before connecting: the repository connect will use already has that history.
   */
  remoteCommit(file: string, content: string | null, message?: string): void;
  /** An edit made outside the app (null deletes); makes an open review stale. */
  editOnDisk(file: string, content: string | null): void;
  /** As `git add -A && git commit` in a terminal: finishes the merge in progress and clears `merge`. */
  finishMergeWithGit(): void;
  /** As `git merge --abort` in a terminal: the files go back, a commit Resolve with git made stays. */
  abortMergeWithGit(): void;
  /** Makes the next call of `method` fail with this error. */
  failNext(method: keyof Api, kind: ErrorKind, message: string): void;
  /**
   * Not in the contract; a shortcut for `pnpm dev` and screenshots. Once connected: GitHub edits
   * two entries and this device edits them too, uncommitted, so the next pull opens the Sync
   * review with one entry that merges cleanly (`clean`) and one whose same line clashes (`clash`).
   */
  seedOverlaps(): { clean: string; clash: string };
}

export type MockApi = Api & { mock: MockControls };

const JOURNAL_DIR = "/mock/journal";
const ENTRY_NAME = /^(\d{4}-\d{2}-\d{2})-(\d{2})(\d{2})-(.+)\.md$/;
const BRANCH = "main";
const STALE = "Something changed since you opened the review.";
const MERGING = "A merge is in progress. Finish it with git, or abort it.";
const BINARY = /\.(jpe?g|png|gif|webp|heic|avif|pdf|zip)$/i;
const MAX_LISTED = 50;

const SAMPLES: Record<string, string> = {
  "2026-11-16-1402-morning-run.md":
    "# Morning run\n\nFive miles along the river. Cold and bright, first frost on the path.\n\n- [x] stretch\n- [ ] new shoes\n",
  "2026-11-14-2130-ideas.md":
    "# Ideas\n\nA journal that is just a folder of markdown files, kept in git.\n\n**Bold** thoughts and _quiet_ ones.\n",
  "2026-10-28-0815-reading-list.md": "# Reading list\n\n- *The Peregrine*, J. A. Baker\n- *Walden*\n",
  "notes.md": "Loose notes that don't follow the naming scheme.\n",
};
const PHONE_ENTRY = "2026-11-15-0700-from-my-phone.md";

function detectPlatform(): Platform {
  const ua = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/iPhone|iPad|iPod/.test(ua)) return "ios";
  return /Mac/.test(ua) ? "macos" : "linux";
}

const pad = (n: number) => String(n).padStart(2, "0");

/** FNV-1a over UTF-16 units; the real backend's hash is opaque to the UI anyway. */
function hash(text: string, seed = 0x811c9dc5): string {
  let h = seed;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193);
  return (h >>> 0).toString(16).padStart(8, "0");
}

function validName(file: string): boolean {
  return !file.includes("/") && !file.includes("..") && file.endsWith(".md");
}

type Tree = Map<string, string>;

interface Commit {
  id: string;
  parents: string[];
  tree: Tree;
  message: string;
  author: string;
  time: number;
  /** Creation order, newest highest. */
  seq: number;
}

interface UndoRecord {
  before: string | null;
  after: string;
  remote: string;
  /** Every path the pull wrote or removed: its content before and after (undefined = absent). */
  files: Map<string, { before: string | undefined; after: string | undefined }>;
  summary: SyncSummary;
}

interface MergeRecord {
  /** HEAD before Resolve with git, and the commit it made of uncommitted edits first. */
  head: string | null;
  precommit: string | null;
  mergeHead: string;
  /** Paths the merge changed in the working tree, with HEAD's version (undefined = absent). */
  touched: Map<string, string | undefined>;
  /** What the merge wrote at each touched path, to tell whether it was edited since. */
  wrote: Map<string, string | undefined>;
  conflicted: string[];
  startedByApp: boolean;
}

/** The whole picture `preview` works from, so `apply` uses exactly what the review showed. */
interface Analysis {
  preview: PullPreview;
  head: string | null;
  remote: string;
  base: Tree;
  local: Tree;
  theirs: Tree;
  remoteChanged: Set<string>;
  /** Every path an overlap decides, by overlap path (its base, mine and theirs paths). */
  paths: Map<string, string[]>;
  covered: Set<string>;
}

const RENAMED: OverlapKind[] = ["modified_renamed", "renamed_modified", "both_renamed"];
const WITH_DELETION: OverlapKind[] = ["modified_deleted", "deleted_modified", "renamed_deleted", "deleted_renamed"];

/** What changed from `before` to `after`, renames detected by identical content. */
function diffTrees(before: Tree, after: Tree): FileChange[] {
  const result: FileChange[] = [];
  const deleted = [...before.keys()].filter((f) => !after.has(f));
  for (const [path, content] of after) {
    const old = before.get(path);
    if (old === undefined) {
      const from = deleted.findIndex((d) => before.get(d) === content);
      if (from >= 0) result.push({ path, kind: "renamed", oldPath: deleted.splice(from, 1)[0] });
      else result.push({ path, kind: "added" });
    } else if (old !== content) {
      result.push({ path, kind: "modified" });
    }
  }
  for (const path of deleted) result.push({ path, kind: "deleted" });
  return result.sort((a, b) => a.path.localeCompare(b.path));
}

function setOrDelete(tree: Tree, path: string, content: string | undefined): void {
  if (content === undefined) tree.delete(path);
  else tree.set(path, content);
}

/** Base paths renamed from `before` to `after` (identical content, as `diffTrees` pairs them). */
function renames(before: Tree, after: Tree): Map<string, string> {
  return new Map(diffTrees(before, after).flatMap((c) => (c.oldPath ? [[c.oldPath, c.path] as const] : [])));
}

/**
 * `git merge`'s result over three trees: per path, the merged content (markers where changes
 * clash) and which paths conflict.
 */
function mergeTrees(base: Tree, ours: Tree, theirs: Tree, label: string): { result: Map<string, string | undefined>; conflicted: string[] } {
  const result = new Map<string, string | undefined>();
  const conflicted: string[] = [];
  for (const p of new Set([...base.keys(), ...ours.keys(), ...theirs.keys()])) {
    const [b, o, t] = [base.get(p), ours.get(p), theirs.get(p)];
    if (o === t || b === t) continue;
    let merged: string | undefined = t;
    if (b !== o) {
      if (o !== undefined && t !== undefined && !isBinary(p, b, o, t)) {
        const chunks = diff3(b ?? "", o, t);
        const clean = cleanMerge(chunks);
        if (clean === null) conflicted.push(p);
        merged = clean ?? withConflictMarkers(chunks, label);
      } else {
        // Modify/delete, or a binary file: git keeps what is there and marks it conflicted.
        conflicted.push(p);
        merged = o ?? t;
      }
    }
    result.set(p, merged);
  }
  return { result, conflicted: conflicted.sort() };
}

function sameTree(a: Tree, b: Tree): boolean {
  if (a.size !== b.size) return false;
  for (const [path, content] of a) if (b.get(path) !== content) return false;
  return true;
}

function isBinary(path: string, ...contents: (string | undefined)[]): boolean {
  return BINARY.test(path) || contents.some((c) => c?.includes("\0"));
}

export function createMockApi(options: MockOptions = {}): MockApi {
  const { latencyMs = 0, samples = true, clientIdPlaceholder = false } = options;
  const now = options.now ?? (() => new Date());

  const files = new Map<string, string>(samples ? Object.entries(SAMPLES) : []);
  const mtimes = new Map<string, number>();
  let settings: Settings = structuredClone(DEFAULT_SETTINGS);
  let auth: AuthStatus = { ...SIGNED_OUT };
  let deviceStarted = false;
  let polls = 0;

  // The repository: every commit by id; this clone's branch and origin/main; GitHub's branch.
  const commits = new Map<string, Commit>();
  let seq = 0;
  let headId: string | null = null;
  let fetchedId: string | null = null;
  let githubId: string | null = null;
  let connected: { fullName: string } | null = null;
  let lastFetch: number | null = null;
  let undo: UndoRecord | null = null;
  let merge: MergeRecord | null = null;
  let sampleActivityQueued = false;

  const calls: string[] = [];
  const failures = new Map<string, ApiError>();
  const syncListeners = new Set<(view: SyncView) => void>();
  const entryListeners = new Set<() => void>();

  function makeCommit(parents: string[], tree: Tree, message: string, author: string): string {
    const n = ++seq;
    const seed = `${n}\0${parents.join(",")}\0${message}`;
    const id = [1, 2, 3, 4, 5].map((k) => hash(seed, 0x811c9dc5 ^ k)).join("");
    commits.set(id, { id, parents, tree: new Map(tree), message: message.trimEnd(), author, time: now().getTime(), seq: n });
    return id;
  }

  function treeOf(id: string | null): Tree {
    return new Map(id ? commits.get(id)!.tree : []);
  }

  function ancestors(id: string | null): Set<string> {
    const seen = new Set<string>();
    const todo = id ? [id] : [];
    while (todo.length) {
      const next = todo.pop()!;
      if (seen.has(next)) continue;
      seen.add(next);
      todo.push(...commits.get(next)!.parents);
    }
    return seen;
  }

  /** Commits reachable from `from` but not from `exclude`, newest first. */
  function only(from: string | null, exclude: string | null): Commit[] {
    const excluded = ancestors(exclude);
    return [...ancestors(from)]
      .filter((id) => !excluded.has(id))
      .map((id) => commits.get(id)!)
      .sort((a, b) => b.seq - a.seq);
  }

  function mergeBase(a: string | null, b: string | null): string | null {
    if (!a || !b) return null;
    const ofA = ancestors(a);
    const queue = [b];
    const seen = new Set<string>();
    while (queue.length) {
      const id = queue.shift()!;
      if (ofA.has(id)) return id;
      if (seen.has(id)) continue;
      seen.add(id);
      queue.push(...commits.get(id)!.parents);
    }
    return null;
  }

  const info = (c: Commit): CommitInfo => ({ id: c.id, message: c.message, author: c.author, time: c.time });

  function authorName(): string {
    return settings.authorName?.trim() || auth.login || "mdgitjournal";
  }

  if (samples) {
    // This journal was on GitHub before, and the phone has added an entry there since.
    headId = makeCommit([], files, "Start the journal", "mdgitjournal");
    const phone = treeOf(headId);
    phone.set(PHONE_ENTRY, "Written on the phone.\n");
    githubId = makeCommit([headId], phone, "From my phone", "octocat");
  }

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
    return diffTrees(treeOf(headId), files);
  }

  function mergeState(): MergeState | null {
    if (!merge) return null;
    const conflicted = new Set(merge.conflicted);
    return {
      conflicted: [...merge.conflicted],
      merged: [...merge.touched.keys()].filter((p) => !conflicted.has(p)).sort(),
      startedByApp: merge.startedByApp,
      precommit: merge.precommit !== null,
      autostash: false,
    };
  }

  function view(): SyncView {
    return {
      connected: connected !== null,
      repo: connected?.fullName ?? null,
      branch: BRANCH,
      ahead: connected ? only(headId, fetchedId).length : 0,
      behind: connected ? only(fetchedId, headId).length : 0,
      changes: changes(),
      lastFetch,
      error: null,
      merge: mergeState(),
      lastSync: undo && headId === undo.after ? structuredClone(undo.summary) : null,
    };
  }

  function emitState(): SyncView {
    const v = view();
    for (const cb of syncListeners) cb(structuredClone(v));
    return v;
  }

  function entriesChanged(): void {
    for (const cb of entryListeners) cb();
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

  function write(file: string, content: string | undefined): void {
    setOrDelete(files, file, content);
    if (content !== undefined) mtimes.set(file, now().getTime());
  }

  function requireRemote(): void {
    if (!connected) throw new ApiError("not_connected", "This journal is not connected to GitHub.");
  }

  function requireNoMerge(): void {
    if (merge) throw new ApiError("merge_in_progress", MERGING);
  }

  function requireMessage(message: string | null): void {
    if (!message?.trim()) throw new ApiError("empty_message", "Write a commit message first.");
  }

  function fetchOrigin(): void {
    fetchedId = githubId;
    lastFetch = now().getTime();
  }

  /** `<dir>/<stem>.from-github-<hh>h<mm><ext>`, then -2, -3 … until free in W, R and the other copies. */
  function copyName(path: string, at: number, taken: (name: string) => boolean): string {
    const slash = path.lastIndexOf("/");
    const dir = path.slice(0, slash + 1);
    const name = path.slice(slash + 1);
    const dot = name.lastIndexOf(".");
    const [stem, ext] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
    const d = new Date(at);
    const base = `${dir}${stem}.from-github-${pad(d.getHours())}h${pad(d.getMinutes())}`;
    let copy = `${base}${ext}`;
    for (let n = 2; taken(copy); n++) copy = `${base}-${n}${ext}`;
    return copy;
  }

  function fingerprint(): string {
    const all = [...files.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([path, content]) => `${path}\0${content}\0`)
      .join("");
    return `${hash(all)}${hash(all, 0x01000193)}`;
  }

  /** The review of what was last fetched (mdgj-git's `preview_pull`); null when up to date. Writes nothing. */
  function analyse(): Analysis | null {
    requireNoMerge();
    const remote = fetchedId;
    if (!remote || remote === headId || ancestors(headId).has(remote)) return null;
    const baseId = mergeBase(headId, remote);
    const base = treeOf(baseId);
    const local = treeOf(headId);
    const theirs = treeOf(remote);
    const work = files;
    const diverged = headId !== null && !ancestors(remote).has(headId);

    const remoteChanged = new Set<string>();
    const committedOverlap: string[] = [];
    for (const p of new Set([...base.keys(), ...local.keys(), ...theirs.keys()])) {
      const [b, l, r] = [base.get(p), local.get(p), theirs.get(p)];
      if (b !== r) remoteChanged.add(p);
      if (diverged && b !== l && b !== r && l !== r) committedOverlap.push(p);
    }

    const at = commits.get(remote)!.time;
    const copies = new Set<string>();
    const taken = (name: string) => files.has(name) || theirs.has(name) || base.has(name) || local.has(name) || copies.has(name);
    const found: { overlap: Overlap; paths: string[] }[] = [];
    const describe = (kind: OverlapKind, path: string, basePath: string | null, minePath: string | null, theirsPath: string | null) => {
      const get = (tree: Tree, at: string | null) => (at === null ? undefined : tree.get(at));
      const [b, w, r] = [get(base, basePath), get(work, minePath), get(theirs, theirsPath)];
      const binary = isBinary(path, b, w, r);
      const choices: Choice[] = WITH_DELETION.includes(kind)
        ? ["mine", "theirs"]
        : binary
          ? ["mine", "theirs", "both"]
          : ["mine", "theirs", "both", "merged"];
      let copy: string | null = null;
      if (choices.includes("both")) {
        copy = RENAMED.includes(kind) ? theirsPath : copyName(path, at, taken);
        copies.add(copy!);
      }
      const chunks = choices.includes("merged") ? diff3(b ?? "", w ?? "", r ?? "") : null;
      const overlap: Overlap = {
        path,
        kind,
        binary,
        minePath,
        theirsPath,
        committed: false,
        versions: [b, w, r].map((c) => (c === undefined ? "0" : hash(c))).join(":"),
        choices,
        copy,
        suggestion: chunks && { clean: chunks.every((c) => c.kind !== "clash"), chunks },
      };
      const paths = [...new Set([basePath, minePath, theirsPath].filter((p): p is string => p !== null))].sort();
      found.push({ overlap, paths });
    };

    // Renamed on one side and changed on the other, or renamed differently on both.
    const mineRenames = renames(base, work);
    const theirRenames = renames(base, theirs);
    for (const from of [...new Set([...mineRenames.keys(), ...theirRenames.keys()])].sort()) {
      const [mineTo, theirTo] = [mineRenames.get(from), theirRenames.get(from)];
      const mine = mineTo ?? (work.has(from) ? from : null);
      const gh = theirTo ?? (theirs.has(from) ? from : null);
      if ((mineTo && theirs.has(mineTo) && gh !== mineTo) || (theirTo && work.has(theirTo) && mine !== theirTo)) continue;
      const [b, w, r] = [base.get(from), mine === null ? undefined : work.get(mine), gh === null ? undefined : theirs.get(gh)];
      const changedHere = mine !== from || w !== b;
      const changedThere = gh !== from || r !== b;
      if (!changedHere || !changedThere || (mine === gh && w === r)) continue;
      if (mine !== null && gh !== null) {
        if (mine === gh) describe("both_modified", mine, from, mine, gh);
        else if (mine === from) describe("modified_renamed", gh, from, mine, gh);
        else if (gh === from) describe("renamed_modified", mine, from, mine, gh);
        else describe("both_renamed", mine, from, mine, gh);
      } else if (mine !== null) describe("renamed_deleted", mine, from, mine, null);
      else if (gh !== null) describe("deleted_renamed", gh, from, null, gh);
    }
    const covered = new Set(found.flatMap((f) => f.paths));
    // Every other file, path by path.
    for (const p of [...remoteChanged].sort()) {
      if (covered.has(p)) continue;
      const [b, w, r] = [base.get(p), work.get(p), theirs.get(p)];
      if (b === w || w === r) continue;
      const kind: OverlapKind =
        b === undefined ? "both_added" : r === undefined ? "modified_deleted" : w === undefined ? "deleted_modified" : "both_modified";
      describe(kind, p, b === undefined ? null : p, w === undefined ? null : p, r === undefined ? null : p);
    }
    for (const f of found) for (const p of f.paths) covered.add(p);
    found.sort((a, b) => a.overlap.path.localeCompare(b.overlap.path));

    const localCommits = only(headId, remote);
    const plan: PullPlan = !diverged
      ? "fast_forward"
      : found.length || committedOverlap.length || localCommits.some((c) => c.parents.length > 1)
        ? "merge"
        : "rebase";
    if (plan === "merge") {
      for (const { overlap, paths } of found) {
        const side = (tree: Tree) => paths.map((p) => tree.get(p) ?? "\0absent").join("\0\0");
        overlap.committed = side(local) !== side(base) && side(local) !== side(theirs);
      }
    }

    const incoming = only(remote, headId);
    const target = new Map(files);
    for (const p of remoteChanged) if (!covered.has(p)) setOrDelete(target, p, theirs.get(p));
    const auto = diffTrees(files, target);
    const overlaps = found.map((f) => f.overlap);

    // Resolve with git: the uncommitted changes the merge writes over are committed first.
    const touches = (p: string | null | undefined) => p != null && (remoteChanged.has(p) || covered.has(p));
    const over = changes().filter((c) => touches(c.path) || touches(c.oldPath));
    const ours = withChanges(local, over);
    const gitMerge = overlaps.length
      ? { conflicted: mergeTrees(base, ours, theirs, `origin/${BRANCH}`).conflicted, uncommitted: over.map((c) => c.path) }
      : { conflicted: [], uncommitted: [] };

    return {
      preview: {
        id: `${BRANCH}:${headId ?? "0"}:${remote}:${fingerprint()}`,
        branch: BRANCH,
        plan,
        automatic: overlaps.length === 0 && plan !== "merge",
        incoming: incoming.slice(0, MAX_LISTED).map(info),
        incomingCount: incoming.length,
        local: localCommits.slice(0, MAX_LISTED).map(info),
        localCount: localCommits.length,
        auto,
        overlaps,
        gitMerge,
      },
      head: headId,
      remote,
      base,
      local,
      theirs,
      remoteChanged,
      paths: new Map(found.map((f) => [f.overlap.path, f.paths])),
      covered,
    };
  }

  /** `tree` with these working-tree changes applied. */
  function withChanges(tree: Tree, selected: FileChange[]): Tree {
    const result = new Map(tree);
    for (const c of selected) {
      if (c.oldPath) result.delete(c.oldPath);
      setOrDelete(result, c.path, files.get(c.path));
    }
    return result;
  }

  /** The same analysis as the review with this id, or `stale_review`. */
  function fresh(id: string): Analysis {
    const analysis = analyse();
    if (!analysis || analysis.preview.id !== id) throw new ApiError("stale_review", STALE);
    return analysis;
  }

  /** mdgj-git's `apply_pull`, after the staleness check. */
  function apply(analysis: Analysis, resolutions: Resolved[], message: string | null): SyncSummary {
    const { preview, base, local, theirs, remote, head } = analysis;
    const byPath = new Map<string, Resolved>();
    for (const r of resolutions) {
      if (byPath.has(r.path)) throw new ApiError("invalid", `${r.path} is resolved twice.`);
      byPath.set(r.path, r);
    }
    for (const o of preview.overlaps) {
      const r = byPath.get(o.path);
      if (!r) throw new ApiError("invalid", `Choose what to keep of ${o.path}.`);
      if (!o.choices.includes(r.choice)) throw new ApiError("invalid", `${o.path} can't be resolved with “${r.choice}”.`);
      if (r.choice === "merged" && typeof r.text !== "string") throw new ApiError("invalid", `The merged text of ${o.path} is missing.`);
    }
    const extra = resolutions.find((r) => !analysis.paths.has(r.path));
    if (extra) throw new ApiError("invalid", `${extra.path} didn't change on both sides.`);
    if (preview.plan === "merge") requireMessage(message);

    // T: the working tree the pull leaves.
    const target = new Map(files);
    for (const p of analysis.remoteChanged) if (!analysis.covered.has(p)) setOrDelete(target, p, theirs.get(p));
    for (const o of preview.overlaps) {
      const r = byPath.get(o.path)!;
      const paths = analysis.paths.get(o.path)!;
      if (r.choice === "theirs") for (const p of paths) setOrDelete(target, p, theirs.get(p));
      else if (r.choice === "both") target.set(o.copy!, theirs.get(o.theirsPath!)!);
      else if (r.choice === "merged") {
        for (const p of paths) target.delete(p);
        target.set(o.path, r.text);
      }
    }

    // N: the new head.
    let newHead: string;
    let rebased = 0;
    let mergeCommit: string | null = null;
    if (preview.plan === "fast_forward") {
      newHead = remote;
    } else if (preview.plan === "rebase") {
      newHead = remote;
      for (const c of only(head, remote).reverse()) {
        const tree = treeOf(newHead);
        const parent = treeOf(c.parents[0] ?? null);
        for (const ch of diffTrees(parent, c.tree)) {
          if (ch.oldPath) tree.delete(ch.oldPath);
          setOrDelete(tree, ch.path, c.tree.get(ch.path));
        }
        // A commit whose changes GitHub already has becomes empty and is dropped.
        if (sameTree(tree, treeOf(newHead))) continue;
        newHead = makeCommit([newHead], tree, c.message, c.author);
        commits.get(newHead)!.time = c.time;
        rebased++;
      }
    } else {
      // What each side committed; resolutions of uncommitted edits stay uncommitted.
      const tree: Tree = new Map();
      for (const p of new Set([...base.keys(), ...local.keys(), ...theirs.keys()])) {
        if (analysis.covered.has(p)) continue;
        const [b, l, r] = [base.get(p), local.get(p), theirs.get(p)];
        setOrDelete(tree, p, l === b ? r : r === b || l === r ? l : target.get(p));
      }
      for (const o of preview.overlaps) {
        const paths = analysis.paths.get(o.path)!;
        const r = byPath.get(o.path)!;
        const take = (from: Tree) => paths.forEach((p) => setOrDelete(tree, p, from.get(p)));
        if (!o.committed) take(paths.every((p) => local.get(p) === base.get(p)) ? theirs : local);
        else if (r.choice === "mine") take(local);
        else if (r.choice === "theirs") take(theirs);
        else if (r.choice === "both") {
          take(local);
          tree.set(o.copy!, theirs.get(o.theirsPath!)!);
        } else if (r.choice === "merged") {
          paths.forEach((p) => tree.delete(p));
          const [w, l] = [files.get(o.minePath ?? ""), local.get(o.minePath ?? "")];
          // The merge without the uncommitted edits it was made from, where they don't touch it.
          const committed = w !== undefined && l !== undefined && w !== l ? cleanMerge(diff3(w, l, r.text)) : null;
          tree.set(o.path, committed ?? r.text);
        }
      }
      newHead = makeCommit([head!, remote], tree, message!.trim(), authorName());
      mergeCommit = newHead;
    }

    const summary: SyncSummary = {
      at: now().getTime(),
      plan: preview.plan,
      incoming: preview.incoming,
      incomingCount: preview.incomingCount,
      rebased,
      mergeCommit,
      files: diffTrees(files, target),
      resolved: preview.overlaps.map((o) => {
        const choice = byPath.get(o.path)!.choice;
        return { path: o.path, kind: o.kind, choice, copy: choice === "both" ? o.copy : null };
      }),
    };
    const touched = new Map<string, { before: string | undefined; after: string | undefined }>();
    for (const p of new Set([...files.keys(), ...target.keys()])) {
      if (files.get(p) !== target.get(p)) touched.set(p, { before: files.get(p), after: target.get(p) });
    }
    undo = { before: head, after: newHead, remote, files: touched, summary: structuredClone(summary) };
    for (const [p, { after }] of touched) write(p, after);
    headId = newHead;
    return summary;
  }

  function incoming(analysis: Analysis | null): PullResult {
    if (!analysis) return { status: "up_to_date", view: emitState() };
    if (!analysis.preview.automatic) return { status: "review", view: emitState(), preview: analysis.preview };
    const summary = apply(analysis, [], null);
    const v = emitState();
    if (summary.files.length) entriesChanged();
    return { status: "applied", view: v, summary };
  }

  /** mdgj-git's `commit(message, paths)`: HEAD's tree plus exactly the ticked changes. */
  function commitPaths(message: string, paths: string[]): boolean {
    requireMessage(message);
    const ticked = new Set(paths);
    const selected = changes().filter((c) => ticked.has(c.path));
    if (!selected.length) return false;
    const tree = treeOf(headId);
    for (const c of selected) {
      if (c.oldPath) tree.delete(c.oldPath);
      setOrDelete(tree, c.path, files.get(c.path));
    }
    headId = makeCommit(headId ? [headId] : [], tree, message.trim(), authorName());
    undo = null;
    return true;
  }

  /** Pushes the waiting commits after a fetch, as `commit_and_push` does; GitHub must not have moved on. */
  function push(): "pushed" | "needs_pull" | "nothing" {
    if (!only(headId, fetchedId).length) return "nothing";
    fetchOrigin();
    if (only(fetchedId, headId).length) return "needs_pull";
    githubId = headId;
    fetchedId = headId;
    undo = null;
    return "pushed";
  }

  /** The three-way merge `git merge origin/main` leaves, with markers in files whose changes clash. */
  function startMerge(remote: string): MergeRecord {
    const base = treeOf(mergeBase(headId, remote));
    const ours = treeOf(headId);
    const { result, conflicted } = mergeTrees(base, ours, treeOf(remote), `origin/${BRANCH}`);
    const touched = new Map<string, string | undefined>();
    const wrote = new Map<string, string | undefined>();
    for (const [p, merged] of result) {
      if (files.get(p) === merged) continue;
      touched.set(p, ours.get(p));
      wrote.set(p, merged);
      write(p, merged);
    }
    return { head: headId, precommit: null, mergeHead: remote, touched, wrote, conflicted, startedByApp: true };
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
      // Named by content like the backend's (which hashes the JPEG it made, not the input); the
      // same image again is the same file.
      const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
      const hex = [...digest.slice(0, 4)].map((b) => b.toString(16).padStart(2, "0")).join("");
      const path = `assets/${entryFile.replace(/\.md$/, "")}-${hex}.jpg`;
      if (!files.has(path)) {
        const url = typeof URL.createObjectURL === "function" ? URL.createObjectURL(new Blob([bytes])) : "";
        files.set(path, url);
      }
      return { path, bytes: bytes.length, width: 0, height: 0 };
    },

    async syncState() {
      await step("syncState");
      return view();
    },
    async fetch() {
      await step("fetch", 4);
      if (connected) fetchOrigin();
      return emitState();
    },
    async pull() {
      await step("pull", 4);
      requireRemote();
      requireNoMerge();
      fetchOrigin();
      return incoming(analyse());
    },
    async pullPreview() {
      await step("pullPreview");
      const analysis = analyse();
      return analysis
        ? { status: "review", view: emitState(), preview: analysis.preview }
        : { status: "up_to_date", view: emitState() };
    },
    async applyPull(id, resolutions, message) {
      await step("applyPull", 2);
      const summary = apply(fresh(id), resolutions, message);
      const v = emitState();
      if (summary.files.length) entriesChanged();
      return { view: v, summary };
    },
    async undoSync() {
      await step("undoSync");
      requireNoMerge();
      if (!undo) throw new ApiError("invalid", "There is no pull to undo.");
      if (headId !== undo.after) {
        throw new ApiError("undo_refused", "You've committed since the pull, so Undo can't reverse it.");
      }
      if (undo.after !== undo.remote && fetchedId && ancestors(fetchedId).has(undo.after)) {
        throw new ApiError("undo_refused", "The pull has been pushed to GitHub, so Undo can't reverse it.");
      }
      const edited = [...undo.files]
        .filter(([p, { before, after }]) => files.get(p) !== before && files.get(p) !== after)
        .map(([p]) => p);
      if (edited.length) {
        throw new ApiError("undo_refused", `You've changed ${edited.join(", ")} since the pull; Undo would overwrite it.`);
      }
      const restored = [...undo.files.keys()].sort();
      for (const [p, { before }] of undo.files) write(p, before);
      headId = undo.before;
      undo = null;
      const v = emitState();
      if (restored.length) entriesChanged();
      return { view: v, restored };
    },
    async startExternalMerge(id, message) {
      await step("startExternalMerge", 2);
      const analysis = fresh(id);
      const head = headId;
      let precommit: string | null = null;
      // Only the uncommitted changes git merges over; other files stay uncommitted.
      const over = new Set(analysis.preview.gitMerge.uncommitted);
      const selected = changes().filter((c) => over.has(c.path));
      if (selected.length) {
        requireMessage(message);
        precommit = headId = makeCommit(head ? [head] : [], withChanges(treeOf(head), selected), message!.trim(), authorName());
      } else if (analysis.preview.plan === "fast_forward") {
        throw new ApiError("invalid", "There is nothing to merge: Pull brings in GitHub's commits as they are.");
      }
      merge = { ...startMerge(analysis.remote), head, precommit };
      undo = null;
      const v = emitState();
      entriesChanged();
      return v;
    },
    async abortMerge() {
      await step("abortMerge");
      if (!merge) throw new ApiError("invalid", "No merge is in progress.");
      const conflicted = new Set(merge.conflicted);
      const edited = [...merge.wrote].filter(([p, wrote]) => !conflicted.has(p) && files.get(p) !== wrote).map(([p]) => `“${p}”`);
      if (edited.length) {
        throw new ApiError(
          "invalid",
          `${edited.join(" and ")} changed after the merge started, and Abort would discard that. Copy the text somewhere safe or finish the merge with git, then abort again.`,
        );
      }
      const restored = [...merge.touched.keys()].sort();
      for (const [p, content] of merge.touched) write(p, content);
      // Resolve with git committed the uncommitted edits first: they are uncommitted again.
      if (merge.precommit && headId === merge.precommit) headId = merge.head;
      merge = null;
      const v = emitState();
      if (restored.length) entriesChanged();
      return { view: v, restored, stashed: false };
    },
    async commit(message, paths) {
      await step("commit");
      requireNoMerge();
      commitPaths(message, paths);
      return emitState();
    },
    async commitAndPush(message, paths) {
      await step("commitAndPush", 4);
      requireRemote();
      requireNoMerge();
      // A blank message with nothing ticked only pushes the commits that are waiting.
      if (message.trim() || paths.length) commitPaths(message, paths);
      return { result: push(), view: emitState() };
    },
    async diff(path) {
      await step("diff");
      const change = changes().find((c) => c.path === path);
      if (!change) throw new ApiError("invalid", `${path} has no changes.`);
      const oldPath = change.oldPath ?? null;
      const before = treeOf(headId).get(oldPath ?? path);
      const after = files.get(path);
      const binary = isBinary(path, before, after) || (oldPath !== null && isBinary(oldPath));
      return {
        path,
        oldPath,
        kind: change.kind,
        binary,
        hunks: binary ? [] : unifiedHunks(before ?? "", after ?? ""),
      };
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
      requireNoMerge();
      if (!auth.signedIn) throw new ApiError("auth", "Sign in to GitHub first.");
      if (!/^[\w.-]+\/[\w.-]+$/.test(fullName)) throw new ApiError("invalid", "Use the form owner/repo.");
      connected = { fullName };
      settings.remote = {
        fullName,
        url: `https://github.com/${fullName}.git`,
        branch: BRANCH,
        auth: auth.kind ?? "github_app",
      };
      fetchOrigin();
      let result: PullResult;
      if (githubId === null) {
        // An empty repository: this journal's history goes up as it is.
        if (headId) push();
        result = { status: "up_to_date", view: emitState() };
      } else {
        result = incoming(analyse());
      }
      if (samples && !sampleActivityQueued) {
        sampleActivityQueued = true;
        api.mock.remoteCommit("2026-11-16-0630-dawn.md", "Up before the sun.\n", "Dawn");
        const ideas = "2026-11-14-2130-ideas.md";
        const onGitHub = treeOf(githubId).get(ideas) ?? "";
        api.mock.remoteCommit(ideas, `${onGitHub}\nEdited on the phone.\n`, "Ideas from the phone");
      }
      return result;
    },
    async githubDisconnect(signOut) {
      await step("githubDisconnect");
      connected = null;
      settings.remote = null;
      fetchedId = null;
      undo = null;
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
    async thirdPartyNotices() {
      await step("thirdPartyNotices");
      return "# Third-party notices\n\nThe app shows packaging/THIRD-PARTY-NOTICES.md here.\n";
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
    // A browser tab closes without asking, and the mock keeps nothing anyway.
    async onCloseRequested() {
      return () => {};
    },
    async closeWindow() {
      window.close();
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
      remoteCommit(file, content, message) {
        const tree = treeOf(githubId);
        const verb = content === null ? "Delete" : tree.has(file) ? "Update" : "Add";
        setOrDelete(tree, file, content ?? undefined);
        githubId = makeCommit(githubId ? [githubId] : [], tree, message ?? `${verb} ${file}`, "octocat");
      },
      editOnDisk(file, content) {
        write(file, content ?? undefined);
      },
      finishMergeWithGit() {
        if (!merge) throw new Error("mock: no merge is in progress");
        const message = `Merge remote-tracking branch 'origin/${BRANCH}'`;
        headId = makeCommit(headId ? [headId, merge.mergeHead] : [merge.mergeHead], files, message, authorName());
        merge = null;
      },
      abortMergeWithGit() {
        if (!merge) throw new Error("mock: no merge is in progress");
        for (const [p, content] of merge.touched) write(p, content);
        merge = null;
      },
      failNext(method, kind, message) {
        failures.set(method, new ApiError(kind, message));
      },
      seedOverlaps() {
        if (!connected) throw new Error("mock: connect before seeding overlaps");
        const clean = "2026-11-16-1402-morning-run.md";
        const clash = "2026-10-28-0815-reading-list.md";
        const onGitHub = treeOf(githubId);
        const edit = (tree: Tree, file: string, from: string, to: string) =>
          (tree.get(file) ?? SAMPLES[file]).replace(from, to);
        // GitHub ticks the last line of one entry while this device edits its first paragraph; both
        // change the same line of the other.
        api.mock.remoteCommit(clean, edit(onGitHub, clean, "- [ ] new shoes", "- [x] new shoes"), "Bought the shoes");
        api.mock.remoteCommit(clash, edit(onGitHub, clash, "- *Walden*", "- *Walden*, H. D. Thoreau"), "Reading list: author");
        write(clean, edit(files, clean, "Five miles", "Six miles"));
        write(clash, edit(files, clash, "- *Walden*", "- *Walden* (again)"));
        return { clean, clash };
      },
    },
  };
  return api;
}
