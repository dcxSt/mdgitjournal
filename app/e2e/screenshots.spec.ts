// Screenshots of the main screens for a visual review: `MDGJ_SHOTS_DIR=/tmp/shots pnpm e2e`.

import { devices, expect, test, type Page } from "@playwright/test";
import { connect, editOnDisk, IDEAS, modKey, refocus, seedOverlaps, start } from "./helpers.ts";

const dir = process.env.MDGJ_SHOTS_DIR;
test.skip(!dir, "set MDGJ_SHOTS_DIR to take screenshots");

const shot = (page: Page, name: string) => page.screenshot({ path: `${dir}/${name}.png` });

for (const scheme of ["light", "dark"] as const) {
  test.describe(`desktop, ${scheme}`, () => {
    test.use({ colorScheme: scheme, deviceScaleFactor: 2, reducedMotion: "reduce" });

    test("screens", async ({ page }) => {
      await start(page);
      await shot(page, `desktop-${scheme}-source`);

      await page.getByRole("group", { name: "Editor mode" }).getByRole("button", { name: "Styled" }).click();
      await shot(page, `desktop-${scheme}-styled`);

      await page.locator(".cm-content").click();
      await page.keyboard.type("A heron on the far bank. ");
      const mod = await modKey(page);
      await page.keyboard.press(`${mod}+k`);
      await expect(page.getByLabel("Commit message")).toBeFocused();
      await shot(page, `desktop-${scheme}-commit`);
      await page.keyboard.press("Escape");

      await page.keyboard.press(`${mod}+,`);
      await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
      await shot(page, `desktop-${scheme}-settings`);
    });
  });

  test.describe(`phone, ${scheme}`, () => {
    const { defaultBrowserType: _, ...iPhone } = devices["iPhone 13"];
    test.use({ ...iPhone, viewport: { width: 390, height: 844 }, colorScheme: scheme, reducedMotion: "reduce" });

    test("screens", async ({ page }) => {
      await page.goto("/");
      const entry = page.getByRole("button", { name: /morning-run/ });
      await expect(entry).toBeVisible();
      await shot(page, `phone-${scheme}-list`);

      await entry.click();
      await expect(page.locator(".cm-content")).toBeVisible();
      await shot(page, `phone-${scheme}-editor`);
    });
  });
}

