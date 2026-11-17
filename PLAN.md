# mdgitjournal — Plan (v4 — final, ready for ultracode)

A minimal, fast markdown journal for **macOS, Linux and iPhone**. Out of the box it is a local markdown editor with built-in git history. You can **optionally** connect one GitHub repo (sign in once, from inside the app) to sync between devices.

Personal project of `dcxSt` (Stephen Fay). Bundle ID `xyz.stephenfay.mdgitjournal`, homepage `mdgitjournal.stephenfay.xyz`.

Changes since v3: renamed to **mdgitjournal** · you write every commit message · serif, "wordcell" typography · Apple signing via your brother's developer team · repo public under MIT.

Changes since v2:
- Rich mode keeps all markdown syntax visible; it only styles it.
- The app works offline with no sign-in.
- Only two sync buttons: **Pull** and **Commit & Push**.
- The GitHub App is scoped to a single repo.
- Images are compressed aggressively to small JPEGs.
- New: dark/light theme, search, word count.

---

## 0. Design principles

1. **The `.md` file is the only source of truth.** No database, no front-matter, no hidden metadata. The app is a view over a git working tree.
2. **Never change bytes you didn't touch.** Opening a file, switching modes, or saving without edits produces zero diff. Tests enforce this.
3. **Nothing happens behind your back.** No auto-commit and no auto-push. The only background network activity is `fetch`.
4. **Plain git underneath.** The journal is an ordinary repo, so the `git` CLI works on it at any time.
5. **Works without an account.** GitHub is an add-on, not a requirement.
6. **Small and fast.** One Rust binary plus the system WebView. No Electron, no telemetry, no analytics.

## 1. Stack

- **Tauri 2**: one codebase for macOS, Linux and iOS. Rust backend; the UI runs in the system WebView (WKWebView on mac/iOS, WebKitGTK on Linux).
- **`mdgj-*` crates (Rust crate):**
  - git via `git2` (libgit2), HTTPS only, vendored. It uses Apple TLS on mac/iOS and vendored OpenSSL on Linux. No SSH.
  - GitHub device flow and token refresh via `reqwest` + `rustls`.
  - Token storage via `keyring`: Keychain on mac/iOS, Secret Service on Linux, falling back to a `0600` file under `$XDG_CONFIG_HOME/mdgitjournal/`.
  - Image pipeline with the `image` crate (pure Rust, so it cross-compiles to iOS without fuss).
- **UI:** TypeScript + Svelte 5 + Vite, built with pnpm.
- **Editor:** CodeMirror 6, one engine for both modes. Optional Vim via `@replit/codemirror-vim`.

## 2. Editor: two views of the same text

Both modes edit the **same buffer**. Switching between them is instant and keeps the cursor, scroll position and undo history. Neither mode ever parses and re-serializes the file, so it is **lossless by construction**.

| | **Source** (desktop default) | **Styled** (phone default) |
|---|---|---|
| Font | monospace | **serif**, ~66-character column, generous line height (see §7 Typography) |
| Line numbers | yes | no |
| Markdown syntax | visible, syntax-coloured | **visible but dimmed**, e.g. `# ` shown faint grey |
| Headings | same size | `# Title` is drawn large, `## Sub` medium |
| `**bold**`, `_italic_`, `` `code` ``, `~~strike~~` | coloured | rendered bold/italic/mono/struck, **asterisks kept** but faint |
| `- [ ] task` | text | text, and the `[ ]` is also clickable; a click rewrites exactly one byte |
| `![](assets/x.jpg)` | text | text, plus a preview of the image under the line |
| `> quote`, code fences | coloured | indented bar / shaded block |
| Vim keys | optional (off by default) | optional (off by default, desktop only) |

All styling comes from CodeMirror's markdown syntax tree as decorations, not a separate document model.

Saving:
- Autosave runs 400 ms after you stop typing.
- Writes are atomic (temp file, then rename).
- Line endings and the trailing newline are preserved as found.

## 3. Data model

- **One journal per device.** It is a git repo at:
  - macOS: `~/Library/Application Support/mdgitjournal/journal/`
  - Linux: `$XDG_DATA_HOME/mdgitjournal/journal/`
  - iOS: the app's Documents folder, visible in the Files app
