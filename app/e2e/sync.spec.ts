// The sync flows of PLAN §5 on the mock backend: one-sided pulls with a summary and Undo, the
// Sync review and the merge view, Resolve with git, and selective commits with diffs.

import { expect, test, type Page } from "@playwright/test";
import {
  addFile,
  allFiles,
  callCount,
  connect,
  editOnDisk,
  fileText,
  finishMergeWithGit,
  IDEAS,
  LIST,
  modKey,
  refocus,
  SAMPLE,
  seedOverlaps,
  start,
} from "./helpers.ts";

const footer = (page: Page) => page.locator("footer");
const pullButton = (page: Page) => footer(page).getByRole("button", { name: /^Pull/ });
const commitButton = (page: Page) => footer(page).getByRole("button", { name: /^Commit/ });
const review = (page: Page) => page.getByRole("dialog", { name: "Sync review" });
const choices = (page: Page, title: string) => review(page).getByRole("group", { name: `What to keep of ${title}` });

/** Opens an entry from the sidebar and puts the cursor at the end of its `line`th line. */
async function typeAtLine(page: Page, title: string, line: number, text: string) {
  await page.getByRole("navigation", { name: "Entries" }).getByRole("button", { name: new RegExp(title) }).click();
  await expect(page.getByLabel("Title")).toHaveValue(title);
  const mod = await modKey(page);
  await page.locator(".cm-content").click();
  await page.keyboard.press(`${mod}+Home`);
  for (let i = 1; i < line; i++) await page.keyboard.press("ArrowDown");
  await page.keyboard.press("End");
  await page.keyboard.type(text);
}

async function connected(page: Page) {
  await start(page);
  await connect(page);
}

test("a one-sided pull applies at once, says what came, and Undo puts it all back", async ({ page }) => {
  await connected(page);
  const before = await allFiles(page);
  await refocus(page);
  await expect(pullButton(page).locator(".badge")).toHaveText("2");
  await pullButton(page).click();

  const notice = page.getByRole("status").filter({ hasText: "Pulled 2 commits from GitHub · 2 files changed." });
  await expect(notice).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Entries" }).getByRole("button", { name: /dawn/ })).toBeVisible();
  expect(await fileText(page, IDEAS)).toContain("Edited on the phone.");

  await notice.getByRole("button", { name: "Details" }).click();
  const details = page.getByRole("dialog", { name: "Pull details" });
  await expect(details.getByText("Ideas from the phone")).toBeVisible();
  await expect(details.getByText("Dawn", { exact: true })).toBeVisible();
  await expect(details.getByText("octocat · just now").first()).toBeVisible();
  await expect(details.getByRole("listitem").filter({ hasText: "new" }).filter({ hasText: "dawn" })).toBeVisible();
  await expect(details.getByRole("listitem").filter({ hasText: "updated" }).filter({ hasText: "ideas" })).toBeVisible();

  await details.getByRole("button", { name: "Undo this pull" }).click();
  await expect(details).toBeHidden();
  await expect(page.getByText("Pull undone.")).toBeVisible();
  await expect(notice).toBeHidden();
  expect(await allFiles(page)).toEqual(before);
  await expect(pullButton(page).locator(".badge")).toHaveText("2");
});

test("Undo refuses, saying why, once a pulled file was edited", async ({ page }) => {
  await connected(page);
  await refocus(page);
  await pullButton(page).click();
  const notice = page.getByRole("status").filter({ hasText: "Pulled 2 commits" });
  await expect(notice).toBeVisible();
  await typeAtLine(page, "ideas", 1, " (edited after the pull)");
  await expect.poll(() => fileText(page, IDEAS)).toContain("edited after the pull");
  const files = await allFiles(page);

  await notice.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByRole("alert").filter({ hasText: `You've changed ${IDEAS} since the pull` })).toBeVisible();
  expect(await allFiles(page)).toEqual(files);
});

