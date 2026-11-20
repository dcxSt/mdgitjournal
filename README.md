<p align="center"><img src="assets/icon.svg" width="96" height="96" alt=""></p>

# mdgitjournal

A minimal, fast markdown journal for macOS, Linux and iPhone. Every entry is a plain `.md` file in an ordinary git repository, so you get version history from the first keystroke, offline and without an account. If you want sync between devices, connect one GitHub repository and use two explicit buttons: **Pull** and **Commit & Push**.

<!-- Screenshot goes here: docs/screenshot.png -->

## Design principles

1. **The `.md` file is the only source of truth.** No database, no front matter, no hidden metadata.
2. **Never change bytes you didn't touch.** Opening, switching modes or saving without edits gives zero diff.
3. **Nothing happens behind your back.** No auto-commit, no auto-push. The only background network activity is `git fetch`.
4. **Plain git underneath.** The `git` CLI works on your journal at any time.
5. **Works without an account.** GitHub is an add-on.
6. **Small and fast.** One Rust binary and the system WebView. No Electron, no telemetry, no analytics.

## Features

- Two views of the same text: **Source** (monospace, line numbers) and **Styled** (serif, markdown syntax kept but dimmed). Switching is instant and lossless.
- One flat folder of entries named `yyyy-mm-dd-hhmm-title.md`; rename an entry by editing its title.
- Git history built in. You write every commit message.
- Optional GitHub sync where you stay in charge of merges. A file changed on only one side arrives at once, with a summary and **Undo**. A file changed on both sides waits for you in the **Sync review**: keep yours, keep GitHub's, keep both, or merge them in an editor that has already applied the edits that don't clash and asks only about the ones that do. Prefer vim or `git mergetool`? **Resolve with git** leaves an ordinary git merge for you to finish.
- You choose which changed files go into each commit, and can look at each file's diff first.
- Paste or drop images: they are stored in `assets/` as small JPEGs (longest side 1024 px, quality 60) with all metadata, including location, removed.
- Filename and full-text search, word count, optional Vim keys, light and dark themes that follow the system.

## Where your data lives

