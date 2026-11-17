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
crates/mdgj-git/           # libgit2 wrapper: init, status, commit, fetch, pull (merge + keep-both), push, connect
crates/mdgj-github/        # GitHub App device flow, token refresh, token store (keyring + file fallback), REST calls
app/                       # pnpm package: Vite + Svelte 5 + TypeScript
app/src/editor/            # CodeMirror 6 editor (source + styled), vim, image paste hooks
app/src/lib/api.ts         # typed wrappers around Tauri invoke + a mock backend for browser dev/tests
app/src/lib/types.ts       # TS mirror of the command types below
app/src/...                # Svelte UI
app/src-tauri/             # Tauri 2 shell: state, commands (thin glue over the crates), events
packaging/aur/  site/  docs/  .github/workflows/
```

The three crates **do not depend on each other**. Only `app/src-tauri` depends on all three. Each crate has its own unit and integration tests (`cargo test -p <crate>`).

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
  pub fn import_image(&self, entry_file: &str, bytes: &[u8], opts: ImageOpts) -> Result<ImportedImage>; // see images.rs; writes assets/<entry-stem>-N.jpg
}
pub struct EntryMeta { pub file: String, pub date: Option<String> /*"2026-11-16"*/, pub time: Option<String> /*"14:02"*/, pub title: String /*slug, or file stem for Other*/, pub mtime: i64 /*unix ms*/, pub size: u64, pub words: Option<u32> }
pub struct EntryContent { pub file: String, pub content: String, pub hash: String }
pub struct SearchHit { pub file: String, pub title: String, pub line: u32 /*1-based, 0 = filename match*/, pub snippet: String, pub match_start: u32, pub match_end: u32 /*char offsets within snippet*/ }
pub fn word_count(text: &str) -> u32;  // unicode words, ignores markdown punctuation like '#', '-', '*'

// images.rs
pub struct ImageOpts { pub max_edge: u32 /*1024*/, pub quality: u8 /*60*/ }
pub struct ImportedImage { pub path: String /*"assets/2026-11-16-1402-morning-run-1.jpg"*/, pub bytes: u64, pub width: u32, pub height: u32 }
pub fn compress_image(bytes: &[u8], opts: ImageOpts) -> Result<(Vec<u8>, u32, u32)>; // decode jpeg/png/webp/gif(first frame); apply EXIF orientation; strip ALL metadata; Lanczos3 downscale to max_edge (never upscale); flatten alpha onto white; encode baseline/progressive JPEG at quality

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

The crate uses libgit2 via `git2 = { default-features = false, features = ["https", "vendored-libgit2", "vendored-openssl"] }`. OpenSSL is built from source on every target because libgit2-sys links it on all Unix targets, though macOS and iOS use Apple's TLS. No SSH.

```rust
pub struct Creds { pub username: String /*"x-access-token"*/, pub password: String /*token*/ }  // None for local/file:// remotes
pub struct Author { pub name: String, pub email: String }