- On first launch the app runs `git init` there, so you get version history immediately, even offline and without an account.
- **Flat folder.** Entries are named `yyyy-mm-dd-hhmm-title.md`, e.g. `2026-11-16-1402-morning-run.md`:
  - The title is slugified: lowercase, `[a-z0-9-]`, ASCII-folded, runs of `-` collapsed.
  - The date and time are local time at creation and **never change**.
  - **Renaming:** edit the title in the entry header. Only the `-title` part of the filename changes, as a git rename.
  - If two entries would get the same name, the second gets `-2`.
  - New entries are **empty**. There is no template.
- **Images** go in `assets/`, the only subfolder (see §6).
- **Sidebar:** entries are parsed from filenames, sorted newest first and grouped by month. Non-matching `.md` files appear under "Other".
- Per-device settings (theme override, Vim, default mode, fetch interval, image size) live outside the repo.

## 4. GitHub (optional, from inside the app)

The app is fully usable with no sign-in. Settings → **Connect GitHub**:

1. **Sign in with GitHub** using the OAuth **device flow**:
   - The app shows a code such as `ABCD-1234` and an **Open GitHub** button.
   - You approve in the browser. No client secret is needed and no local server runs.
   - This works the same on mac, Linux and iPhone.
2. **Install the mdgitjournal GitHub App** on one repo. If you don't have a repo yet, a button opens `github.com/new?name=journal&visibility=private`; after creating it you install the app on it. Doing it this way means the app never needs permission to create repos.
   - **Advanced:** paste a fine-grained personal access token and type `owner/repo` instead. This is for source builds without a registered GitHub App, and it's also how to test before the App exists.
   - Permissions: *Contents: read & write* and *Metadata: read*, on **that repo only**.
3. **Connect.** The app fetches the remote:
   - If the remote is empty, it pushes your local history.
   - If the remote has history, it rebases your local commits on top of it. Unrelated histories are fine because filenames are unique.
   - Any filename clash is resolved by the keep-both rule in §5.
4. **Tokens:**
   - The user token expires after 8 hours and is refreshed silently.
   - The refresh token lasts 6 months and rotates on every use, so you **sign in once per device**. You only need to sign in again if you don't open the app for 6 months, and it's one tap.
   - Tokens are stored in the OS keychain.
   - The token is passed to libgit2 in memory only. It is never written to `.git/config` or into the remote URL.
5. **Disconnect** removes the token and the remote. Local files stay.

The GitHub App is registered by you, once, at github.com/settings/apps, using the exact settings in `docs/github-app.md`:
- Homepage: `https://mdgitjournal.stephenfay.xyz`
- Device flow enabled
- Expiring tokens on

Its client ID is public and is compiled in. Until you register it, the code uses a **placeholder** (`MDGJ_GITHUB_CLIENT_ID`). Source builders can supply their own.

## 5. Sync: two buttons

Without GitHub connected there is one button, **Commit**. It makes a local commit with a message.

With GitHub connected:

| Button | State |
|---|---|
| **⬇ Pull** | grey when up to date · **accent colour + badge** when the remote has new commits · spinner while busy · red dot when an error happened (tap to see it) |
| **⬆ Commit & Push** | grey when nothing has changed · highlighted with a badge showing the number of changed files · **disabled with the hint "Pull first"** when the remote is ahead |

The app runs `fetch` on launch, when it regains focus, and every 5 minutes (configurable or off). Pull-to-refresh on the phone also runs `fetch`.

**Commit & Push:**
1. A small sheet opens with the changed files and an **empty** message box that has focus. You write the message; it can't be empty. `⌘Enter`/`Enter` or the Commit & Push button confirms, and `Esc` cancels.
2. The app fetches immediately before pushing.
3. If something arrived in the meantime, nothing is pushed. The Pull button lights up. The commit is already made locally, so nothing is lost.
4. If the push is rejected (a race with another device), the outcome is the same as step 3.

**Pull is graceful, even with uncommitted edits:**
1. If incoming changes touch **no files you've edited**, it is a plain fast-forward. Your edits are untouched. This is the usual case, since each entry is its own file.
2. If the same file was edited on both sides, the app runs a **three-way merge** (libgit2 `merge_file`). Separate edits to different parts of the file merge cleanly, and the result stays in your working copy as uncommitted changes.
3. If there is a **true conflict** (the same lines changed on both sides):
   - The remote version wins at the original path.
   - Your version is saved next to it as `…-title.conflict-hhmm.md`.
   - A banner says so.
   - **Text is never lost** and no conflict markers appear in your journal.
4. Local commits that haven't been pushed are rebased onto the remote, using the same merge rules.

All git work runs on one Rust worker thread, so it never blocks the UI and operations never overlap.

