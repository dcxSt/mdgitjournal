# mdgitjournal — architecture & interface contract

This file is the **contract** between the pieces of the app. PLAN.md says *what* to build; this file says *how the pieces talk*. If you change an interface here, update every side of it.

Identifiers:
- Product name: `mdgitjournal`
- Bundle ID: `xyz.stephenfay.mdgitjournal`
- Homepage: `https://mdgitjournal.stephenfay.xyz`
- GitHub: `dcxSt/mdgitjournal` (public, MIT)
- Test repo: `dcxSt/mdgitjournal-test` (private)
- Env-var prefix: `MDGJ_`

## Workspace layout

```
Cargo.toml                 # [workspace] members = crates/*, app/src-tauri
crates/mdgj-core/          # entries, slug, filesystem journal ops, search, word count, images, settings
crates/mdgj-git/           # libgit2 wrapper: init, status, diff, commit, fetch, pull (preview, review, apply, undo), external merge, push, connect
crates/mdgj-github/        # GitHub App device flow, token refresh, token store (keyring + file fallback), REST calls
app/                       # pnpm package: Vite + Svelte 5 + TypeScript
app/src/editor/            # CodeMirror 6 editor (source + styled), vim, image paste hooks, merge editor
app/patches/               # pnpm patch of @replit/codemirror-vim-core (visual mode on emoji)
app/src/lib/api.ts         # typed wrappers around Tauri invoke + a mock backend for browser dev/tests
app/src/lib/types.ts       # TS mirror of the command types below
app/src/...                # Svelte UI
app/src-tauri/             # Tauri 2 shell: state, commands (thin glue over the crates), events
scripts/                   # two-devices.sh: two instances on one machine
packaging/aur/  site/  docs/  .github/workflows/
```

The three crates **do not depend on each other**. Only `app/src-tauri` depends on all three. Each crate has its own unit and integration tests (`cargo test -p <crate>`); mdgj-git's tests also use mdgj-core, to import photos on two devices.

## Crate: `mdgj-core`

```rust
// entries.rs — filenames are `yyyy-mm-dd-hhmm-<slug>.md`
pub fn slugify(title: &str) -> String;                  // lowercase, ASCII-fold (deunicode), [a-z0-9-], collapse/trim '-', "" -> "untitled"
pub struct EntryName { pub date: NaiveDate, pub time: NaiveTime /* hh:mm */, pub slug: String, pub dup: Option<u32> }
pub fn parse_filename(name: &str) -> Option<EntryName>; // accepts "2026-11-16-1402-morning-run.md" and "...-morning-run-2.md" (dup suffix is part of slug; keep it simple: slug = everything after hhmm-)
pub fn format_filename(date, time, slug) -> String;

// journal.rs — a Journal is a directory (the git working tree)
pub struct Journal { root: PathBuf }
impl Journal {
  pub fn open(root: impl Into<PathBuf>) -> Result<Self>;          // creates dir if missing
  pub fn root(&self) -> &Path;
  pub fn list(&self) -> Result<Vec<EntryMeta>>;                    // *.md in root (NOT recursive; ignores dotfiles, assets/), newest first; non-matching .md files last, sorted by name
  pub fn read(&self, file: &str) -> Result<EntryContent>;         // file = bare file name, validated (no '/', no '..', must end .md)
  pub fn write(&self, file: &str, content: &str) -> Result<String /*hash*/>;  // atomic: write temp in same dir + fsync + rename; returns content hash
  pub fn create(&self, title: &str, now: NaiveDateTime) -> Result<EntryMeta>;    // empty file; dedupe with -2, -3 …
  pub fn rename(&self, file: &str, new_title: &str) -> Result<EntryMeta>;       // keeps date+time prefix, only slug changes; no-op if same; dedupe
  pub fn delete(&self, file: &str) -> Result<()>;
  pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>>;   // case-insensitive substring over filename + content; snippet around match
  pub fn import_image(&self, entry_file: &str, bytes: &[u8], opts: ImageOpts) -> Result<ImportedImage>; // see images.rs; writes assets/<entry-stem>-<first 8 hex digits of the JPEG's SHA-256>.jpg (identical bytes reuse the file; -2, -3 … only if the name holds a different file); refuses an `assets` that is a symlink or a file
}
pub struct EntryMeta { pub file: String, pub date: Option<String> /*"2026-11-16"*/, pub time: Option<String> /*"14:02"*/, pub title: String /*slug, or file stem for Other*/, pub mtime: i64 /*unix ms*/, pub size: u64, pub words: Option<u32> }
pub struct EntryContent { pub file: String, pub content: String, pub hash: String }
pub struct SearchHit { pub file: String, pub title: String, pub line: u32 /*1-based, 0 = filename match*/, pub snippet: String, pub match_start: u32, pub match_end: u32 /*char offsets within snippet*/ }
pub fn word_count(text: &str) -> u32;  // unicode words, ignores markdown punctuation like '#', '-', '*'

// images.rs
pub struct ImageOpts { pub max_edge: u32 /*1024*/, pub quality: u8 /*60*/ }
pub struct ImportedImage { pub path: String /*"assets/2026-11-16-1402-morning-run-5f3a09c2.jpg"*/, pub bytes: u64, pub width: u32, pub height: u32 }
pub fn compress_image(bytes: &[u8], opts: ImageOpts) -> Result<(Vec<u8>, u32, u32)>; // decode jpeg/png/webp/gif(first frame), refusing more than 64 megapixels (Error::Image "the image is too large to import") before decoding; apply EXIF orientation; strip ALL metadata; Lanczos3 downscale to max_edge (never upscale); flatten alpha onto white; encode baseline/progressive JPEG at quality

// settings.rs — per-device settings, stored as JSON outside the repo
pub struct Settings { pub vim: bool, pub mode_desktop: Mode, pub mode_mobile: Mode, pub fetch_interval_min: u32 /*5; 0 = off*/,
                      pub image_max_edge: u32, pub image_quality: u8, pub theme: Theme /*System|Light|Dark*/,
                      pub author_name: Option<String>, pub author_email: Option<String>,
                      pub remote: Option<RemoteConfig> }
pub enum Mode { Source, Styled }   // serde: "source" | "styled"
pub struct RemoteConfig { pub full_name: String /*"owner/repo"*/, pub url: String /*https://github.com/owner/repo.git*/, pub branch: String, pub auth: AuthKind }
pub enum AuthKind { GithubApp, Pat }  // serde: "github_app" | "pat"
impl Settings { pub fn load(path: &Path) -> Settings /*defaults on missing/corrupt*/; pub fn save(&self, path: &Path) -> Result<()> /*atomic*/ }
```

All serde structs use `#[serde(rename_all = "camelCase")]`, so they cross the Tauri boundary as camelCase.

## Crate: `mdgj-git`

The crate uses libgit2 via `git2 = { default-features = false, features = ["https", "vendored-libgit2", "vendored-openssl"] }`. OpenSSL is built from source on every target because libgit2-sys links it on all Unix targets, though macOS and iOS use Apple's TLS. No SSH. Line diffs come from libgit2 (`git2::Patch::from_buffers`, the same xdiff git uses), and the three-way chunking is in `src/diff3.rs`. `serde_json` (already in the workspace, for mdgj-core and mdgj-github) reads and writes the undo and merge records.

The rule everything below implements: **a path changed on one side only flows automatically; a path changed on both sides waits for the user.** Nothing in this crate ever writes conflict markers into the journal, except `start_external_merge`, which exists to do exactly that, the way `git merge` does.

```rust
pub struct Creds { pub username: String /*"x-access-token"*/, pub password: String /*token*/, pub url_prefix: String /*the token is only sent to URLs starting with this, e.g. "https://github.com/"*/ }  // None for local/file:// remotes
pub struct Author { pub name: String, pub email: String }

pub struct Repo { /* wraps git2::Repository */ }
impl Repo {
  pub fn open_or_init(path: &Path) -> Result<Repo>;     // init with initial branch "main" if not a repo
  pub fn status(&self) -> Result<Vec<FileChange>>;       // working tree vs HEAD, includes untracked (respects .gitignore), renames detected
  pub fn diff(&self, path: &str) -> Result<FileDiff>;    // one changed file vs HEAD (see "Diff")

  pub fn commit(&self, message: &str, paths: &[String], author: &Author) -> Result<Option<String /*oid*/>>; // only these paths (see "Commit")
  pub fn commit_all(&self, message: &str, author: &Author) -> Result<Option<String>>; // = commit(message, <every changed path>)

  pub fn remote_url(&self) -> Option<String>;
  pub fn set_remote(&self, url: &str) -> Result<()>;      // remote "origin"; URL stored WITHOUT credentials
  pub fn remove_remote(&self) -> Result<()>;               // also clears the undo record
  pub fn fetch(&self, creds: Option<&Creds>) -> Result<SyncState>;
  pub fn sync_state(&self) -> Result<SyncState>;          // local only, no network, never writes

  pub fn pull(&self, creds: Option<&Creds>, author: &Author) -> Result<PullOutcome>;   // fetch + preview_pull; applies at once iff the preview is `automatic`
  pub fn preview_pull(&self) -> Result<Option<PullPreview>>;                          // local only; None = up to date; writes NOTHING (no refs, index, files or objects)
  pub fn apply_pull(&self, id: &str, resolutions: &[Resolved], message: Option<&str>, author: &Author) -> Result<SyncSummary>;
  pub fn last_sync(&self) -> Result<Option<SyncSummary>>;  // the pull undo_last_sync would reverse
  pub fn undo_last_sync(&self) -> Result<Vec<String> /*paths restored*/>;

  pub fn start_external_merge(&self, id: &str, message: Option<&str>, author: &Author) -> Result<()>;
  pub fn merge_state(&self) -> Result<Option<MergeState>>;
  pub fn abort_merge(&self) -> Result<Vec<String> /*paths restored*/>;

  pub fn push(&self, creds: Option<&Creds>) -> Result<PushOutcome>;
  pub fn commit_and_push(&self, message: &str, paths: &[String], author: &Author, creds: Option<&Creds>) -> Result<CommitPushOutcome>;
  pub fn connect(&self, url: &str, creds: Option<&Creds>, author: &Author) -> Result<PullOutcome>; // see below
}
```