pub struct Repo { /* wraps git2::Repository */ }
impl Repo {
  pub fn open_or_init(path: &Path) -> Result<Repo>;     // init with initial branch "main" if not a repo
  pub fn status(&self) -> Result<Vec<FileChange>>;       // working tree vs HEAD, includes untracked (respects .gitignore), renames detected
  pub fn commit_all(&self, message: &str, author: &Author) -> Result<Option<String /*oid*/>>; // stage everything (adds, mods, deletes); None if nothing to commit; works on unborn branch; rejects empty/blank message with Error::EmptyMessage
  pub fn remote_url(&self) -> Option<String>;
  pub fn set_remote(&self, url: &str) -> Result<()>;      // remote "origin"; URL stored WITHOUT credentials
  pub fn remove_remote(&self) -> Result<()>;
  pub fn fetch(&self, creds: Option<&Creds>) -> Result<SyncState>;
  pub fn sync_state(&self) -> Result<SyncState>;          // local only, no network
  pub fn pull(&self, creds: Option<&Creds>, author: &Author) -> Result<PullOutcome>;
  pub fn push(&self, creds: Option<&Creds>) -> Result<PushOutcome>;
  pub fn commit_and_push(&self, message: &str, author: &Author, creds: Option<&Creds>) -> Result<CommitPushOutcome>;
  pub fn connect(&self, url: &str, creds: Option<&Creds>, author: &Author) -> Result<PullOutcome>; // see below
}
pub struct FileChange { pub path: String, pub kind: ChangeKind /*added|modified|deleted|renamed*/, pub old_path: Option<String> }
pub struct SyncState { pub has_remote: bool, pub branch: String, pub ahead: u32, pub behind: u32, pub changes: Vec<FileChange>, pub last_fetch: Option<i64 /*unix ms*/> }
pub struct Conflict { pub path: String, pub copy: Option<String> /*where OUR version was saved, None if ours was deleted*/ }
pub struct PullOutcome { pub state: SyncState, pub updated: Vec<String> /*paths changed in working tree by the pull*/, pub merged: Vec<String> /*clean 3-way merges*/, pub conflicts: Vec<Conflict>, pub rebased: u32 }
pub enum PushOutcome { Pushed { state: SyncState }, NothingToPush { state: SyncState }, Rejected { state: SyncState } /*non-ff: remote moved; caller should show Pull*/ }
pub enum CommitPushOutcome { Pushed { state: SyncState }, NeedsPull { state: SyncState } /*committed locally, remote ahead*/, NothingToCommit { state: SyncState } }
pub enum Error { EmptyMessage, Auth(String), Network(String), NotConnected, Git(String), Io(String) }  // Display messages are user-readable
```

### Sync semantics. Never lose text.

**`fetch`** updates `refs/remotes/origin/<branch>`, then returns `sync_state`. `ahead` and `behind` are counted HEAD vs `origin/<branch>`.

**`pull`**
1. Fetch.
2. Snapshot the working tree as a tree `W`. This includes untracked files and excludes ignored ones; it uses an in-memory index and touches neither the real index nor HEAD.
3. Let `L` = HEAD, `R` = origin/branch, `B` = merge-base(L, R).
4. Compute the new head `N`:
   - If L == R or R is an ancestor of L: N = L.
   - If L is an ancestor of R (or L is unborn): N = R.
   - Otherwise, **rebase** each local commit in B..L onto R using `merge_trees`. Commit conflicts are resolved with the keep-both rule and committed with the same message and author. If the local history contains merge commits, make one merge commit instead.
5. Merge the working-tree changes onto N with `merge_trees(ancestor = tree(L), ours = W, theirs = tree(N))`. Conflicts use the **keep-both** rule:
   - Theirs (remote) is written at `path`.
   - Ours is written at `<stem>.conflict-<hhmm>.md` (local time; add `-2` and so on if taken; non-md files get `<stem>.conflict-<hhmm>.<ext>`).
   - If one side deleted the file and the other modified it, keep the modified version at `path`. No copy is needed.
6. Write the result into the working directory, set HEAD/branch to N, and reset the index to N. Local edits stay as uncommitted changes; conflict copies appear as new untracked files.
7. Return `updated`, `merged` and `conflicts`.

**`commit_and_push(msg)`**
1. Commit all changes. If there's nothing to commit and ahead == 0, return `NothingToCommit`.
2. Fetch.
3. If behind > 0, return `NeedsPull`. The commit stays local.
4. Push. A rejected ref in the push-update-reference callback means `NeedsPull` (rejection is detected via that callback, **not** only via the returned Err).

**`push`**
- Pushes `refs/heads/<b>:refs/heads/<b>`.
- Sets upstream config `branch.<b>.remote/merge`.

**`connect(url)`**
1. `set_remote`, then fetch.
2. If the remote is empty (no refs): push local history (if HEAD is unborn, create an empty initial commit first — no, only push if there is at least one commit; otherwise just record the branch as "main").
3. If the remote has a default branch (read from the remote HEAD symref, falling back to `main`, then `master`), adopt that branch name locally and run the `pull` algorithm. Unrelated histories are allowed: with no merge-base, use the empty tree as the base.

**Credentials callback**
- `Cred::userpass_plaintext(&creds.username, &creds.password)`.
- Give up after 3 attempts, returning `Error::Auth`.
- The token never touches disk or `.git/config`.

**Tests** use `file://` bare remotes in temp dirs, with 2 clones simulating 2 devices.

## Crate: `mdgj-github`