Commit author is your GitHub name with `<id>+<login>@users.noreply.github.com`, or a local default when offline. You can edit it in Settings.

## 6. Images: small pointers, not archives

Paste, drag-drop, or (on the phone) the photo picker. The pipeline is all Rust:

1. Decode the image (JPEG, PNG, WebP, GIF first frame). iOS hands HEIC to the WebView already converted to JPEG.
2. Apply the EXIF orientation, then **strip all metadata**, including GPS location.
3. Downscale so the longest side is at most **1024 px** (Lanczos).
4. Encode as JPEG at **quality 60**, progressive, with 4:2:0 chroma subsampling.
   - A typical phone photo comes out at about **60–150 KB**. Screenshots compress further.
   - It stays clearly readable, not pixelated.
5. Save as `assets/<entry-stem>-1.jpg` (then `-2`, and so on) and insert `![](assets/…jpg)` at the cursor.

Max size and quality are configurable in Settings, but these are the defaults.

## 7. UI

**Desktop (macOS + Linux):**

```
┌────────────────┬─────────────────────────────────────────────┐
│ 🔍 search       │ 2026-11-16 14:02 · [morning-run    ] Src|Sty│
│ + New      ⌘N  │                                              │
│ ── Nov 2026 ── │   editor                                     │
│ 16 morning-run │                                              │
│ 14 ideas       │                                              │
├────────────────┴─────────────────────────────────────────────┤
│ dcxSt/journal · main   ⬇ Pull (2)   ⬆ Commit & Push (1)  312w│
└──────────────────────────────────────────────────────────────┘
```

Keyboard shortcuts (`Ctrl` instead of `⌘` on Linux):

| Shortcut | Action |
|---|---|
| ⌘N | new entry |
| ⌘E | toggle Source/Styled |
| ⌘P | quick-open |
| ⌘⇧F | full-text search |
| ⌘⇧L | pull |
| ⌘K | Commit & Push sheet |
| ⌘\ | toggle sidebar |
| ⌘, | settings |