test("different paragraphs: the review prefills the merge, the merge view shows it, Apply writes it", async ({ page }) => {
  await connected(page);
  await typeAtLine(page, "ideas", 3, " And nothing else.");
  await expect.poll(() => fileText(page, IDEAS)).toContain("And nothing else.");
  const before = await allFiles(page);

  await pullButton(page).click();
  await expect(review(page)).toBeVisible();
  expect(await allFiles(page)).toEqual(before);
  await expect(review(page).getByText("Changed here and on GitHub")).toBeVisible();
  await expect(choices(page, "ideas").getByRole("button", { name: "Merge…" })).toHaveAttribute("aria-pressed", "true");
  await expect(review(page).getByText("Merged: 1 change of yours and 1 change from GitHub, no clashes.")).toBeVisible();
  await review(page).getByText(/changed on GitHub only/).click();
  await expect(review(page).getByRole("listitem").filter({ hasText: "dawn" })).toBeVisible();
  const apply = review(page).getByRole("button", { name: "Apply" });
  await expect(apply).toBeEnabled();

  await review(page).getByRole("button", { name: "Check the merge" }).click();
  const merge = page.getByRole("dialog", { name: "Merge ideas" });
  await expect(merge).toBeVisible();
  await expect(merge.locator(".cm-line.mdgj-merge-mine")).toHaveText("A journal that is just a folder of markdown files, kept in git. And nothing else.");
  await expect(merge.locator(".cm-line.mdgj-merge-theirs").last()).toHaveText("Edited on the phone.");
  await expect(merge.getByText("No clashes left")).toBeVisible();
  await merge.getByRole("button", { name: "Done" }).click();
  await expect(merge).toBeHidden();

  await apply.click();
  await expect(review(page)).toBeHidden();
  await expect(page.getByRole("status").filter({ hasText: "Pulled 2 commits from GitHub" })).toBeVisible();
  const merged = await fileText(page, IDEAS);
  expect(merged).toContain("And nothing else.");
  expect(merged).toContain("Edited on the phone.");
  expect(merged).not.toMatch(/^(<{7}|={7}|>{7})/m);
  // The open entry shows what was written, and the merge stays a change for the next commit.
  await expect(page.locator(".cm-content")).toContainText("Edited on the phone.");
  await expect(commitButton(page).locator(".badge")).toHaveText("1");
});

test("a clash needs a choice before Apply; the merge view picks one, and Done takes it", async ({ page }) => {
  await connected(page);
  const mod = await modKey(page);
  await typeAtLine(page, "ideas", 1, "");
  await page.keyboard.press(`${mod}+End`);
  await page.keyboard.type("Typed here.");
  await expect.poll(() => fileText(page, IDEAS)).toContain("Typed here.");

  await pullButton(page).click();
  const apply = review(page).getByRole("button", { name: "Apply" });
  await expect(apply).toBeDisabled();
  await expect(review(page).getByText("Choose for 1 more file")).toBeVisible();
  for (const name of ["Keep mine", "Keep GitHub's", "Keep both", "Merge…"]) {
    await expect(choices(page, "ideas").getByRole("button", { name })).toHaveAttribute("aria-pressed", "false");
  }

  await choices(page, "ideas").getByRole("button", { name: "Merge…" }).click();
  const merge = page.getByRole("dialog", { name: "Merge ideas" });
  const done = merge.getByRole("button", { name: "Done" });
  await expect(merge.getByText("1 clash left")).toBeVisible();
  await expect(done).toBeDisabled();
  const clash = merge.locator(".mdgj-clash");
  await expect(clash.locator(".mdgj-clash-mine")).toContainText("Typed here.");
  await expect(clash.locator(".mdgj-clash-theirs")).toContainText("Edited on the phone.");
  await clash.getByRole("button", { name: "Use both" }).click();
  await expect(merge.getByText("No clashes left")).toBeVisible();
  await expect(merge.locator(".mdgj-clash")).toHaveCount(0);

  // Undo takes the pick back; picking again and editing freely is fine too.
  await merge.getByRole("button", { name: "Undo" }).click();
  await expect(merge.getByText("1 clash left")).toBeVisible();
  await merge.locator(".mdgj-clash").getByRole("button", { name: "Use GitHub's" }).click();
  await merge.locator(".cm-content").click();
  await page.keyboard.press(`${mod}+End`);
  await page.keyboard.type("Edited in the merge.");
  await done.click();

  await expect(choices(page, "ideas").getByRole("button", { name: "Merge…" })).toHaveAttribute("aria-pressed", "true");
  await expect(review(page).getByText("Merged in the merge view.")).toBeVisible();
  await apply.click();
  await expect(review(page)).toBeHidden();
  const text = await fileText(page, IDEAS);
  expect(text).toContain("Edited on the phone.\nEdited in the merge.");
  expect(text).not.toContain("Typed here.");
});