```rust
pub const DEFAULT_CLIENT_ID: &str = "MDGJ_PLACEHOLDER_CLIENT_ID";  // overridden at build time by env!("MDGJ_GITHUB_CLIENT_ID") via option_env!
pub const DEFAULT_APP_SLUG: &str = "mdgitjournal";                 // overridden by option_env!("MDGJ_GITHUB_APP_SLUG")
pub fn client_id() -> &'static str; pub fn app_slug() -> &'static str; pub fn is_placeholder() -> bool;

pub trait Http: Send + Sync { fn post_form(&self, url, form: &[(&str,&str)], headers) -> Result<(u16, String)>; fn get(&self, url, headers) -> Result<(u16,String)>; }
pub struct ReqwestHttp;  // blocking reqwest + rustls

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
pub struct KeyringStore { service: "xyz.stephenfay.mdgitjournal", account: "github" , fallback_file: PathBuf } // keyring first; on keyring error fall back to fallback_file written 0600; remember which backend is used; expose fn backend(&self) -> &'static str ("keychain"|"secret-service"|"file")
pub struct TokenManager<S: TokenStore, H: Http> { .. }
impl TokenManager { pub fn access_token(&self) -> Result<Option<String>>; } // refreshes when expires_at - 5min < now; persists rotated refresh token; on refresh failure with bad_refresh_token → Err(Error::ReauthRequired)
```

Tests use a mock `Http`. No real network in unit tests.

## Tauri commands (`app/src-tauri`) ⇄ `app/src/lib/api.ts`

All commands are `async` and run their blocking work on `tauri::async_runtime::spawn_blocking`. **All git operations share one `Mutex`**, so they never overlap.

Errors cross the boundary as `{ kind: string, message: string }`. Kinds: `auth`, `reauth`, `network`, `not_connected`, `empty_message`, `invalid`, `io`, `git`.

Paths:
- The journal lives in `app_data_dir()/journal`; on iOS, `document_dir()/journal`. The env var `MDGJ_JOURNAL_DIR` overrides it.
- Settings live in `app_config_dir()/settings.json`.
- The fallback token file is `app_config_dir()/credentials.json`.

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
| `pull` | – | `{ view: SyncView, updated, merged, conflicts }` |
| `commit` | `message` | `SyncView` (local-only commit; used when not connected) |
| `commit_and_push` | `message` | `{ view: SyncView, result: "pushed"\|"needs_pull"\|"nothing" }` |
| `auth_status` | – | `{ signedIn, kind: "github_app"\|"pat"\|null, login: string\|null }` |
| `github_device_start` | – | `{ userCode, verificationUri, expiresIn, interval }` (the device code is kept in backend state) |
| `github_device_poll` | – | `{ status: "pending"\|"slow_down"\|"complete"\|"expired"\|"denied", login?: string }` |
| `github_set_pat` | `token` | `{ login }` (validates via `/user`) |
| `github_list_repos` | – | `RepoInfo[]` (PAT: returns [] → UI shows manual "owner/repo" input) |
| `github_connect` | `fullName` | `{ view: SyncView, updated, merged, conflicts }` |
| `github_disconnect` | `signOut: bool` | `SyncView` |
| `github_urls` | – | `{ newRepo, install }` |

```ts
type SyncView = { connected: boolean; repo: string | null; branch: string; ahead: number; behind: number;
                  changes: { path: string; kind: "added"|"modified"|"deleted"|"renamed"; oldPath?: string }[];
                  lastFetch: number | null; error: { kind: string; message: string } | null };
```

Events emitted by the backend:
- `sync://state` (SyncView), after any git operation.
- `entries://changed` (no payload), after a pull changes files.

The commit author is taken from settings `authorName`/`authorEmail`. If those are unset and signed in, it's the GitHub `name` and `<id>+<login>@users.noreply.github.com`. If neither exists, it's `mdgitjournal <mdgitjournal@localhost>`.

**Frontend owns timing:**
- It runs `fetch` on launch, on window focus, and every `fetchIntervalMin`.
- It flushes pending autosave **before** `pull`, `commit` and `commit_and_push`.
- After `pull` it reloads the open entry if it is in `updated` or `merged`.
- On focus it re-lists entries, and reloads the open entry from disk if the hash changed and the buffer is clean.
- It flushes pending autosave when the page is hidden and before the window closes. If that save fails, the window stays open and says why, offering "Close anyway". On macOS, Quit (⌘Q) closes the window instead of ending the app at once, so it goes through the same save.