| | Journal (a git repository) | Settings |
|---|---|---|
| macOS | `~/Library/Application Support/xyz.stephenfay.mdgitjournal/journal/` | `~/Library/Application Support/xyz.stephenfay.mdgitjournal/settings.json` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/xyz.stephenfay.mdgitjournal/journal/` | `${XDG_CONFIG_HOME:-~/.config}/xyz.stephenfay.mdgitjournal/settings.json` |
| iOS | Files app › On My iPhone › mdgitjournal › journal | inside the app's sandbox |

GitHub tokens are kept in the macOS/iOS Keychain or the Linux Secret Service. Without a Secret Service, they go in `credentials.json` (mode `0600`) next to `settings.json`. Tokens are never written into the repository or `.git/config`.

## Build from source

Prerequisites:

- **macOS:** Xcode Command Line Tools (`xcode-select --install`), [Rust](https://rustup.rs), [Node.js](https://nodejs.org) LTS and [pnpm](https://pnpm.io/installation).
- **Arch Linux**, plus Rust (`rustup` or `rust`):
  ```sh
  sudo pacman -S --needed base-devel webkit2gtk-4.1 dbus curl wget file librsvg nodejs pnpm
  ```
- **Debian / Ubuntu** (22.04 or newer), plus Rust, Node.js LTS and pnpm:
  ```sh
  sudo apt install build-essential pkg-config curl wget file libwebkit2gtk-4.1-dev libdbus-1-dev librsvg2-dev
  ```
- **Fedora**, plus Rust, Node.js LTS and pnpm:
  ```sh
  sudo dnf install webkit2gtk4.1-devel dbus-devel curl wget file librsvg2-devel perl
  sudo dnf group install c-development
  ```

libgit2 and OpenSSL are compiled from source and linked statically, which needs `perl` and `make`; the build tools above include both.

Build:

```sh
git clone https://github.com/dcxSt/mdgitjournal && cd mdgitjournal
pnpm -C app install
pnpm -C app tauri build
```

Output, relative to the repository root:

- macOS: `target/release/bundle/macos/mdgitjournal.app` and `target/release/bundle/dmg/*.dmg`
- Linux: the binary `target/release/mdgitjournal`, plus `target/release/bundle/{appimage,deb,rpm}/`

To build only the binary, add `--no-bundle`; for a single package format, add e.g. `--bundles deb`.

On macOS, building the `.dmg` runs a Finder AppleScript that lays out its window, so the first build asks to let your terminal control Finder. Over SSH that prompt can't be answered, and the build waits at *Running bundle_dmg.sh*. To skip the layout, build with `CI=true pnpm -C app tauri build` (the `.dmg` then has a plain window), or build only the app with `--bundles app`.

For Arch, [`packaging/aur`](packaging/aur) has the PKGBUILDs of `mdgitjournal` (from source) and `mdgitjournal-bin` (the release build), which are published to the AUR with each release.

## Development

```sh
pnpm -C app tauri dev                                  # app with hot reload
MDGJ_JOURNAL_DIR=/tmp/journal MDGJ_CONFIG_DIR=/tmp/config pnpm -C app tauri dev  # same, on a scratch journal and sign-in
pnpm -C app dev                                        # UI only, in a browser, on a mock backend
```

Tests and checks (CI runs all of these):

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
pnpm -C app test
pnpm -C app check
pnpm -C app e2e      # the UI in WebKit, on the mock backend; once: pnpm -C app exec playwright install webkit
```

`MDGJ_SHOTS_DIR=/tmp/shots pnpm -C app e2e` also saves screenshots of the main screens, light and dark, desktop and phone.

End-to-end tests that need github.com are marked `#[ignore]`, so these commands run offline.

### Two devices on one machine

To try syncing without a second computer, build the app (`pnpm -C app tauri build --bundles app` on macOS, `--no-bundle` on Linux) and run

```sh
scripts/two-devices.sh            # --reset starts from empty journals; --dev uses the debug build and `pnpm -C app dev`
```

It opens two windows, *mdgitjournal — Device A* and *— Device B*, each with its own journal, settings and sign-in under `~/mdgitjournal-test-devices/`, kept between runs. Sign in and connect both to the same (test) repository, then edit, commit and pull on either side. Your real journal and keychain are not touched.

## Connecting GitHub

Optional. In **Settings › Connect GitHub**:

1. **Sign in with GitHub.** The app shows a short code; approve it in the browser.
2. **Install the mdgitjournal GitHub App on one repository**, e.g. a new private `journal` repository. The app can only see repositories you install it on.
3. **Connect.** An empty repository receives your local history. An existing one is pulled by the same rules as **Pull**: files only one side has arrive at once, and a file both sides have with different content waits for you in the Sync review.

Alternatively, paste a fine-grained personal access token limited to that repository (*Contents: read and write*) and type `owner/repo`. Source builds without a registered GitHub App use this path. [docs/github-app.md](docs/github-app.md) covers both, including registering your own GitHub App.

## Environment variables

| Variable | When | Effect |
|---|---|---|
| `MDGJ_JOURNAL_DIR` | run time | Use this directory as the journal instead of the default. |
| `MDGJ_CONFIG_DIR` | run time | Keep `settings.json` and the sign-in in this directory. The sign-in is then stored only in a `0600` file there, never in the keychain, so instances with different directories are fully separate. |
| `MDGJ_DEVICE_NAME` | run time | Adds ` — <name>` to the window title, to tell instances apart. |
| `MDGJ_GITHUB_CLIENT_ID` | build time | Client ID of the GitHub App to sign in with; overrides the one in `.cargo/config.toml`. Without either, only personal access tokens work. |
| `MDGJ_GITHUB_APP_SLUG` | build time | URL name of that GitHub App, if it isn't `mdgitjournal`. |

## Keyboard shortcuts

On Linux, use `Ctrl` instead of `⌘`.

| Shortcut | Action |
|---|---|
| `⌘N` | New entry |
| `⌘E` | Toggle Source / Styled |
| `⌘P` | Quick open |
| `⌘⇧F` | Full-text search |
| `⌘⇧L` | Pull |
| `⌘K` | Commit & Push (or Commit, when not connected) |
| `⌘\` | Toggle sidebar |
| `⌘,` | Settings |
| `⌘S`, Vim `:w` | Save now (saving is automatic anyway) |

In the commit sheet, `Enter` or `⌘Enter` commits, `Shift+Enter` adds a new line, and `Esc` cancels.

On Linux with Vim keys on, `Ctrl+E`, `Ctrl+N` and `Ctrl+P` go to Vim while the text has focus; anywhere else they are the app's shortcuts.

## iOS

iOS builds need macOS with full Xcode (licence accepted: `sudo xcodebuild -license accept`).

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
pnpm -C app tauri ios init                               # once; generates app/src-tauri/gen/apple
pnpm -C app tauri ios dev                                # run in the Simulator
pnpm -C app tauri ios build --target aarch64-sim --debug # Simulator build
```

`tauri ios init` generates the Xcode project from `app/src-tauri/project.ios.yml`: Tauri's own template, plus libz and libiconv, which libgit2 needs. Run it through `pnpm -C app` as above, since that path is relative to `app/`. After updating `@tauri-apps/cli`, start `project.ios.yml` again from the CLI's `templates/mobile/ios/project.yml` and add those two lines back.

Builds for a real device need an Apple development team: `export APPLE_DEVELOPMENT_TEAM=<Team ID>`. App Store and TestFlight steps are in [RELEASING.md](RELEASING.md).

## Privacy

mdgitjournal collects no data. See [docs/privacy.md](docs/privacy.md).

## License

[MIT](LICENSE) © 2026 Stephen Fay.

This software is based in part on the work of the Independent JPEG Group. The licences of everything compiled into the app are in [packaging/THIRD-PARTY-NOTICES.md](packaging/THIRD-PARTY-NOTICES.md), which the app also shows under Settings › About.