test("Keep both keeps yours at the name and saves GitHub's next to it", async ({ page }) => {
  await connected(page);
  const { clash } = await seedOverlaps(page);
  const mine = (await fileText(page, clash))!;
  await pullButton(page).click();
  await expect(review(page)).toBeVisible();

  await choices(page, "reading-list").getByRole("button", { name: "Keep both" }).click();
  const outcome = review(page).getByText(/^Yours stays; GitHub's is saved next to it as /);
  await expect(outcome).toBeVisible();
  const copy = (await outcome.textContent())!
    .trim()
    .replace(/^Yours stays; GitHub's is saved next to it as /, "")
    .replace(/\.$/, "");
  expect(copy).toMatch(/^2026-10-28-0815-reading-list\.from-github-\d{4}\.md$/);

  await review(page).getByRole("button", { name: "Apply" }).click();
  await expect(review(page)).toBeHidden();
  expect(await fileText(page, clash)).toBe(mine);
  expect(await fileText(page, copy)).toContain("- *Walden*, H. D. Thoreau");
  await expect(
    page.getByRole("navigation", { name: "Entries" }).getByRole("button", { name: /reading-list\.from-github/ }),
  ).toBeVisible();
});

test("Cancel (or Esc) leaves every file as it was and asks nothing of the backend", async ({ page }) => {
  await connected(page);
  await seedOverlaps(page);
  const before = await allFiles(page);
  await pullButton(page).click();
  await expect(review(page)).toBeVisible();
  await choices(page, "reading-list").getByRole("button", { name: "Keep GitHub's" }).click();
  await review(page).getByRole("button", { name: "Cancel", exact: true }).click();
  await expect(review(page)).toBeHidden();

  await pullButton(page).click();
  await expect(review(page)).toBeVisible();
  // A fresh review: the earlier choice is gone.
  await expect(choices(page, "reading-list").getByRole("button", { name: "Keep GitHub's" })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  await page.keyboard.press("Escape");
  await expect(review(page)).toBeHidden();
  expect(await allFiles(page)).toEqual(before);
  expect(await callCount(page, "applyPull")).toBe(0);
  // Still behind: the two commits from connecting and the two of the overlaps.
  await expect(pullButton(page).locator(".badge")).toHaveText("4");
});

test("a review gone stale writes nothing and opens again, fresh", async ({ page }) => {
  await connected(page);
  await seedOverlaps(page);
  await pullButton(page).click();
  await choices(page, "reading-list").getByRole("button", { name: "Keep mine" }).click();
  await editOnDisk(page, "notes.md", "Edited in vim while the review was open.\n");
  const before = await allFiles(page);

  await review(page).getByRole("button", { name: "Apply" }).click();
  await expect(review(page).getByText("Something changed since you opened the review.")).toBeVisible();
  expect(await allFiles(page)).toEqual(before);
  await expect(choices(page, "reading-list").getByRole("button", { name: "Keep mine" })).toHaveAttribute("aria-pressed", "false");
  await expect(review(page).getByRole("button", { name: "Apply" })).toBeDisabled();
  await expect(pullButton(page).locator(".dot")).toHaveCount(0);
});

test("Resolve with git leaves a merge for git; the app says so, blocks syncing, and Abort undoes it", async ({ page }) => {
  await connected(page);
  await seedOverlaps(page);
  const before = await allFiles(page);
  await pullButton(page).click();
  await review(page).getByRole("button", { name: "Resolve with git…" }).click();
  await expect(review(page).getByRole("heading", { name: "Resolve with git" })).toBeVisible();
  await expect(review(page).getByText(/git merge origin\/main/)).toBeVisible();
  const start = review(page).getByRole("button", { name: "Start the merge" });
  await expect(start).toBeDisabled();
  await review(page).getByLabel("Commit message for your changes").fill("Edits before merging");
  await start.click();
  await expect(review(page)).toBeHidden();

  const banner = page.getByRole("status").filter({ hasText: "Merge in progress — finish it with git, or abort it." });
  await expect(banner).toBeVisible();
  await expect(banner.getByRole("button", { name: LIST })).toBeVisible();
  await expect(banner.getByRole("button", { name: "Dismiss" })).toHaveCount(0);
  await expect(pullButton(page)).toBeDisabled();
  await expect(pullButton(page)).toHaveAttribute("title", /Merge in progress — finish it with git/);
  await expect(commitButton(page)).toBeDisabled();
  expect(await fileText(page, LIST)).toMatch(/^<{7} HEAD\n[\s\S]*^={7}\n[\s\S]*^>{7} origin\/main\n/m);

  await banner.getByRole("button", { name: "Abort merge" }).click();
  const confirm = page.getByRole("dialog", { name: "Abort merge" });
  await expect(confirm.getByText(/Files git merged go back to how they were before the merge/)).toBeVisible();
  await confirm.getByRole("button", { name: "Abort merge" }).click();
  await expect(confirm).toBeHidden();
  await expect(banner).toBeHidden();
  expect(await allFiles(page)).toEqual(before);
  await expect(pullButton(page)).toBeEnabled();
  await expect(commitButton(page).locator(".badge")).toHaveText("2");
  await expect(page.getByText("Merge finished.")).toHaveCount(0);
});

test("a merge finished in a terminal is noticed when the window comes back", async ({ page }) => {
  await connected(page);
  await seedOverlaps(page);
  await pullButton(page).click();
  await review(page).getByRole("button", { name: "Resolve with git…" }).click();
  await review(page).getByLabel("Commit message for your changes").fill("Edits before merging");
  await review(page).getByRole("button", { name: "Start the merge" }).click();
  const banner = page.getByRole("status").filter({ hasText: "Merge in progress" });
  await expect(banner).toBeVisible();

  await editOnDisk(page, LIST, "# Reading list\n\n- *The Peregrine*, J. A. Baker\n- *Walden*, again, by H. D. Thoreau\n");
  await finishMergeWithGit(page);
  await page.waitForTimeout(2100); // focus refreshes are at most every 2 s
  await refocus(page);
  await expect(page.getByText("Merge finished.")).toBeVisible();
  await expect(banner).toBeHidden();
  await expect(pullButton(page)).toBeEnabled();
});

test("Commit takes only the ticked files, and each file's diff is a click away", async ({ page }) => {
  await start(page);
  const mod = await modKey(page);
  await page.locator(".cm-content").click();
  await page.keyboard.press(`${mod}+End`);
  await page.keyboard.type("Ran again at dusk.");
  await expect.poll(() => fileText(page, SAMPLE)).toContain("dusk");
  await editOnDisk(page, IDEAS, "# Ideas\n\nRewritten elsewhere.\n");
  await addFile(page, "assets/2026-11-16-1402-morning-run-5f3a09c2.jpg", "\0jpeg");
  await refocus(page);
  await expect(commitButton(page).locator(".badge")).toHaveText("3");

  await page.keyboard.press(`${mod}+k`);
  const sheet = page.getByRole("dialog", { name: "Commit" });
  const boxes = sheet.getByRole("checkbox", { name: /^Commit / });
  await expect(boxes).toHaveCount(3);
  for (const box of await boxes.all()) await expect(box).toBeChecked();
  await expect(sheet.getByText("All 3 changed files")).toBeVisible();

  // A text diff: added and removed lines, tinted.
  await sheet.getByRole("button", { name: IDEAS }).click();
  const diff = sheet.getByRole("group", { name: `Changes to ${IDEAS}` });
  await expect(diff.locator(".line.added .text")).toHaveText(["Rewritten elsewhere."]);
  await expect(diff.locator(".line.context .text").first()).toHaveText("# Ideas");
  await expect(diff.locator(".line.removed .text").first()).toHaveText("A journal that is just a folder of markdown files, kept in git.");
  await expect(diff.locator(".line.removed .text").last()).toHaveText("**Bold** thoughts and _quiet_ ones.");
  await sheet.getByRole("button", { name: IDEAS }).click();
  await expect(diff).toBeHidden();
  await sheet.getByRole("button", { name: /morning-run-5f3a09c2\.jpg/ }).click();
  await expect(sheet.getByText("Image added.")).toBeVisible();

  await sheet.getByRole("checkbox", { name: `Commit ${IDEAS}` }).uncheck();
  await sheet.getByRole("checkbox", { name: /5f3a09c2\.jpg$/ }).uncheck();
  await expect(sheet.getByText("1 of 3 files")).toBeVisible();
  await expect(sheet.getByText("Unticked files stay as changes for a later commit.")).toBeVisible();
  await sheet.getByRole("checkbox", { name: `Commit ${SAMPLE}` }).uncheck();
  const message = sheet.getByLabel("Commit message");
  await message.fill("Evening run");
  await expect(sheet.getByRole("button", { name: "Commit", exact: true })).toBeDisabled();
  await sheet.getByRole("checkbox", { name: `Commit ${SAMPLE}` }).check();
  await message.press("Enter");
  await expect(sheet).toBeHidden();
  await expect(page.getByText("Committed.")).toBeVisible();
  await expect(commitButton(page).locator(".badge")).toHaveText("2");
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test("the review and the merge view fill the screen, with touch-sized controls", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "Settings" }).click();
    await page.getByRole("button", { name: "Connect GitHub" }).click();
    await page.getByRole("button", { name: "Sign in with GitHub" }).click();
    await page.getByRole("button", { name: /octocat\/journal/ }).click({ timeout: 15_000 });
    await expect(page.getByText("Connected to")).toBeVisible({ timeout: 15_000 });
    await page.getByRole("button", { name: "Close settings" }).click();
    await seedOverlaps(page);

    await page.getByRole("button", { name: /^Pull/ }).click();
    const sheet = review(page);
    await expect(sheet).toBeVisible();
    const box = (await sheet.boundingBox())!;
    expect(box.width).toBe(390);
    expect(box.height).toBe(844);
    for (const button of await choices(page, "reading-list").getByRole("button").all()) {
      expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    }
    expect((await sheet.getByRole("button", { name: "Apply" }).boundingBox())!.height).toBeGreaterThanOrEqual(44);

    await choices(page, "reading-list").getByRole("button", { name: "Merge…" }).click();
    const merge = page.getByRole("dialog", { name: "Merge reading-list" });
    await expect(merge).toBeVisible();
    await expect(merge.locator(".cm-gutters")).toHaveCount(0);
    const use = merge.locator(".mdgj-clash").getByRole("button", { name: "Use mine" });
    expect((await use.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    await use.click();
    await merge.getByRole("button", { name: "Done" }).click();
    await sheet.getByRole("button", { name: "Apply" }).click();
    await expect(sheet).toBeHidden();
    expect(await fileText(page, LIST)).toContain("- *Walden* (again)");
  });
});