## Editor module (`app/src/editor/index.ts`)

```ts
export type EditorMode = "source" | "styled";
export interface EditorOptions {
  doc: string; mode: EditorMode; vim: boolean; dark: boolean; mobile: boolean;
  onChange(doc: string): void;                 // every change (UI debounces autosave 400ms)
  onSave?(): void;                             // vim :w / Mod-s
  onImageFiles?(files: File[]): Promise<string[] /*relative paths*/>; // paste/drop; editor inserts ![](path) at cursor for each
  resolveImageUrl?(relPath: string): string;   // for styled-mode previews (convertFileSrc)
}
export interface EditorHandle {
  setDoc(doc: string): void;   // replaces content, resets history (used when switching entries / reload)
  getDoc(): string;
  setMode(m: EditorMode): void; setVim(on: boolean): void; setDark(on: boolean): void;
  focus(): void; insertAtCursor(text: string): void; wrapSelection(before: string, after: string): void; // mobile toolbar
  toggleLinePrefix(prefix: string): void;      // "# ", "- ", "- [ ] "
  destroy(): void;
}
export function createEditor(parent: HTMLElement, opts: EditorOptions): EditorHandle;
export function imageLink(relPath: string): string;  // "![](path)", or "![](<path>)" if the path has spaces or parentheses
```

**Lossless invariant:** the editor never alters text it was given. `getDoc()` after any sequence of `setMode`, `setVim` or `setDark` returns exactly the string passed to `setDoc`, including `\r\n` and a missing final newline. A file with one kind of line break is edited as plain lines and written back with that break (`linebreaks.ts`), so vim, paste and IME can't mix in another; a file that already mixes them is split on `\n` and keeps its `\r` characters.

## Design tokens (UI + editor share `app/src/styles/tokens.css`)

| | Light | Dark |
|---|---|---|
| page background | `#fbfaf7` | `#1b1a18` |
| text | `#2b2a27` | `#e6e2d9` |
| muted | `#8a867d` | `#8f8a80` |
| syntax marks | `#b8b3a8` | `#5f5b54` |
| accent | `#3d6fb4` | `#7ea6e0` |
| danger | `#b5483b` | — |

Typography:
- Styled mode: `ui-serif, "New York", "Source Serif 4", "Noto Serif", Georgia, serif`, 18px / 1.6, `max-width: 28em` (about 66 characters; `66ch` would be 66 zeros, some 90 characters of serif prose).
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
pnpm -C app tauri dev                                   # MDGJ_JOURNAL_DIR=/tmp/j for a scratch journal
pnpm -C app dev                                         # UI only, in a browser, on the mock backend
pnpm -C app tauri build                                 # bundles in target/release/bundle/
pnpm -C app tauri ios build --target aarch64-sim --debug  # after `tauri ios init`; needs full Xcode
```

- Tests don't use the network: the crates test against a mock `Http` and against `file://` remotes. End-to-end tests that need github.com, such as those against `dcxSt/mdgitjournal-test`, are `#[ignore]`d. Run them with `cargo test --workspace -- --ignored`.
- `MDGJ_GITHUB_CLIENT_ID` and `MDGJ_GITHUB_APP_SLUG` are read at compile time. The registered client ID goes in `.cargo/config.toml` `[env]`, and a real environment variable overrides it. `DEFAULT_CLIENT_ID` stays the placeholder.
- `MACOSX_DEPLOYMENT_TARGET` in `.cargo/config.toml` must equal `bundle.macOS.minimumSystemVersion`.
- The version is repeated across several files; `RELEASING.md` lists them.

CI (`.github/workflows/`):
- **`ci.yml`** runs on pushes to `main` and on pull requests:
  - Rust fmt, clippy and tests on Ubuntu and macOS.
  - Frontend check, tests and build.
  - Unsigned `tauri build` bundles on Ubuntu 22.04 and macOS, uploaded as artifacts.
  - An iOS Simulator build, allowed to fail until `gen/apple` is committed.
- **`release.yml`** runs on `v*` tags. It creates a draft GitHub Release with a universal macOS `.dmg` and the Linux AppImage, `.deb` and `.rpm`.
- **`pages.yml`** deploys `site/` to GitHub Pages.