/** Connected, with the morning run merging cleanly and the reading list clashing. */
async function overlaps(page: Page) {
  await connect(page);
  await seedOverlaps(page);
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`sync, desktop, ${scheme}`, () => {
    test.use({ colorScheme: scheme, deviceScaleFactor: 2, reducedMotion: "reduce", viewport: { width: 1100, height: 720 } });

    test("review, merge view, summary, merge in progress", async ({ page }) => {
      await start(page);
      await overlaps(page);
      const footer = page.locator("footer");
      await footer.getByRole("button", { name: /^Pull/ }).click();
      const review = page.getByRole("dialog", { name: "Sync review" });
      await expect(review).toBeVisible();
      await review.getByText(/changed on GitHub only/).click();
      await shot(page, `sync-desktop-${scheme}-review`);

      await review.getByRole("group", { name: "What to keep of reading-list" }).getByRole("button", { name: "Keep both" }).click();
      await shot(page, `sync-desktop-${scheme}-review-chosen`);

      await review.getByRole("group", { name: "What to keep of reading-list" }).getByRole("button", { name: "Merge…" }).click();
      const merge = page.getByRole("dialog", { name: "Merge reading-list" });
      await expect(merge.locator(".mdgj-clash")).toBeVisible();
      await shot(page, `sync-desktop-${scheme}-merge-clash`);
      await merge.getByRole("button", { name: "Cancel" }).click();

      await review.getByRole("button", { name: "Check the merge" }).click();
      await expect(page.getByRole("dialog", { name: "Merge morning-run" })).toBeVisible();
      await shot(page, `sync-desktop-${scheme}-merge-clean`);
      await page.getByRole("dialog", { name: "Merge morning-run" }).getByRole("button", { name: "Done" }).click();

      await review.getByRole("button", { name: "Resolve with git…" }).click();
      await shot(page, `sync-desktop-${scheme}-resolve-with-git`);
      await review.getByRole("button", { name: "Back" }).click();

      await review.getByRole("button", { name: "Apply" }).click();
      await expect(review).toBeHidden();
      await shot(page, `sync-desktop-${scheme}-summary-notice`);
      await page.getByRole("status").getByRole("button", { name: "Details" }).click();
      await shot(page, `sync-desktop-${scheme}-summary`);
      await page.getByRole("dialog", { name: "Pull details" }).locator("button.primary").click();

      // A merge left for git.
      await page.evaluate(() => {
        const mock = (window as unknown as { mdgjMock: { remoteCommit(f: string, c: string, m: string): void } }).mdgjMock;
        mock.remoteCommit("2026-11-14-2130-ideas.md", "# Ideas\n\nA different first line on GitHub.\n", "Rewrite ideas");
      });
      await editOnDisk(page, IDEAS, "# Ideas\n\nA different first line here.\n");
      await refocus(page);
      await footer.getByRole("button", { name: /^Pull/ }).click();
      await review.getByRole("button", { name: "Resolve with git…" }).click();
      await review.getByLabel("Commit message for your changes").fill("Before merging");
      await review.getByRole("button", { name: "Start the merge" }).click();
      await expect(page.getByRole("status").filter({ hasText: "Merge in progress" })).toBeVisible();
      await shot(page, `sync-desktop-${scheme}-merge-in-progress`);
    });

    test("commit sheet with a diff", async ({ page }) => {
      await start(page);
      const mod = await modKey(page);
      await page.locator(".cm-content").click();
      await page.keyboard.press(`${mod}+End`);
      await page.keyboard.type("Ran again at dusk; the heron was back.");
      await editOnDisk(page, IDEAS, "# Ideas\n\nA journal that is just a folder of markdown files.\n\n**Bold** thoughts and _quiet_ ones.\n");
      await refocus(page);
      await page.keyboard.press(`${mod}+k`);
      const sheet = page.getByRole("dialog", { name: "Commit" });
      await sheet.getByRole("button", { name: IDEAS }).click();
      await expect(sheet.locator(".diff")).toBeVisible();
      await sheet.getByRole("checkbox", { name: /morning-run/ }).uncheck();
      await sheet.getByLabel("Commit message").fill("Ideas: shorter");
      await shot(page, `sync-desktop-${scheme}-commit-diff`);
    });
  });

  test.describe(`sync, phone, ${scheme}`, () => {
    const { defaultBrowserType: _, ...iPhone } = devices["iPhone 13"];
    test.use({ ...iPhone, viewport: { width: 390, height: 844 }, colorScheme: scheme, reducedMotion: "reduce" });

    test("review and merge view", async ({ page }) => {
      await page.goto("/");
      await page.getByRole("button", { name: "Settings" }).click();
      await page.getByRole("button", { name: "Connect GitHub" }).click();
      await page.getByRole("button", { name: "Sign in with GitHub" }).click();
      await page.getByRole("button", { name: /octocat\/journal/ }).click({ timeout: 15_000 });
      await expect(page.getByText("Connected to")).toBeVisible({ timeout: 15_000 });
      await page.getByRole("button", { name: "Close settings" }).click();
      await seedOverlaps(page);
      await page.getByRole("button", { name: /^Pull/ }).click();
      const review = page.getByRole("dialog", { name: "Sync review" });
      await expect(review).toBeVisible();
      await shot(page, `sync-phone-${scheme}-review`);

      await review.getByRole("group", { name: "What to keep of reading-list" }).getByRole("button", { name: "Merge…" }).click();
      const merge = page.getByRole("dialog", { name: "Merge reading-list" });
      await expect(merge.locator(".mdgj-clash")).toBeVisible();
      await shot(page, `sync-phone-${scheme}-merge-clash`);
      await merge.locator(".mdgj-clash").getByRole("button", { name: "Use both" }).click();
      await shot(page, `sync-phone-${scheme}-merge-picked`);
    });
  });
}
