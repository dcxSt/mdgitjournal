# Saturday guide: testing mdgitjournal by hand

A step-by-step script for a first full manual pass on a Mac. About 90 minutes. Tick things off as you go; anything that surprises you is worth reporting (see [section E](#e-what-to-look-out-for-and-how-to-report)).

Commands assume you are in the repository root (`~/.t3/projects/mdjournal`).

## A. Install and open

The release build is already made:

- App: `target/release/bundle/macos/mdgitjournal.app`
- Disk image: `target/release/bundle/dmg/mdgitjournal_0.1.0_aarch64.dmg`

To rebuild: `CI=true pnpm -C app tauri build`.

**Option 1: install it.** Open the `.dmg` and drag *mdgitjournal* to *Applications*. A local build opens normally. A copy downloaded from GitHub is ad-hoc signed, not notarized; if macOS says it can't check it, open *System Settings › Privacy & Security* and click **Open Anyway** (on macOS 15 and later, right-click › Open no longer offers this). This uses the real journal at `~/Library/Application Support/xyz.stephenfay.mdgitjournal/journal/`.

**Option 2: a scratch journal** that the real app never sees:

```sh
MDGJ_JOURNAL_DIR=~/mdgitjournal-solo/journal MDGJ_CONFIG_DIR=~/mdgitjournal-solo/config \
  target/release/bundle/macos/mdgitjournal.app/Contents/MacOS/mdgitjournal
```

**Option 3: a dev build** with live reload: `MDGJ_JOURNAL_DIR=/tmp/j MDGJ_CONFIG_DIR=/tmp/c pnpm -C app tauri dev`. Leave out the two variables and it uses the real journal.

## B. On your own (no GitHub), about 15 minutes

- [ ] **New entry:** `⌘N`, type a title, Enter. Write a few paragraphs. The file appears in the sidebar under the month; there is no Save button (autosave, 0.4 s).
- [ ] **Rename:** edit the title at the top of the entry. The file name changes (`yyyy-mm-dd-hhmm-new-title.md`); the date and time stay.
- [ ] **Styled / Source:** `⌘E` toggles. Headings, **bold**, lists and `- [ ]` tasks look different but are the same text; toggle a few times and nothing in the file changes (check with `git diff` later).
- [ ] **Vim:** Settings (`⌘,`) › *Vim keys*. Try `dd`, `u`, `:w`. Turn it off again if you don't use vim.
- [ ] **Image paste:** copy a photo (or a screenshot with `⌘⇧⌃4`) and paste into an entry. A `![](assets/…jpg)` line appears and, in Styled, the picture. It is resized to under 200 KB.
- [ ] **Search:** `⌘P` finds entries by name; `⌘⇧F` searches the text.
- [ ] **Commit:** the **Commit** button shows how many files changed. Open it (`⌘K`):
  - every file has a ticked checkbox; click a file name to see its diff (an image says "image added");
  - untick one file, write a message, commit. Open the sheet again: only the unticked file is left;
  - an empty message isn't accepted.
- [ ] In a terminal, `git -C <journal> log --stat` shows plain commits, as any git repository would.

## C. Connect GitHub with a token, about 10 minutes

Use a **fresh private repository**, e.g. `dcxSt/journal-saturday` (create it empty on github.com, no README). `dcxSt/mdgitjournal-test` also works, but the automated e2e tests write to it and clean up after themselves, so keep your test entries out of it.

Make a fine-grained token on github.com: *Settings › Developer settings › Personal access tokens › Fine-grained tokens › Generate new token*:

- **Token name:** `mdgitjournal Saturday`; **Expiration:** 7 days.
- **Resource owner:** dcxSt.
- **Repository access:** *Only select repositories* › the test repository.
- **Permissions › Repository permissions › Contents:** *Read and write*. (*Metadata: Read-only* is added by itself; nothing else.)
- Generate, and copy the token (`github_pat_…`).

In the app: Settings (`⌘,`) › **Connect GitHub** › paste the token › type `dcxSt/journal-saturday` › **Connect**. An empty repository receives your commits from section B. The sync area now shows **Pull** and **Commit & Push**.

## D. Two devices on one Mac, about 60 minutes

```sh
scripts/two-devices.sh          # add --reset to start again from empty journals
```

Two windows open, **mdgitjournal — Device A** and **— Device B** (on top of each other; drag one aside). Each has its own journal, settings and sign-in under `~/mdgitjournal-test-devices/{a,b}/`; your real journal and keychain are not touched. Quit by closing both windows; Ctrl-C in the terminal also saves and closes them.

**Setup:** connect both to the same fresh repository (section C; the same token works for both). Connect **A** first. On A, make an entry *Run* with three paragraphs (`First paragraph.`, `Second paragraph.`, `Third paragraph.` on separate lines with blank lines between), **Commit & Push**. Switch to B: it fetches when its window gets focus, Pull lights up with a badge; **Pull**. B now has *Run*.

The journals are plain folders: `~/mdgitjournal-test-devices/a/journal` and `…/b/journal`. Below, "push on A" means: edit on A, **Commit & Push** on A, then click into B's window (which fetches).

1. [ ] **One-sided pull, summary, Undo.** Push an edit to *Run* on A. On B, **Pull**: it applies at once; a notice says "Pulled 1 commit from GitHub · 1 file changed" with **Details** (commit message, author, time, the file) and **Undo**. Click **Undo**: B's *Run* is back as it was, Pull is lit again. **Pull** again.
2. [ ] **"Pull first".** Push an edit on A. On B, edit a *different* entry. B's **Commit & Push** is disabled with the hint "Pull first". Pull (it applies at once: different files), then Commit & Push.
3. [ ] **Different paragraphs of one entry.** Push an edit to the first paragraph of *Run* on A. On B, edit the third paragraph (don't commit). **Pull**: nothing changes on disk; the **Sync review** opens. *Run* is listed as "Changed here and on GitHub", already set to **Merge…** with "Merged: 1 change of yours and 1 change from GitHub, no clashes." Click **Check the merge**: GitHub's line and yours are tinted differently. **Done**, **Apply**. *Run* has both edits; the notice says "1 resolved file ready to commit". Commit & Push it.
4. [ ] **The same line: a clash.** Push a change to the second paragraph on A; on B change the same line differently. **Pull**: no default; Apply is disabled ("Choose for 1 more file"). Click **Keep mine** and **Keep GitHub's** and read what each says it will do. Then **Merge…**: the clash is a block with *Yours* and *GitHub's* and **Use mine · Use GitHub's · Use both**. Pick one, then edit the text freely; **Undo** takes a pick back. **Done**, **Apply**. Commit & Push.
5. [ ] **Keep both.** Make another clash as in 4 and choose **Keep both**: yours stays; GitHub's is saved next to it as `…-run.from-github-14h05.md` (the time of GitHub's commit). After Apply, both are in the sidebar.
6. [ ] **Cancel and Esc.** Make a clash, Pull, choose **Keep GitHub's**, then press **Esc**: it asks "Discard your choices for this pull?" (**Keep reviewing**). Then **Cancel**. Nothing changed: `git -C ~/mdgitjournal-test-devices/b/journal status` shows only your own edit. Pull again: the review is back, fresh.
7. [ ] **A review that went stale.** With a review open on B, change B's file from outside, e.g. `echo "typed in the terminal" >> ~/mdgitjournal-test-devices/b/journal/<file>.md`. **Apply**: nothing is written; the review reopens saying "Something changed since you opened the review", naming that file. Your choices for other files are kept.
8. [ ] **Retitle on one device, edit on the other.** On A, change *Run*'s title to *Long run* and push. On B, add a line to *Run*. **Pull**: one row, "Changed here, renamed on GitHub to “long-run”", set to Merge. **Apply**: one entry, *Long run*, with B's line. Commit & Push.
9. [ ] **Unpushed commits on both, no overlap.** Push an edit to entry *X* on A. On B, edit a different entry *Y* and commit it **without pushing** (the app commits locally only when GitHub moved meanwhile, so use git here):

   ```sh
   cd ~/mdgitjournal-test-devices/b/journal
   git add -A && git commit -m "Y, on B"
   ```

   **Pull** on B: applied at once, "your 1 unpushed commit was placed after them". **Undo** works here too (then Pull again). **Commit & Push** pushes the waiting commit. `git log --graph --oneline` is a straight line.
10. [ ] **Unpushed commits on both, same file.** As 9, but B's local commit changes the same paragraph of *Run* as A's. **Pull**: the review explains Apply makes a merge commit, with a message box prefilled "Merge from GitHub" and what you chose. Choose, Apply, Commit & Push. `git log --graph --oneline` shows the merge.
11. [ ] **Resolve with git, finished in a terminal.** Make a clash (as in 4, B's edit uncommitted). In the review, **Resolve with git…**: it lists the files that will get conflict markers (and any git merges on its own), and asks for a commit message for your uncommitted changes to them. **Start the merge**. A banner says "Merge in progress — finish it with git, or abort it."; Pull and Commit & Push are off. In a terminal:

    ```sh
    cd ~/mdgitjournal-test-devices/b/journal
    git status                 # "You have unmerged paths"
    vim 2026-…-run.md          # remove the <<<<<<< ======= >>>>>>> lines, keep what you want
    git add 2026-…-run.md
    git commit --no-edit
    ```

    Back in the app (within 5 s, or on focus): "Merge finished.", the banner goes. **Commit & Push** pushes the merge. `git mergetool` works too.
12. [ ] **Abort merge.** Start another Resolve with git. In the banner, **Abort merge**: the confirmation lists exactly the files it puts back. Abort: "Merge aborted."; your edit is back as an uncommitted change. Then try these:
    - Push from A a clashing edit to *Run* **and** an edit to another entry *Z*; edit *Run* on B; Resolve with git. The banner says "git also merged 1 other file". Type a line into *Z* on B, then Abort: it refuses and names *Z*, changing nothing. Undo your line in *Z* (or keep the text elsewhere) and Abort again.
    - Start one and run `git merge --abort` in the terminal: the app says "Merge aborted with git. Your edits stay committed, as an unpushed commit."

## E. What to look out for, and how to report

- **Never lost text:** after every step, does every file hold what you expect? `git -C <journal> status` and `git -C <journal> diff` are the ground truth.
- **Nothing behind your back:** nothing should be committed, pushed, merged or copied without a click from you.
- **Plain git:** `git log --graph --oneline --all` and `git status` in either journal should always make sense.
- **Words:** every banner, button and note should say what happened or will happen. If you have to guess, that is a bug.

To report: what you did (the step number above helps), what you expected, what happened, and a screenshot. Attach:

- the logs: `~/mdgitjournal-test-devices/a/app.log` and `…/b/app.log` (an installed app launched from Finder logs nowhere; start it from a terminal as in section A to see its output);
- `git -C ~/mdgitjournal-test-devices/b/journal log --graph --oneline --all -20` and `git status`.

To start over: close both windows, then `scripts/two-devices.sh --reset` (deletes both test devices' journals, settings and sign-ins; you connect again). To clear the test repository, delete it on github.com and create it again, empty.

## F. Still to do (owner)

- **iPhone build:** accept the Xcode licence once with `sudo xcodebuild -license accept`, then follow [README › iOS](../README.md#ios) (`pnpm -C app tauri ios init`, then `pnpm -C app tauri ios build --target aarch64-sim --debug`).
- **Register the GitHub App**, so people can sign in without a token: [docs/github-app.md](github-app.md), then put its client ID in `.cargo/config.toml` ([RELEASING.md](../RELEASING.md)).
- **GitHub Pages and DNS** for <https://mdgitjournal.stephenfay.xyz>: in the repository, *Settings › Pages › Source: GitHub Actions*; at the DNS provider, `mdgitjournal CNAME dcxst.github.io.`; then *Settings › Pages › Custom domain* and **Enforce HTTPS** ([RELEASING.md](../RELEASING.md)). Until then the *Pages* workflow fails on every push; that is expected.