Types. Every struct is `#[serde(rename_all = "camelCase")]`, every enum `rename_all = "snake_case"`; `Option` fields serialize as `null` (not skipped) unless noted. `Resolved`/`Resolution` also derive `Deserialize` (they come from the UI).

```rust
pub struct FileChange { pub path: String, pub kind: ChangeKind /*added|modified|deleted|renamed*/, #[serde(skip_serializing_if = "Option::is_none")] pub old_path: Option<String> }
pub struct SyncState { pub has_remote: bool, pub branch: String, pub ahead: u32, pub behind: u32, pub changes: Vec<FileChange>, pub last_fetch: Option<i64 /*unix ms*/>,
                       pub merge: Option<MergeState>,        // a git merge is in progress (MERGE_HEAD exists)
                       pub last_sync: Option<SyncSummary> }  // = last_sync(), only while Undo is possible

pub struct CommitInfo { pub id: String /*full oid*/, pub message: String /*full message, trailing whitespace trimmed; the UI shows the first line*/, pub author: String /*name*/, pub time: i64 /*unix ms*/ }
pub enum PullPlan { FastForward, Rebase, Merge }          // what Apply does to history; see "Plan"

pub struct PullPreview {
  pub id: String,                 // opaque fingerprint "<branch>:<L or 0>:<R>:<W tree>" (see "Staleness")
  pub branch: String,
  pub plan: PullPlan,
  pub automatic: bool,            // overlaps.is_empty() && plan != Merge: Pull applies it without asking
  pub incoming: Vec<CommitInfo>,  // L..R (in R, not in L), newest first, at most 50
  pub incoming_count: u32,
  pub local: Vec<CommitInfo>,     // R..L (unpushed), newest first, at most 50
  pub local_count: u32,
  pub auto: Vec<FileChange>,      // one-sided incoming changes, as they will change the working tree (renames detected)
  pub overlaps: Vec<Overlap>,     // sorted by path
}
pub enum OverlapKind { BothModified, BothAdded, ModifiedDeleted /*mine modified, GitHub deleted*/, DeletedModified /*mine deleted, GitHub modified*/ }
pub enum Choice { Mine, Theirs, Both, Merged }
pub struct Overlap {
  pub path: String,
  pub kind: OverlapKind,
  pub binary: bool,               // any present side is not UTF-8, contains a NUL byte, or isn't a regular file (symlink, submodule)
  pub base: Option<String>, pub mine: Option<String>, pub theirs: Option<String>, // full text; None if that side is absent or the file is binary
  pub choices: Vec<Choice>,       // allowed, in this order: text both-sides [mine, theirs, both, merged]; binary both-sides [mine, theirs, both]; modified_deleted / deleted_modified [mine, theirs]
  pub copy: Option<String>,       // where Keep both puts GitHub's version; Some iff `both` is allowed
  pub suggestion: Option<MergeSuggestion>, // Some iff `merged` is allowed
}
pub struct MergeSuggestion { pub clean: bool /*no Clash chunks*/, pub chunks: Vec<MergeChunk> }
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MergeChunk {                       // whole lines; concatenating every chunk's text (a clash contributing its pick) gives the merged file byte for byte
  Same   { text: String },                  // unchanged on both sides
  Mine   { text: String, base: String },    // changed here only: already applied (text "" = lines deleted here)
  Theirs { text: String, base: String },    // changed on GitHub only: already applied
  Both   { text: String, base: String },    // the same change on both sides
  Clash  { base: String, mine: String, theirs: String }, // the user picks
}

#[serde(tag = "choice", rename_all = "snake_case")]
pub enum Resolution { Mine, Theirs, Both, Merged { text: String } }
pub struct Resolved { pub path: String, #[serde(flatten)] pub resolution: Resolution }   // JSON: { "path": "...", "choice": "merged", "text": "..." }

pub struct ResolvedFile { pub path: String, pub choice: Choice, pub copy: Option<String> }
pub struct SyncSummary {
  pub at: i64,                    // unix ms
  pub plan: PullPlan,
  pub incoming: Vec<CommitInfo>, pub incoming_count: u32,
  pub rebased: u32,               // unpushed commits placed after GitHub's (plan Rebase), else 0
  pub merge_commit: Option<String>, // oid of the merge commit (plan Merge)
  pub files: Vec<FileChange>,     // what the pull changed in the working tree, relative to before it (renames detected); includes Keep both copies
  pub resolved: Vec<ResolvedFile>,
}
pub struct MergeState { pub conflicted: Vec<String> /*paths with conflict entries in the index*/, pub started_by_app: bool }

pub enum LineKind { Context, Added, Removed }
pub struct DiffLine { pub kind: LineKind, pub text: String /*without its line break*/ }
pub struct DiffHunk { pub old_start: u32, pub old_lines: u32, pub new_start: u32, pub new_lines: u32, pub lines: Vec<DiffLine> }
pub struct FileDiff { pub path: String, pub old_path: Option<String>, pub kind: ChangeKind, pub binary: bool, pub hunks: Vec<DiffHunk> /*empty if binary*/ }

#[serde(tag = "status", rename_all = "snake_case")]
pub enum PullOutcome { UpToDate { state: SyncState }, Applied { state: SyncState, summary: SyncSummary }, Review { state: SyncState, preview: PullPreview } }
pub enum PushOutcome { Pushed { state: SyncState }, NothingToPush { state: SyncState }, Rejected { state: SyncState } /*non-ff: remote moved; caller should show Pull*/ }
pub enum CommitPushOutcome { Pushed { state: SyncState }, NeedsPull { state: SyncState } /*committed locally, remote ahead*/, NothingToCommit { state: SyncState } }

pub enum Error { EmptyMessage, Auth(String), Network(String), NotConnected, Git(String), Io(String),
                 StaleReview,            // "Something changed since you opened the review."
                 MergeInProgress,        // "A merge is in progress. Finish it with git, or abort it."
                 UndoRefused(String),    // why, naming the files or the commit/push that came after
                 Invalid(String) }       // e.g. resolutions that don't match the review
// Display messages are user-readable; "failed to connect to …" is Network.
```

There is no silent merge: no conflict copies, no remote-wins, no 3-way merge applied without the review.

### Names used below

- `b` = the branch HEAD is on (a detached HEAD is `Error::Git`). `L` = HEAD's commit (may be unborn). `R` = `refs/remotes/origin/<b>`. `B` = merge-base(L, R); none (unborn L, or unrelated histories) means the empty tree.
- `W` = the working tree as a tree: tracked and untracked files, minus ignored ones (the existing in-memory `Snapshot`, which writes nothing).
- For a path `p`: `base[p]`, `local[p]` (in L), `work[p]` (in W), `remote[p]` (in R), each a `(blob, mode)` or absent. **Mine = `work[p]`** (what is on disk, including unpushed commits and uncommitted edits); **GitHub's = `remote[p]`**.
- **Remote-changed** `C_R = { p : base[p] ≠ remote[p] }`. **Locally changed** `C_W = { p : base[p] ≠ work[p] }`.
- **Overlaps** `O = { p ∈ C_R ∩ C_W : work[p] ≠ remote[p] }`. The same change on both sides (e.g. the same photo imported on two devices) is not an overlap.
- **Committed overlaps** `O_c = { p : base[p] ≠ local[p], base[p] ≠ remote[p], local[p] ≠ remote[p] }` (only when diverged).
- Kind of an overlap: base present and both present → `both_modified`; base absent → `both_added`; remote absent → `modified_deleted`; work absent → `deleted_modified`. Renames are not followed: a remote rename is a delete plus an add, so a local edit to the old name is `modified_deleted` there, and the new name arrives automatically.
- Text: every present side is valid UTF-8 without a NUL byte; otherwise `binary: true`.

### Preview (`preview_pull`) — read-only

1. `repo.state()` must be clean: a merge → `Error::MergeInProgress`; any other operation (rebase, cherry-pick, …) → `Error::Git("a git <op> is in progress; finish it with git first")`. A leftover `merge.json` with a clean state is deleted first (the user finished or aborted with git).
2. No R, or L == R, or R is an ancestor of L → `None` (up to date).
3. Compute W, O, `O_c`, the plan, `auto`, the overlaps and their suggestions.
4. Check the target tree it would write (W with `auto` applied and any resolution) as `worktree::apply` does: no symlinks from the remote, no two paths that differ only in case, no folder in the way of a file, no ignored local file where GitHub has a file ("…is on this device but ignored by git… move it out of the way"), no file name that isn't UTF-8 among the paths it writes or removes (the undo record keys them as JSON strings). A failure is `Error::Git`/`Error::Io` naming the path and asking to fix it with git; there is no review for these.
5. Everything is computed on the snapshot's in-memory object database (mempack), so **no object, ref, index or file is written**. `Cancel` therefore needs no backend call at all.

