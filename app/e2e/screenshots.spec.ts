// Screenshots of the main screens for a visual review: `MDGJ_SHOTS_DIR=/tmp/shots pnpm e2e`.

import { devices, expect, test, type Page } from "@playwright/test";
import { modKey, start } from "./helpers.ts";

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