- Everything can be done from the keyboard.
- Vim `:w` saves.
- **Search:** filename filter as you type, and full-text search across entries with match snippets. It's an in-memory scan in Rust, which is fast for thousands of entries.
- **Word count** for the current entry is shown in the status bar.
- **Themes:** light and dark, following the system automatically.
- **Typography (minimal, "wordcell"):**
  - Styled mode uses a serif: `ui-serif` (Apple's *New York*) on mac and iOS. Linux falls back through `Source Serif 4`, `Noto Serif`, `Georgia`, then `serif`.
  - 18 px body (17 pt on the phone), line height 1.6, a measure of about 66 characters, and an off-white or warm-dark background.
  - Headings use the same serif, sized up but not bolder, with old-style figures where the font supports them.
  - The chrome (sidebar, buttons) uses the small system sans in muted grey, so the text is the only thing on the page.
  - Source mode uses `ui-monospace`, falling back to `JetBrains Mono`, `DejaVu Sans Mono`, then `monospace`.
  - No fonts are bundled.

**iPhone:**
- **Entry list:** search field, month groups, a **+** button, pull-to-refresh, and the Pull / Commit & Push bar.
- **Editor:** Styled mode. A keyboard accessory bar has `#`, **B**, *I*, `-`, `[ ]`, link, 📷 image and Done.
- **First launch** goes straight to an empty journal. GitHub is in Settings.

## 8. Identifiers (what the "reverse-domain ID" is)

Apple, Linux desktops and Flatpak need one globally unique name for the app. It is used to:
- tell the App Store which app this is;
- keep its Keychain items, settings and sandbox folder separate from other apps.

The convention is to take a domain you own, reversed, so nobody else can collide with it. **It can never change after the app is published.**

- Bundle / app ID: **`xyz.stephenfay.mdgitjournal`** (the reverse of `mdgitjournal.stephenfay.xyz`)
- Linux `.desktop` file and AppStream ID: `xyz.stephenfay.mdgitjournal`
- Homepage: `https://mdgitjournal.stephenfay.xyz`. The App Store also requires a privacy policy URL; a one-page `/privacy` there saying "no data collected" is enough.
- **GitHub publishing (done during the run):**
  - Create a **public** `dcxSt/mdgitjournal` with an `MIT` LICENSE and push to it.
  - Create a **private** `dcxSt/mdgitjournal-test` for end-to-end tests.
  - Nothing references Recast.
- **Homepage / privacy page:** `site/` holds a tiny static `index.html` and `privacy.html`, deployable with GitHub Pages. To make `mdgitjournal.stephenfay.xyz` work, you add one DNS CNAME record (`mdgitjournal` → `dcxst.github.io`).

## 9. Distribution

- **From source:** `git clone https://github.com/dcxSt/mdgitjournal && cd mdgitjournal && pnpm i && pnpm tauri build`. The README lists prerequisites for each OS:
  - macOS: Xcode CLT
  - Arch: `webkit2gtk-4.1`, `base-devel`
  - Debian/Ubuntu: `libwebkit2gtk-4.1-dev` …
- **GitHub Releases**, built by CI on a version tag:
  - macOS universal `.dmg`, signed and notarized if Apple secrets are set
  - Linux `.AppImage`, `.deb` and `.rpm`
- **AUR:** `packaging/aur/PKGBUILD` (`mdgitjournal` and `mdgitjournal-bin`).
- **Mac App Store / iOS App Store / TestFlight:**
  - Sandbox, entitlements and bundle ID are all configured.
  - Builds are signed under **your brother's Apple Developer team**. The Team ID, signing identity and App Store Connect API key come from env vars / GitHub secrets (`APPLE_TEAM_ID`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY*`) and are never committed.
  - The App Store will list his team as the seller. If you ever move to your own account, you'll need an App Store app transfer (the bundle ID stays the same).
  - `RELEASING.md` has a step-by-step checklist.
  - Submission is done by you.

## 10. Repo layout

```
crates/mdgj-{core,git,github}/      entries, slug, git ops, merge/conflict, github auth, keyring, images  (+ tests)
app/src-tauri/        Tauri commands, worker thread, events; tauri.conf (mac/linux/ios)
app/src/              Svelte UI: Sidebar, EntryHeader, SyncBar, CommitSheet, Settings, ConnectGitHub, Search
app/src/editor/       CodeMirror setup, source theme, styled-mode decorations, vim, image paste
docs/                 github-app.md, architecture.md, privacy.md
packaging/aur/        PKGBUILD
.github/workflows/    ci.yml (lint, test, build mac/linux/ios-sim), release.yml
```

## 11. Verification ("done" for the one-shot)

- **`mdgj-*` crates unit tests:**
  - slug, filename parse/format, rename keeps the timestamp
  - collision and conflict naming
  - image pipeline: output under 200 KB for a 12 MP test photo, EXIF stripped, orientation applied
- **`mdgj-*` crates integration tests** against local bare repos:
  - init, commit, connect to an empty remote, connect to a non-empty remote
  - fetch with ahead/behind counts
  - Commit & Push blocked when behind
  - pull with dirty unrelated files, a clean three-way merge, and a true conflict (keep-both)
  - push rejected after a race
- **Editor tests:**
  - load → toggle mode N times → save gives byte-identical output (property test, including CRLF and a missing trailing newline)
  - a checkbox click changes exactly one byte
- **Builds:**
  - `tauri build` on macOS, launched and screenshotted
  - Linux build in CI
  - `tauri ios build` for the **Simulator**, launched and screenshotted
- **Real end-to-end:** blocked until the GitHub App is registered (placeholder ID). The plumbing will be tested against `dcxSt/mdgitjournal-test` using a `gh` token through the same HTTPS code path. Device flow itself gets a manual test once you've registered the app.

## 12. Risks

1. **WebKitGTK on Linux** is slower than WKWebView. CodeMirror only decorates the visible viewport, and the CSS avoids heavy effects.
2. **Device flow can't be tested end-to-end** until the real client ID exists. It is isolated behind a trait so the git and sync paths are fully tested without it.
3. **App Store review and sandbox quirks** can't be verified automatically.

## 13. Build order

1. `mdgj-*` crates: entries and slug, local git (init/commit), images.
2. `mdgj-*` crates: remote ops (fetch, ahead/behind, pull with merge and keep-both, push, connect), tested on local bare repos.
3. GitHub auth: device flow, refresh, keyring, repo list/create, behind a trait.
4. Tauri shell: commands, worker, sync-state events.
5. Editor: Source + Vim, Styled decorations, lossless tests, image paste.
6. Desktop UI: sidebar, header rename, search, sync bar, commit sheet, settings, connect flow, themes, word count.
7. iOS layout + Simulator build.
8. CI, release workflow, AUR, README, RELEASING.md, docs.
9. End-to-end against `dcxSt/mdgitjournal-test`.