**Plan:**
- `FastForward` — L is unborn, or L is an ancestor of R.
- `Rebase` — diverged, and all of: no commit in R..L has more than one parent; `O_c` is empty; every commit of R..L replays onto R with `merge_trees` without a conflict (tried in memory, oldest first, with rename detection off to match the per-path overlaps; commits that become empty are dropped); and the replay ends with exactly the per-path tree (local's version where the local commits changed a path, else GitHub's).
- `Merge` — diverged otherwise. A merge commit is always shown to the user first, so **`Merge` is never automatic**, even with no overlaps (then the review only asks for the commit message).

**Suggestion (`diff3.rs`):** split base, mine and theirs into lines *including their line break* (`split_inclusive('\n')`, so `\r\n` and a missing last newline survive byte for byte). Diff base→mine and base→theirs with `git2::Patch::from_buffers` (`context_lines(0)`, `interhunk_lines(0)`, force text, Myers). Walk the hunks in base order; hunks from the two sides that overlap **or touch** (as git does: adjacent edits clash) form one group. A group with hunks from one side is `Mine`/`Theirs`; from both sides, it is `Both` if the two replacements are identical, else `Clash` (over the union of their base ranges, each side's text for that range). Unchanged runs are `Same`. `clean` = no `Clash`. For `both_added`, base is `""`.

**Keep-both copy name:** `<dir>/<stem>.from-github-<hhmm><.ext>` — `2026-11-16-1402-run.from-github-1530.md`, `assets/x-5f3a09c2.from-github-1530.jpg` — where `hhmm` is R's commit time in local time (so preview and apply agree), then `-2`, `-3`, … until the name is free in W, R and every other copy. Names without an extension get the suffix at the end.

### Apply (`apply_pull(id, resolutions, message)`)

1. Clean repo state (as in Preview step 1). Recompute the preview from the current L, R and W. **If its `id` differs from `id` → `Error::StaleReview`, nothing written.**
2. Validate: exactly one `Resolved` per overlap path and no others; each choice in that overlap's `choices`; `Merged` text only where allowed. Otherwise `Error::Invalid`. `message`: required (non-blank, else `Error::EmptyMessage`) iff plan is `Merge`; ignored otherwise.
3. Target working tree `T`: `T[p] = work[p]` for p ∉ C_R; `T[p] = remote[p]` for p ∈ C_R \ O; for p ∈ O by its resolution — `mine`: `work[p]` (absent if deleted here); `theirs`: `remote[p]` (absent if deleted on GitHub); `both`: `work[p]` at p and `remote[p]` at `copy`; `merged`: the given text's bytes exactly, with mine's file mode.
4. New head `N`:
   - `FastForward`: N = R.
   - `Rebase`: replay R..L onto R, oldest first (original author and message, the app's author as committer). Summary: `rebased` = commits replayed.
   - `Merge`: a commit with parents `[L, R]`, the given message (`message_prettify`d), the app's author as author and committer, and this tree: for each path, if `local[p] == base[p]` then `remote[p]`; else if `remote[p] == base[p]` or `local[p] == remote[p]` then `local[p]`; else (p ∈ O_c) `T[p]`, plus `T[copy]` for a Keep both. So **the merge commit records how committed clashes were resolved** (with whatever uncommitted text those files had), and nothing else that was uncommitted.
   - In every plan, `T` minus `tree(N)` stays as uncommitted changes. With no committed overlap this means: **resolutions of files changed only by uncommitted edits are left uncommitted on top of GitHub's commits**, just as those edits were before the pull. No commit is made that the user didn't write.
5. Plan every write and removal (paths where T ≠ W) with the existing safety checks; any problem → error, nothing written.
6. Write the **undo record** (below) before touching a file.
7. Write files (each new content staged and fsynced first, then the file re-checked against W immediately before it is replaced or removed). Then the index = `tree(N)`; then `refs/heads/<b>` = N, as a compare-and-swap against L (reflog `mdgitjournal: pull (fast-forward|rebase|merge)`). Files first: a failure part way leaves HEAD at L. **Any failure after the undo record is written rolls everything back** — every file this pull wrote that is still as it left them, the index bytes, and the previous undo record — and returns the error; a file that changed in that instant, or a branch the git CLI moved meanwhile, is `Error::StaleReview`. So `StaleReview` always means nothing was written.
8. Return the `SyncSummary` (also stored in the undo record).

**`pull(creds, author)`** = fetch; `preview_pull`; `None` → `UpToDate`; `automatic` → `apply_pull(&preview.id, &[], None, author)` → `Applied`; else `Review` (nothing written).

### Undo

Every applied pull (automatic or reviewed) replaces the single undo record:
- `.git/mdgitjournal/undo.json`, written atomically: `{ "version": 1, "branch", "before": <L oid | null>, "after": <N oid>, "remote": <R oid>, "files": { "<path>": { "before": <blob | null>, "beforeMode": <u32 | null>, "after": <blob | null> } }, "summary": <SyncSummary> }` — `files` holds every path the pull wrote or removed, including Keep both copies.
- `.git/mdgitjournal/undo-index`: a copy of `.git/index` from before the pull (absent if there was none).
- `refs/mdgitjournal/undo` → a commit (message `mdgitjournal: before pull (kept for Undo)`, parent L if any) whose tree holds the `before` blobs of `files`, so `git gc` keeps them. This ref is never pushed (push names `refs/heads/<b>` only).

`undo_last_sync()`:
1. No record → `Error::Invalid("There is no pull to undo.")`. Clean repo state required.
2. Refuse (`Error::UndoRefused`, nothing changed) if: HEAD is not on `branch`, or its tip ≠ `after` ("you've committed since"); or `after` ≠ `remote` and origin/<b> contains `after` ("it has been pushed"); or any recorded file is now neither its `after` nor its `before` content (message names the files: "You've changed X since the pull; Undo would overwrite it.").
3. Restore each file to `before` (removing it if `before` is null) with the safe write path, put `undo-index` back as `.git/index` (or reset the index to L if there was none), set `refs/heads/<b>` to `before` (or delete the ref, keeping HEAD on the unborn branch), and delete the record and the ref. Returns the restored paths.

The record is cleared by: a new apply, a `commit` (or `commit_and_push`) that commits, a `push` that pushes (`commit_and_push`'s too), `start_external_merge`, `remove_remote`. `last_sync()`/`SyncState.last_sync` report it only while the branch tip still equals `after`.

### Resolve with git (`start_external_merge(id, message)`), `merge_state`, `abort_merge`

`start_external_merge`:
1. Clean state, fresh `id` (else `Error::StaleReview`). With no uncommitted changes and plan `fast_forward` there is nothing for git to merge: `Error::Invalid("There is nothing to merge: Pull brings in GitHub's commits as they are.")` (the review never offers it then, since it has no overlaps).
2. Copy `.git/index` to `.git/mdgitjournal/merge-index`, so Abort restores it byte for byte. If there are uncommitted changes (W ≠ tree(L)): `message` is required (`Error::EmptyMessage`), and they are committed first as one ordinary commit `P` (all paths, `commit_all`), so git can merge them and Abort can bring them back exactly. With no uncommitted changes `message` is ignored.
3. Merge R into HEAD the way `git merge origin/<b>` does, with libgit2 (`Repository::merge`, checkout `safe` + standard `merge` conflict style, labels `HEAD` and `origin/<b>`, never overwriting ignored files): `MERGE_HEAD` = R, `MERGE_MSG` = `Merge remote-tracking branch 'origin/<b>'`, `MERGE_MODE`, `ORIG_HEAD` = HEAD (written by us; libgit2 doesn't), clean paths staged, conflicted paths with stage 1/2/3 index entries and conflict markers in the files. HEAD stays where it was. Unrelated histories are allowed (empty base). The checkout is all-or-nothing (no `allow_conflicts`, with which libgit2 would skip a blocked file and leave half a merge); if it fails, `P` is taken back and the index restored before the error is returned.
4. Write `.git/mdgitjournal/merge.json`: `{ "version": 1, "head": <L | null>, "precommit": <P | null>, "mergeHead": <R>, "startedAt": <ms> }`. Clear the undo record.

`merge_state()` = `Some` iff `repo.state()` is `Merge` (also for a merge the user started with the CLI; `started_by_app` = `merge.json` exists). `conflicted` = paths with conflict entries in the index, sorted.

While a merge is in progress, `preview_pull`, `pull`, `apply_pull`, `commit`, `commit_all`, `commit_and_push`, `push`, `undo_last_sync`, `connect` return `Error::MergeInProgress`. `fetch`, `status`, `diff`, `sync_state` work.

`abort_merge()` (`git merge --abort` semantics):
1. Not in a merge → `Error::Invalid("No merge is in progress.")`.
2. Every path that is unmerged in the index, or whose index entry differs from HEAD where the merged commit differs from HEAD too, goes back to HEAD's version in the working tree (removed if HEAD doesn't have it). Other files are left alone.
3. The index: for a merge the app started (`merge.json` describes this merge), `merge-index` is put back byte for byte; otherwise the reset paths get HEAD's entries. If `merge.json` has a `precommit` and HEAD is still that commit, the branch goes back to `head` (or unborn), leaving the working tree as step 2 left it — the edits are uncommitted again, exactly as before.
4. Remove `MERGE_HEAD`, `MERGE_MSG`, `MERGE_MODE` (`cleanup_state`); delete `merge.json` and `merge-index`. Returns the restored paths.

**Finishing with git:** the user resolves, `git add`s and `git commit`s (or `git merge --abort`s). The repository is then clean; the next `sync_state` reports `merge: None`, and the next locked operation deletes the stale `merge.json` and `merge-index`. A CLI abort leaves `P` as a commit; that was the user's own git command.

### Commit

`commit(message, paths, author)`:
- Blank message → `Error::EmptyMessage`; merge in progress → `Error::MergeInProgress`.
- The new tree is **HEAD's tree with exactly the selected paths taken from the working tree** (added/modified → its blob; deleted → removed; renamed → the new path added and `old_path` removed). Requested paths that are no longer changed are ignored; if nothing remains, `Ok(None)`. Works on an unborn branch.
- The real index afterwards: the selected paths match the new commit; every other entry is left as it was (so something staged with the CLI stays staged and uncommitted).
- Clears the undo record.

`commit_and_push(message, paths, …)`:
1. `commit(message, paths)` (so a blank message is `EmptyMessage`, even with no paths: pushing waiting commits alone is `push`). If nothing was committed and ahead == 0 → `NothingToCommit`.
2. Fetch. 3. If behind > 0 → `NeedsPull` (the commit stays local). 4. Push; a rejected ref in the push-update-reference callback means `NeedsPull` (rejection is detected via that callback, **not** only via the returned Err).

### Diff

`diff(path)`: `path` must be in `status()` (else `Error::Invalid`). Old = HEAD's blob at `old_path` (renamed) or `path`, absent for added; new = the working-tree file, absent for deleted. Text (UTF-8, no NUL, both sides) → hunks from `git2::Patch::from_buffers` with 3 context lines; otherwise `binary: true`, no hunks. A line whose only change is its line ending shows as removed and added.

### Fetch, push, connect, credentials (unchanged except where noted)

**`fetch`** updates `refs/remotes/origin/<branch>`, then returns `sync_state`. `ahead` and `behind` are counted HEAD vs `origin/<branch>`.

**`push`**: pushes `refs/heads/<b>:refs/heads/<b>`, never forces, sets upstream config `branch.<b>.remote/merge`, and clears the undo record (a pushed pull is no longer the user's to undo).

**`connect(url)`**
1. `set_remote`, then fetch.
2. If the remote is empty (no refs): push local history if there is at least one commit; otherwise just record the branch as "main". → `UpToDate`.
3. If the remote has a default branch (read from the remote HEAD symref, falling back to `main`, then `master`), adopt that branch name locally and run **`pull`'s logic** (preview, then apply if automatic, else `Review`). Unrelated histories are allowed: with no merge-base, the base is the empty tree, so a file on both sides with different content is a `both_added` overlap.
4. If connecting fails, the previous remote setting is restored. A `Review` is not a failure: the journal is connected and Pull is lit.

**Credentials callback**
- `Cred::userpass_plaintext(&creds.username, &creds.password)`, only if the URL libgit2 asks for starts with `creds.url_prefix`. Otherwise `Error::Auth`, naming the host refused and pointing at `remote.origin.pushurl` and `url.*.insteadOf`.
- Give up after 3 attempts, returning `Error::Auth`.
- The token never touches disk or `.git/config`.

### Staleness

`PullPreview.id` = `"<b>:<L oid or 0>:<R oid>:<W tree oid>"`. W's tree id hashes every non-ignored file's content, so any write to the journal (autosave, Finder, vim), a new untracked file, a commit, or a fetch that moved R changes it. `apply_pull` and `start_external_merge` recompute it; nothing is stored between preview and apply.

### Files mdgitjournal keeps in `.git/`

| path | what |
|---|---|
| `.git/mdgitjournal-last-fetch` | time of the last fetch (existing) |
| `.git/mdgitjournal/undo.json`, `.git/mdgitjournal/undo-index`, `refs/mdgitjournal/undo` | the undo record |
| `.git/mdgitjournal/merge.json`, `.git/mdgitjournal/merge-index` | a merge started by Resolve with git, and the index from before it |

All are local to the clone; plain git ignores them (`git gc` keeps the undo ref's objects).

**Tests** use `file://` bare remotes in temp dirs, with 2 clones simulating 2 devices.

## Crate: `mdgj-github`

```rust
pub const DEFAULT_CLIENT_ID: &str = "MDGJ_PLACEHOLDER_CLIENT_ID";  // overridden at build time by env!("MDGJ_GITHUB_CLIENT_ID") via option_env!
pub const DEFAULT_APP_SLUG: &str = "mdgitjournal";                 // overridden by option_env!("MDGJ_GITHUB_APP_SLUG")
pub fn client_id() -> &'static str; pub fn app_slug() -> &'static str; pub fn is_placeholder() -> bool;

pub trait Http: Send + Sync { fn post_form(&self, url, form: &[(&str,&str)], headers) -> Result<(u16, String)>; fn get(&self, url, headers) -> Result<(u16,String)>; }
pub struct ReqwestHttp;  // blocking reqwest + rustls; gives up after 10 s without a connection, 30 s in all

pub struct DeviceCode { pub device_code: String, pub user_code: String, pub verification_uri: String, pub expires_in: u64, pub interval: u64 }
pub enum Poll { Pending, SlowDown { interval: u64 }, Complete(Tokens), Expired, Denied }
pub struct Tokens { pub access_token: String, pub expires_at: Option<i64 /*unix s*/>, pub refresh_token: Option<String>, pub refresh_expires_at: Option<i64> }
pub struct User { pub login: String, pub id: u64, pub name: Option<String>, pub email: Option<String> }
pub struct RepoInfo { pub full_name: String, pub private: bool, pub clone_url: String, pub default_branch: String }

pub struct GitHub<H: Http = ReqwestHttp> { .. }
impl GitHub {
  pub fn start_device_flow(&self) -> Result<DeviceCode>;          // POST https://github.com/login/device/code  (client_id) Accept: application/json
  pub fn poll_device_flow(&self, dc: &DeviceCode) -> Result<Poll>; // POST https://github.com/login/oauth/access_token grant_type=urn:ietf:params:oauth:grant-type:device_code
  pub fn refresh(&self, refresh_token: &str) -> Result<Tokens>;    // grant_type=refresh_token (GitHub Apps: no client secret needed for device-flow apps)
  pub fn user(&self, token: &str) -> Result<User>;                 // GET https://api.github.com/user
  pub fn installation_repos(&self, token: &str) -> Result<Vec<RepoInfo>>; // GET /user/installations → for each: GET /user/installations/{id}/repositories (paginate)
  pub fn repo(&self, token: &str, full_name: &str) -> Result<RepoInfo>;   // GET /repos/{full_name} (used by PAT path)
  pub fn new_repo_url(name: &str) -> String;       // "https://github.com/new?name=journal&visibility=private"
  pub fn install_url(&self) -> String;             // "https://github.com/apps/<slug>/installations/new"
}
// token store
pub trait TokenStore: Send + Sync { fn load(&self) -> Result<Option<StoredAuth>>; fn save(&self, a: &StoredAuth) -> Result<()>; fn clear(&self) -> Result<()>; }
pub struct StoredAuth { pub kind: AuthKind /*github_app|pat*/, pub tokens: Tokens, pub login: Option<String> }
pub struct KeyringStore { service: "xyz.stephenfay.mdgitjournal", account: "github" , fallback_file: PathBuf } // keyring first; on keyring error fall back to fallback_file written 0600; remember which backend is used; expose fn backend(&self) -> &'static str ("keychain"|"secret-service"|"file"); pub fn file_only(path) -> Self: never uses the keyring (MDGJ_CONFIG_DIR)
pub struct TokenManager<S: TokenStore, H: Http> { .. }
impl TokenManager { pub fn access_token(&self) -> Result<Option<String>>; } // refreshes when expires_at - 5min < now; persists rotated refresh token; on refresh failure with bad_refresh_token → Err(Error::ReauthRequired)
```

Tests use a mock `Http`. No real network in unit tests.

## Tauri commands (`app/src-tauri`) ⇄ `app/src/lib/api.ts`

All commands are `async` and run their blocking work on `tauri::async_runtime::spawn_blocking`. **All git operations share one `Mutex`**, so they never overlap. `sync_state` and `diff` are read-only and go through a second repository handle, so they never wait for them: during one, `sync_state` reports the state from before it, and the operation sends `sync://state` when it ends; the commit sheet's diffs don't wait behind a fetch. libgit2 gives up after 15 s without a connection and after 30 s without data, so no stalled fetch or push holds the lock for long.

On opening a journal, the shell adds `.mdgj-tmp-*` (mdgj-core's temp files) and `.DS_Store` to `.git/info/exclude`, which belongs to the clone and is never committed. A file already tracked stays tracked.

Errors cross the boundary as `{ kind: string, message: string }`. Kinds: `auth`, `reauth`, `network`, `not_connected`, `empty_message`, `invalid`, `io`, `git`, **`stale_review`** (mdgj-git `StaleReview`), **`merge_in_progress`** (`MergeInProgress`), **`undo_refused`** (`UndoRefused`); mdgj-git `Invalid` maps to `invalid`. Only `auth`, `reauth`, `network`, `not_connected`, `io` and `git` become `SyncView.error` (the red dot); the others are answers for the sheet that asked.

Paths and environment:
- The journal lives in `app_data_dir()/journal`; on iOS, `document_dir()/journal`. **`MDGJ_JOURNAL_DIR`** overrides it.
- Settings live in `<config>/settings.json`, the fallback token file is `<config>/credentials.json`, where `<config>` = `app_config_dir()`, or **`MDGJ_CONFIG_DIR`** if set and non-empty (made absolute, created if missing). **With `MDGJ_CONFIG_DIR` set the token store is file-only** — `KeyringStore::file_only(<config>/credentials.json)`, a new mdgj-github constructor (`with_keyring(None, path)`; `backend()` = `"file"`) — so the keychain is never read or written and two instances never share a sign-in.
- **`MDGJ_DEVICE_NAME`**: if set and non-empty, `setup` sets the main window's title to `mdgitjournal — <name>` (Rust side, no new capability). Otherwise the title stays `mdgitjournal`.

| command | args | returns |
|---|---|---|
| `app_info` | – | `{ version, platform: "macos"\|"linux"\|"ios", journalDir, tokenBackend, clientIdPlaceholder: bool }` |
| `get_settings` / `set_settings` | `settings: Settings` | `Settings` |
| `list_entries` | – | `EntryMeta[]` |
| `read_entry` | `file` | `EntryContent` |
| `write_entry` | `file, content` | `{ hash }` |
| `create_entry` | `title` | `EntryMeta` (uses local now) |
| `rename_entry` | `file, newTitle` | `EntryMeta` |
| `delete_entry` | `file` | `null` |
| `search` | `query` | `SearchHit[]` (limit 200) |
| `import_image` | raw body = image bytes, header `x-entry-file` | `ImportedImage` |
| `sync_state` | – | `SyncView` |
| `fetch` | – | `SyncView` (no-op returning state when not connected) |
| `pull` | – | `PullResult` (fetches; applies at once only if `automatic`) |
| `pull_preview` | – | `PullResult` with status `up_to_date` or `review` — local only, **never applies**, a review even when automatic (used to reopen a fresh review after `stale_review`) |
| `apply_pull` | `id: string, resolutions: Resolved[], message: string \| null` | `{ view: SyncView, summary: SyncSummary }` |
| `undo_sync` | – | `{ view: SyncView, restored: string[] }` |
| `start_external_merge` | `id: string, message: string \| null` | `SyncView` |
| `abort_merge` | – | `{ view: SyncView, restored: string[] }` |
| `commit` | `message, paths: string[]` | `SyncView` (local-only commit; used when not connected) |
| `commit_and_push` | `message, paths: string[]` | `{ view: SyncView, result: "pushed"\|"needs_pull"\|"nothing" }`; a blank message with `paths: []` only pushes waiting commits |
| `diff` | `path` | `FileDiff` |
| `auth_status` | – | `{ signedIn, kind: "github_app"\|"pat"\|null, login: string\|null }` |
| `github_device_start` | – | `{ userCode, verificationUri, expiresIn, interval }` (the device code is kept in backend state) |
| `github_device_poll` | – | `{ status: "pending"\|"slow_down"\|"complete"\|"expired"\|"denied", login?: string }` |
| `github_set_pat` | `token` | `{ login }` (validates via `/user`) |
| `github_list_repos` | – | `RepoInfo[]` (PAT: returns [] → UI shows manual "owner/repo" input) |
| `github_connect` | `fullName` | `PullResult` |
| `github_disconnect` | `signOut: bool` | `SyncView` |
| `github_urls` | – | `{ newRepo, install }` |
| `third_party_notices` | – | `packaging/THIRD-PARTY-NOTICES.md` (markdown), built into the binary |

The TS mirror (`app/src/lib/types.ts`); Rust serializes exactly these shapes:

```ts
type SyncView = { connected: boolean; repo: string | null; branch: string; ahead: number; behind: number;
                  changes: FileChange[]; lastFetch: number | null; error: CommandError | null;
                  merge: MergeState | null;          // a git merge is in progress
                  lastSync: SyncSummary | null };    // the pull Undo would reverse; null when there is nothing to undo
type FileChange = { path: string; kind: "added" | "modified" | "deleted" | "renamed"; oldPath?: string };
type MergeState = { conflicted: string[]; startedByApp: boolean };

type PullResult =
  | { status: "up_to_date"; view: SyncView }
  | { status: "applied"; view: SyncView; summary: SyncSummary }
  | { status: "review"; view: SyncView; preview: PullPreview };

type PullPlan = "fast_forward" | "rebase" | "merge";
type CommitInfo = { id: string; message: string; author: string; time: number };
type PullPreview = { id: string; branch: string; plan: PullPlan; automatic: boolean;
                     incoming: CommitInfo[]; incomingCount: number; local: CommitInfo[]; localCount: number;
                     auto: FileChange[]; overlaps: Overlap[] };
type Choice = "mine" | "theirs" | "both" | "merged";
type Overlap = { path: string; kind: "both_modified" | "both_added" | "modified_deleted" | "deleted_modified";
                 binary: boolean; base: string | null; mine: string | null; theirs: string | null;
                 choices: Choice[]; copy: string | null; suggestion: MergeSuggestion | null };
type MergeSuggestion = { clean: boolean; chunks: MergeChunk[] };
type MergeChunk =
  | { kind: "same"; text: string }
  | { kind: "mine" | "theirs" | "both"; text: string; base: string }
  | { kind: "clash"; base: string; mine: string; theirs: string };
type Resolved = { path: string; choice: "mine" | "theirs" | "both" } | { path: string; choice: "merged"; text: string };
type SyncSummary = { at: number; plan: PullPlan; incoming: CommitInfo[]; incomingCount: number; rebased: number;
                     mergeCommit: string | null; files: FileChange[];
                     resolved: { path: string; choice: Choice; copy: string | null }[] };
type FileDiff = { path: string; oldPath: string | null; kind: FileChange["kind"]; binary: boolean;
                  hunks: { oldStart: number; oldLines: number; newStart: number; newLines: number;
                           lines: { kind: "context" | "added" | "removed"; text: string }[] }[] };
```

`SyncView` is built from `SyncState`, including its `merge` and `lastSync`. `PullResult` is one `#[serde(tag = "status", rename_all = "snake_case")]` enum in `commands.rs`; `PullOutcome::*.state` becomes `view`.

Events emitted by the backend:
- `sync://state` (SyncView), after any git operation.
- `entries://changed` (no payload), after `pull` (when applied), `apply_pull`, `undo_sync`, `start_external_merge`, `abort_merge` or `github_connect` changed files.

The commit author is taken from settings `authorName`/`authorEmail`. If those are unset and signed in, it's the GitHub `name` and `<id>+<login>@users.noreply.github.com`. If neither exists, it's `mdgitjournal <mdgitjournal@localhost>`.

### Two devices on one machine (`scripts/two-devices.sh`)

```
scripts/two-devices.sh [--dev] [--reset]
```
- Starts two instances as **Device A** and **Device B**, each with its own `MDGJ_JOURNAL_DIR=~/mdgitjournal-test-devices/<a|b>/journal`, `MDGJ_CONFIG_DIR=~/mdgitjournal-test-devices/<a|b>/config` and `MDGJ_DEVICE_NAME="Device A"`/`"Device B"`. The folders persist across runs, so sign-ins and journals survive restarts. Logs go to `~/mdgitjournal-test-devices/<a|b>/app.log`.
- The binary: `$MDGJ_BIN` if set; else on macOS `target/release/bundle/macos/mdgitjournal.app/Contents/MacOS/mdgitjournal` (run directly, so each instance gets its environment and a second instance really starts); on Linux `target/release/mdgitjournal` (`target/` is `$CARGO_TARGET_DIR` if set). If it is missing the script says how to build it (`pnpm -C app tauri build --bundles app`, or `--no-bundle` on Linux) and exits 1. `--dev` uses `target/debug/mdgitjournal`, which loads the UI from the dev server: the script checks that `localhost:1420` answers, and otherwise says to run `pnpm -C app dev`.
- `--reset` deletes `~/mdgitjournal-test-devices` (and only that) first, saying so, once the binary has been found.
- It stays in the foreground; Ctrl-C, closing the terminal, or closing both windows ends both instances. It never touches the real journal, settings or keychain: both variables are always set.
- POSIX `sh`, `set -eu`, shellcheck-clean. Each device signs in on its own (Settings › Connect GitHub; a PAT works) and connects to the same test repository.

### Frontend owns timing

- It runs `fetch` on launch, on window focus, and every `fetchIntervalMin` — except while the Sync review or the merge view is open (a fetch that moved R would only make the review stale).
- It flushes pending autosave **before** `pull`, `pull_preview`, `apply_pull`, `undo_sync`, `start_external_merge`, `abort_merge`, `commit`, `commit_and_push` and `github_connect`. A fetch in flight holds the git lock, so these wait for it first, and autosave keeps working meanwhile.
- While an operation that writes the working tree runs (`pull`, `apply_pull`, `undo_sync`, `start_external_merge`, `abort_merge`, `github_connect`), autosave is paused **and the editor is read-only** (`EditorHandle.setReadOnly`), so the buffer can't diverge from a file the operation rewrites. Afterwards it re-lists entries and, if the operation touched the open entry (`summary.files`, `restored`), reloads it (keeping the cursor), follows it if it was renamed, or closes it if the file is gone. Meanwhile the `entries://changed` handler only re-lists entries and leaves the open entry to that operation.
- Working-tree operations (open, create, rename, delete, the ones above, image imports) run one at a time, in order. An image import doesn't start once another entry is open.
- A `sync_state` reply that a git operation's report (its result, or `sync://state`) overtook is dropped and read again.
- On focus it re-lists entries, reloads the open entry from disk if the hash changed and the buffer is clean, and re-reads the sync state; while `sync.merge` is set it also re-reads the sync state every 5 s, so finishing the merge in a terminal is noticed.
- It flushes pending autosave when the page is hidden and before the window closes. If that save fails, the window stays open and says why, offering "Close anyway". On macOS, Quit (⌘Q) closes the window instead of ending the app at once, so it goes through the same save.

### Sync flows in the UI (`app/src/lib/app.svelte.ts`, `app/src/components/`)

- **Pull** (button, ⌘⇧L): `pull()`. `up_to_date` → toast "Up to date." `applied` → the **summary notice** (below). `review` → overlay `"review"`. While a working-tree operation runs, the Pull button says "Updating files…" (`SyncOp` `"apply"`).
- **Summary notice** (key `sync-summary`, stays until dismissed or replaced): "Pulled 3 commits from GitHub · 4 files changed" (and "· your 2 unpushed commits were placed after them" for `rebase`), with actions **Details** (opens the `SyncSummary` sheet: each commit's first line, author and relative time; each file with new/updated/deleted/renamed; each resolution) and **Undo**. A `Notice` has up to two `actions` (each may `keep` the notice open) and may be `persistent` (no dismiss button). Undo → `undo_sync()` → toast "Pull undone." `undo_refused` → an error notice with the backend's message. The sheet and notice go away when `sync.lastSync` stops describing that pull.
- **Sync review** (`SyncReview.svelte`, a wide `Sheet`; full screen on the phone):
  - Header: "3 commits from GitHub" (expandable list of `incoming`) and, if any, "2 unpushed commits here".
  - "Changed on both sides": one row per overlap — an entry by its title and date (as the sidebar shows it), any other file by path — with a note ("Changed here and on GitHub", "Changed here, deleted on GitHub", "Deleted here, changed on GitHub", "New here and on GitHub, with different text", "Image changed here and on GitHub", …) and a row of buttons for its `choices`, labelled **Keep mine · Keep GitHub's · Keep both · Merge…**; `modified_deleted` labels them **Keep mine · Delete**, `deleted_modified` **Keep deleted · Keep GitHub's**. Under the buttons, what the current choice does ("Yours stays; GitHub's is saved next to it as `<copy>`.", "Merged: 1 change of yours and 1 change from GitHub, no clashes.", …).
  - Default choice: `merged` with `joinChunks(suggestion.chunks)` when `suggestion.clean`; otherwise none.
  - **Merge…** opens the merge view for that file; **Done** there sets the choice to `merged` with the editor's text (and "Check the merge" reopens it as Done left it).
  - "N files changed on GitHub only: they apply automatically" (collapsed): `auto`, as new/updated/deleted/renamed.
  - A line says what Apply does to history (brings in GitHub's commits; places the unpushed commits after GitHub's; or joins them with a merge commit), and that resolved files stay as changes for the next commit when there is no merge commit.
  - When `plan === "merge"`: a commit message box, prefilled `mergeMessage(preview, choices)` = `"Merge from GitHub\n\n"` + one line per overlap (`- <path>: kept mine` / `kept mine (deleted)` / `kept GitHub's` / `kept GitHub's (deleted)` / `kept both (GitHub's saved as <copy>)` / `merged`), kept in step with the choices until the user edits it; required.
  - **Apply** (enabled once every overlap has a choice and, for `merge`, the message isn't blank; ⌘Enter) → `apply_pull(id, resolutions, message)` → close, summary notice. `stale_review` → `pull_preview()` reopens a fresh review (choices reset) with the banner "Something changed since you opened the review." inside it (a notice would sit under the modal); if there is nothing to pull any more, the review closes with a toast saying so. **Cancel** / Esc → close; no backend call.
  - **Resolve with git…** (a small link at the bottom, only when there are overlaps): a confirmation inside the sheet explains what happens (a standard `git merge origin/<b>`, conflict markers where changes clash, the files listed, Pull and Commit & Push off until it is committed); if `sync.changes` isn't empty it asks for a commit message for those changes (required), saying Abort merge makes them uncommitted again. → `start_external_merge(id, message)`. An `empty_message` answer (changes the review didn't know of) re-reads the sync state, so the message box appears.
- **Merge view** (`MergeView.svelte`, overlay `"merge"` on top of the review state; full window on desktop and phone, with a two-row header on the phone): title (the entry's title or path), a legend ("yours" / "GitHub's" tints), "N clashes left", **Undo** (the phone has no ⌘Z), **Done** (disabled while clashes remain; ⌘Enter) and **Cancel** (back to the review, choice unchanged). Esc cancels only while nothing has been edited, so edits aren't lost to a stray key.
- **Merge in progress** (`sync.merge` set): a persistent notice (key `merge`): "Merge in progress — finish it with git, or abort it. These files have conflict markers:" with the conflicted files (or "No conflicts are left: commit with git to finish."), and action **Abort merge**, which first confirms ("Files git merged go back to how they were before the merge; changes made to them since are discarded.") then `abort_merge()`. Pull and Commit & Push are disabled with the title "Merge in progress — finish it with git". When `sync.merge` becomes null the notice goes and a toast says "Merge finished." (or nothing, after Abort).
- **Commit sheet**: each change has a checkbox (all ticked when the sheet opens); the button is disabled with none ticked (unless push-only); `commit(message, ticked paths)` / `commit_and_push(message, ticked paths)`. Clicking a file's name toggles its diff underneath (`diff(path)`, fetched on demand): a plain unified view, removed lines in the danger tint and added in the accent tint, monospace, hunk headers as "…"; binary files say "image added/changed/deleted" (or "file …"). Renames show `old → new`.
- **Connect**: `github_connect` returns a `PullResult`. `applied` → the summary notice; `review` → the review opens at once, in front of Settings, with the banner "Connected to owner/repo. Some files differ here and on GitHub: choose what to keep."

## Editor module (`app/src/editor/index.ts`)

```ts
export type EditorMode = "source" | "styled";
export interface EditorOptions {
  doc: string; mode: EditorMode; vim: boolean; dark: boolean; mobile: boolean;
  onChange(doc: string): void;                 // every change (UI debounces autosave 400ms)
  onSave?(): void;                             // vim :w / Mod-s
  onImageFiles?(files: File[]): Promise<string[] /*relative paths*/>; // paste/drop/importImages; editor inserts ![](path) at the cursor (selection kept), one per line
  onImagesUnlinked?(paths: string[]): void;    // imports whose links were dropped because setDoc()/destroy() came first
  resolveImageUrl?(relPath: string): string;   // for styled-mode previews (convertFileSrc)
}
export interface EditorHandle {
  setDoc(doc: string, options?: { keepCursor?: boolean }): void; // replaces content, resets history (switching entries, reloads); keepCursor keeps the cursor's line and column and the scroll position (a reload after a pull, Undo or focus)
  getDoc(): string;
  setMode(m: EditorMode): void; setVim(on: boolean): void; setDark(on: boolean): void;
  setReadOnly(on: boolean): void;              // no edits from keys, vim, paste, drop, the phone toolbar or image imports while on; selection and scrolling still work. Links for images that finish importing meanwhile are inserted once it is off (or reported through onImagesUnlinked if the document was replaced)
  focus(): void; insertAtCursor(text: string): void; wrapSelection(before: string, after: string): void; // mobile toolbar
  importImages(files: File[]): void;           // the phone's photo picker: imports like a paste
  toggleLinePrefix(prefix: string): void;      // "# ", "- ", "- [ ] "
  destroy(): void;
}
export function createEditor(parent: HTMLElement, opts: EditorOptions): EditorHandle;
export function imageLink(relPath: string): string;  // "![](path)", or "![](<path>)" if the path has spaces or parentheses
```

**Lossless invariant:** the editor never alters text it was given. `getDoc()` after any sequence of `setMode`, `setVim`, `setDark` or `setReadOnly` returns exactly the string passed to `setDoc`, including `\r\n` and a missing final newline. A file with one kind of line break is edited as plain lines and written back with that break (`linebreaks.ts`), so vim, paste and IME can't mix in another; a file that already mixes them is split on `\n` and keeps its `\r` characters, and input never separates a line's `\r` from its `\n` (text typed or pasted at the line end goes before the `\r`; a new line or typing over the line end keeps it). Under vim, edits, selections and register reads never split a UTF-16 surrogate pair (`surrogates.ts`), text that arrives outside insert mode is dropped rather than typed, and each document gets a fresh jump list (`vimguards.ts`, `index.ts`). The UI never sends a buffer with half a surrogate pair to be saved; it names the line instead.

### Merge editor (`app/src/editor/merge.ts`, re-exported from `index.ts`)

Built on plain CodeMirror 6 (decorations, a `StateField` of ranges mapped through changes, block widgets) — **`@codemirror/merge` is not used**: its views compare two documents, while this is one result document with clash blocks, and it would add a dependency for little. No package is added.

```ts
export type MergeChunk =                      // = the backend's, see types.ts
  | { kind: "same"; text: string }
  | { kind: "mine" | "theirs" | "both"; text: string; base: string }
  | { kind: "clash"; base: string; mine: string; theirs: string };
export type ClashPick = "mine" | "theirs" | "both";
export interface MergeSnapshot { readonly json: unknown }  // a document and its picks, to reopen the editor where Done left it
export interface MergeEditorOptions {
  chunks: MergeChunk[];
  dark: boolean; mobile: boolean;
  onChange(doc: string, unresolved: number): void;   // after every edit, pick or undo
  snapshot?: MergeSnapshot;                          // reopen an earlier editor of the same chunks (undo history starts afresh: CodeMirror can't store a pick's undo as JSON)
}
export interface MergeEditorHandle {
  getDoc(): string;            // the result; unpicked clashes contribute nothing (Done stays disabled until unresolved() is 0)
  unresolved(): number;
  pick(clash: number, pick: ClashPick): void;        // what the block's buttons do; clash = 0-based, in document order
  undo(): boolean; redo(): boolean; undoDepth(): number; // as Mod-z / Mod-Shift-z; the merge view's Undo button (the phone has no Mod-z)
  snapshot(): MergeSnapshot;
  setDark(on: boolean): void; focus(): void; destroy(): void;
}
export function createMergeEditor(parent: HTMLElement, opts: MergeEditorOptions): MergeEditorHandle;
/** Pure: the merged text with these picks (missing = unpicked), or null if a clash is unpicked. */
export function joinChunks(chunks: MergeChunk[], picks?: readonly (ClashPick | null)[]): string | null;
/** "Use both": mine then theirs; if mine is non-empty and doesn't end in a line break, one is put between ("\r\n" if mine has one, else "\n"). */
export function bothText(mine: string, theirs: string): string;
```
- The initial document is `joinChunks` with every clash empty. `mine`, `theirs` and `both` chunks are marked (`.mdgj-merge-mine`, `.mdgj-merge-theirs`, `.mdgj-merge-both`: faint accent / faint `--color-theirs` / faint grey backgrounds, with a left bar), so the user can check what was taken from each side; a chunk whose text is empty (lines deleted) shows a thin marker whose title is the deleted text. The marks follow edits and disappear where their text is deleted.
- Each unpicked clash is a block widget at its position: "Yours" and "GitHub's" versions one above the other (monospace, wrapped), and **Use mine · Use GitHub's · Use both**. A pick inserts that text (`bothText` for both) at the slot and marks it `.mdgj-merge-picked`; the user may then edit it like any text, and undo (Mod-z) undoes the pick.
- Free editing everywhere else, Source-mode look (monospace, line wrapping; no line numbers on the phone, where the buttons are 44 px tall and the text 16 px), no vim, no image paste. The markdown language setup is shared with the main editor (`language.ts`). The same `lineBreaks`/`fileText` handling as the main editor, so `getDoc()` keeps `\r\n` and a missing final newline; picks are inserted with the document's line break.

## Mock backend (`app/src/lib/mock.ts`)

Used by `pnpm dev`, vitest and Playwright, so it must drive every flow above with the same shapes. It models: a remote history (commits with id, message, author, time and file changes), `head`/base and working-tree maps, overlaps by the definition in mdgj-git (mine = working tree), the plan (`fast_forward` with no unpushed commits; `rebase` with unpushed commits that don't share a path with incoming ones; else `merge`), automatic apply with a summary, Undo (refused if a touched file changed since), the review `id` (a fingerprint of head, remote tip and every file; any change makes `apply_pull` fail with `stale_review`), every resolution, `pull_preview`, `start_external_merge` (writes `<<<<<<< HEAD` / `=======` / `>>>>>>> origin/main` markers into conflicted files and sets `merge`), `abort_merge`, `commit`/`commit_and_push` with `paths`, and `diff`. Suggestions come from a small line-based diff3 in `app/src/lib/mockmerge.ts` (an LCS is fine; mock only).

`MockControls` (also `window.mdgjMock` in the browser):
```ts
remoteCommit(file: string, content: string | null /*null deletes*/, message?: string): void; // lands on GitHub; counts as behind after the next fetch
editOnDisk(file: string, content: string | null): void;  // an edit made outside the app (makes an open review stale)
finishMergeWithGit(): void;                              // as `git add -A && git commit` in a terminal: clears `merge`
failNext(method: keyof Api, kind: ErrorKind, message: string): void;
seedOverlaps(): { clean: string; clash: string };       // once connected: two entries changed on both sides, one merging cleanly, one clashing (for `pnpm dev` and screenshots)
readonly files: Map<string, string>; readonly calls: string[];
```
`remoteCommit` also works before connecting: the repository connect will use already has that history. The sample journal shares its first commit with the mock GitHub repository, which also has a phone entry, so connecting is `applied` (one commit, with Undo).

## Design tokens (UI + editor share `app/src/styles/tokens.css`)

| | Light | Dark |
|---|---|---|
| page background | `#fbfaf7` | `#1b1a18` |
| text | `#2b2a27` | `#e6e2d9` |
| muted | `#6b675f` | `#8f8a80` |
| syntax marks | `#b8b3a8` | `#5f5b54` |
| accent | `#3d6fb4` | `#7ea6e0` |
| danger | `#a63f33` | `#e07a6c` |
| GitHub's side in the merge view (`--color-theirs`) | `#9a6818` | `#d6ad62` |

Typography:
- Styled mode: `ui-serif, "New York", "Source Serif 4", "Noto Serif", Georgia, serif`, 18px / 1.6, `max-width: 31em` (about 66 characters; `66ch` would be 66 zeros, some 90 characters of serif prose).
- Source mode: `ui-monospace, "SF Mono", "JetBrains Mono", "DejaVu Sans Mono", monospace`, 14px.
- Chrome: `system-ui`, 13px.

Theme follows `prefers-color-scheme`, overridable via `settings.theme` (`data-theme` on `<html>`).

## Build & test

Per-OS prerequisites are in `README.md`. From the repository root:

```sh
pnpm -C app install --frozen-lockfile
cargo test --workspace                                  # or: cargo test -p mdgj-core
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
pnpm -C app check && pnpm -C app test && pnpm -C app build
pnpm -C app e2e                                         # Playwright + WebKit on the mock backend (app/e2e/)
pnpm -C app tauri dev                                   # MDGJ_JOURNAL_DIR=/tmp/j MDGJ_CONFIG_DIR=/tmp/c for a scratch journal and sign-in
scripts/two-devices.sh                                  # Device A and Device B on one machine (after `pnpm -C app tauri build --bundles app`)
pnpm -C app dev                                         # UI only, in a browser, on the mock backend
pnpm -C app tauri build                                 # bundles in target/release/bundle/; CI=true skips the .dmg's Finder layout (needed over SSH)
pnpm -C app tauri ios build --target aarch64-sim --debug  # after `tauri ios init`; needs full Xcode
```

- Tests don't use the network: the crates test against a mock `Http` and against `file://` remotes. End-to-end tests that need github.com, such as those against `dcxSt/mdgitjournal-test`, are `#[ignore]`d. Run them with `cargo test --workspace -- --ignored`.
- `MDGJ_GITHUB_CLIENT_ID` and `MDGJ_GITHUB_APP_SLUG` are read at compile time. The registered client ID goes in `.cargo/config.toml` `[env]`, and a real environment variable overrides it. `DEFAULT_CLIENT_ID` stays the placeholder.
- `MACOSX_DEPLOYMENT_TARGET` in `.cargo/config.toml` must equal `bundle.macOS.minimumSystemVersion`.
- The version is repeated across several files; `RELEASING.md` lists them.
- `packaging/THIRD-PARTY-NOTICES.md` is generated by `packaging/notices/generate.sh` and committed. The app shows it under Settings › About; the AUR packages install it in `/usr/share/licenses/`.
- The iOS Xcode project is generated by `tauri ios init` from `app/src-tauri/project.ios.yml` (`bundle.iOS.template`, a path from `app/`): tauri-cli's template plus libz and libiconv, which the vendored libgit2 needs.

CI (`.github/workflows/`):
- **`ci.yml`** runs on pushes to `main` and on pull requests:
  - Rust fmt, clippy and tests on Ubuntu and macOS, with `--locked`.
  - Frontend check, tests and build.
  - A check that `packaging/THIRD-PARTY-NOTICES.md` is up to date.
  - `tauri build` bundles on Ubuntu 24.04 and macOS (ad-hoc signed, signature checked), uploaded as artifacts. The build must leave the working tree unchanged.
  - An iOS Simulator build (`tauri ios init`, then `tauri ios build --target aarch64-sim`).
- **`release.yml`** runs on `v*` tags. It creates a draft GitHub Release with a universal macOS `.dmg` and the Linux AppImage, `.deb` and `.rpm`. Without the Apple secrets the `.dmg` is ad-hoc signed and not notarized.
- **`pages.yml`** deploys `site/` to GitHub Pages.
- All actions are pinned to commit SHAs; Dependabot (`.github/dependabot.yml`) proposes updates.

## Decisions (sync review)

Where the product decisions left a choice, the option that keeps the user most in control with the least UI:

1. **When Apply makes a merge commit.** Only when *committed* local work clashes with GitHub's (`O_c` non-empty), when local history has merge commits, or when an unpushed commit can't be replayed cleanly — i.e. exactly when history can't stay a line. Otherwise (no unpushed commits, or unpushed commits that don't touch the overlapping files) Apply fast-forwards or rebases and **leaves the resolved files as uncommitted changes**, which is what they were before the pull. This is `git stash; git pull --rebase; git stash pop` with the user choosing each hunk, and it means the app never commits text the user didn't ask to commit — with one exception: a file that has both an unpushed commit and uncommitted edits, and clashes, goes into the merge commit as resolved, since that commit is the record of the resolution.
2. **Overlap is per path, by content.** Both sides changed it, and the results differ. The same change on both sides flows. Renames are not followed (a remote rename is delete + add), which keeps every case one of four kinds.
3. **Clash = overlapping or touching hunks**, as git decides it; changes to different paragraphs (separated by at least one unchanged line) merge.
4. **Keep both** keeps yours at the path and saves GitHub's as `<stem>.from-github-<hhmm><ext>`, `hhmm` from GitHub's newest commit, so the name is the same in the review and on disk. Not offered for delete/modify (the "other version" is a deletion).
5. **No rebase of a reviewed pull's overlapping work, ever**: the review exists because something clashes; replaying it commit by commit would mean resolving it more than once.
6. **Undo** is one level, for every applied pull (automatic or reviewed), stored as a ref plus a small JSON in `.git/mdgitjournal/`. It ends at the next commit, push, pull, Resolve with git or disconnect, and refuses rather than overwrite a file edited since, or rewind history that was pushed.
7. **Resolve with git with uncommitted edits** asks for a commit message and commits them first. A real `git merge` can't start over uncommitted changes to the files it merges, and a stash would have to be popped onto the very files that conflict. Committing keeps the user's text in the merge (on the "ours" side, where they'll expect it), and the app's Abort undoes that commit again, so Abort is exact. An abort with the git CLI leaves the commit: that was the user's own command.
8. **Abort merge discards what was done to merged files since the merge started**, as `git merge --abort` does, after a confirmation. No extra state to compare.
9. **The editor is read-only while the working tree is being rewritten** (a second or two), so a buffer typed during a pull never has to be moved aside into a copy. Copies are never made without being asked for.
10. **The review is stateless**: its id is a fingerprint of branch, HEAD, origin/branch and the working tree's tree id. Nothing is kept between preview and apply; Cancel is free; staleness is exact.
11. **A merge commit is never automatic**: a review with no overlaps can appear (local merge commits, a commit that doesn't replay) and only asks for the message.
12. **Structural problems** (a symlink from the remote, names that differ only in case, a file where a folder is, an ignored local file where GitHub has one) still stop the pull with a message to fix it with git; they're rare, and git is the right tool.
13. **After `stale_review`** the fresh review comes from `pull_preview`, which never applies — "nothing written" holds even if the fresh state turned out to be one-sided.
14. **A merge started with the git CLI** is shown and abortable the same way; the app just didn't make it (`startedByApp: false`).
15. **Dependencies:** no new crate or package. Diffs use libgit2's (git's own algorithm, so the review and the commit sheet agree with `git diff`); the merge editor is CodeMirror decorations; `@codemirror/merge` was considered and not needed. mdgj-git uses the workspace's `serde_json` for its records (one line in `Cargo.lock`).
16. **Commit selection** builds the commit from HEAD's tree plus the ticked paths, not from the index, so something staged with the CLI never sneaks into a commit; it stays staged.
17. **`MDGJ_CONFIG_DIR` makes the token store file-only**, so two instances can never share or clobber one keychain item, and testing never touches the real keychain.
18. **A failed Apply puts everything back** (files it wrote, the index, the previous undo record) rather than leaving a partial pull for Undo, so `stale_review` always means nothing was written.

## Test scenarios for the sync review

**mdgj-git** (`crates/mdgj-git/tests/`, `file://` remotes, two `Device`s; byte comparisons with `fs::read`, refs via `git2`; where marked *CLI*, also check with the `git` binary if it is on `PATH`, skipping otherwise):
1. Up to date: `UpToDate`, nothing written (refs, index file bytes, files).
2. No overlap, fast-forward, with dirty unrelated files: applied at once; uncommitted bytes untouched; index = R; summary lists commits (message, author, time, newest first) and files (added, modified, deleted, renamed).
3. Undo after (2): branch, index (byte-identical copy) and every touched file back exactly — a file the pull added is removed, one it deleted is back; the undo ref and JSON are gone; `behind` is back.
4. Diverged, no overlap: rebased into a line (`rebased` = 2, parents linear, messages/authors kept); Undo restores the original commit ids.
5. Undo refused: a touched file edited since (named in the message, nothing changed); after a commit (record cleared → `Invalid`); after the branch was pushed.
6. Overlap (same file, different paragraphs; same line; adjacent lines): `Review`; working tree, index file, HEAD and branch refs byte-identical to before the pull (only `refs/remotes/origin/*` moved, from the fetch). `preview_pull` twice gives the same id. Cancel = no call = identical.
7. Each resolution, in the fast-forward case (resolutions left uncommitted, HEAD = R, `status` shows them) and the diverged case with committed overlap (merge commit with parents `[L, R]`, the given message, tree as specified; other uncommitted edits stay uncommitted; *CLI* `git log --graph` and `git status` are sane): `mine`, `theirs`, `both` (yours at the path, GitHub's at the `.from-github-hhmm` copy, both byte-exact; `-2` when taken), `merged` (exact bytes of the given text).
8. Diverged with an overlap only in uncommitted edits: plan `rebase`, no merge commit, resolution uncommitted.
9. Suggestions: different paragraphs → `clean` and the chunks join to the expected merge; same line or touching lines → one `Clash` with each side's text; identical change on both sides → `Both`; deletions → empty-text chunks with `base`; CRLF files and a missing final newline survive chunking and `merged` byte for byte.
10. Binary (two different PNGs at one `assets/` path): `binary`, no suggestion, choices `[mine, theirs, both]`; `both` copy keeps the extension; bytes exact. Two different photos added to one entry on two devices still both arrive with no review (different names).
11. `modified_deleted` and `deleted_modified`: choices `[mine, theirs]`; each outcome (kept / deleted / restored).
12. `both_added`: connect to a remote with unrelated history that has a file at a path the device also has, different content → `Review` from `connect`, still connected; same content → automatic.
13. Staleness: R moved by a fetch, an entry edited on disk, a new untracked file, a commit — each makes `apply_pull` and `start_external_merge` fail with `StaleReview` and write nothing; the fresh preview has a new id and applies.
14. Invalid resolutions (missing path, extra path, `both` on delete/modify, `merged` on binary) → `Invalid`, nothing written; plan `merge` without a message → `EmptyMessage`.
15. Local merge commits in B..L: plan `merge`, `automatic: false` even with no overlaps.
16. Resolve with git on a clean tree: `MERGE_HEAD` = R, conflicted paths have stage 1/2/3 entries and standard markers, HEAD unchanged; `merge_state` lists them; *CLI* `git status` says "unmerged paths". Pull, apply, commit, commit_and_push, push, undo, connect → `MergeInProgress`; fetch works.
17. Abort (clean start): refs, index and files byte-identical to before; no `MERGE_*` files. With uncommitted edits: needs a message; `P` committed; Abort brings HEAD back to L and the same `status()` and bytes as before.
18. Finish by "CLI" (resolve, stage, commit with parents `[HEAD, MERGE_HEAD]` and `cleanup_state` through `git2`, and *CLI* `git commit --no-edit` when available): `merge_state` None, `merge.json` removed by the next operation, push works, the other device pulls it automatically.
19. `commit(message, paths)`: only the ticked paths are in the commit, the rest still changed; a ticked rename removes the old path; an unticked staged entry stays staged; paths no longer changed are ignored; none left → `None`.
20. `diff`: modified text (hunks with 3 lines of context), added, deleted, renamed, binary.
21. Also: ignored local files never overwritten; remote symlink refused and nothing written; case-only collisions refused; push race → `NeedsPull`; `commit_and_push` needs pull when behind; credentials only to `url_prefix`.

**app/src-tauri** (unit tests; `tests/github_e2e.rs`):
1. Serialized shapes of every new response (`PullResult` × 3, `{view, summary}`, `{view, restored}`, `FileDiff`, `SyncView` with `merge` and `lastSync`), and `Resolved` deserializing from `{ path, choice: "merged", text }` and `{ path, choice: "mine" }`.
2. Error kinds `stale_review`, `merge_in_progress`, `undo_refused`, `invalid` from mdgj-git; none of them (nor `empty_message`) sets `SyncView.error`.
3. `MDGJ_CONFIG_DIR`: settings and `credentials.json` land there; `tokenBackend` is `"file"`; the keyring is never built. `MDGJ_DEVICE_NAME` → title `mdgitjournal — Device A` (a pure `window_title(Option<&str>)` helper, tested).
4. The capability test still passes unchanged.
5. `github_e2e` (ignored, real GitHub): devices use `MDGJ_CONFIG_DIR`; A and B edit different paragraphs of one entry → B's pull is a `review` with a clean suggestion → `apply_pull` merged → push → A's pull is `applied` → `undo_sync` → pull again; and a one-sided pull is applied at once.
6. `scripts/two-devices.sh`: `sh -n`, shellcheck if installed, `--help`; a dry run with `MDGJ_BIN=/usr/bin/true` exits 0 and creates the two folders.

**app/src** (vitest with the mock; Playwright WebKit in `app/e2e/`):
1. `sync.ts`: buttons disabled with the merge title while `merge` is set.
2. Review logic (`app/src/lib/review.svelte.ts`): defaults (clean → merged, clash → none), Apply enabled only when complete (and with a message for `merge`), `mergeMessage`, resolutions sent exactly.
3. Merge editor: `joinChunks`, `bothText` (with and without final newline, CRLF), `pick` per kind, `unresolved` count, undo of a pick, editing around clash slots keeps them in place, `getDoc` lossless for CRLF and no final newline, never a conflict marker.
4. Flows on the mock: one-sided pull → summary notice → Undo → files back; Undo refused after an edit; overlap → review, Cancel leaves `mock.files` identical; each choice through Apply; stale (`editOnDisk` while open) → message and a fresh review; Resolve with git → notice, buttons disabled, Abort (confirmed) restores; `finishMergeWithGit` → notice gone; connect → review; the editor is read-only during a pull; fetch doesn't run while the review is open.
5. Commit sheet: all ticked; untick one → only the rest committed (`mock.calls` / args); none ticked → disabled; a diff opens and closes; binary says "image changed".
6. e2e: the flows of 4 and 5 in the desktop layout, the review and the merge view at the phone size, and screenshots of the review and merge view (light and dark) in `screenshots.spec.ts`.
