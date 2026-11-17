import { expect, type Page } from "@playwright/test";

/** The parts of `window.mdgjMock` (src/lib/mock.ts) the tests read. */
interface MockWindow {
  mdgjMock: { files: Map<string, string>; calls: string[] };
}

export const SAMPLE = "2026-11-16-1402-morning-run.md";

/** Loads the app on the mock backend and waits until the first entry is open. */
export async function start(page: Page): Promise<void> {
  await page.goto("/");
  await expect(page.locator(".cm-content")).toBeVisible();
}

/** The app's Mod key, which follows the platform the mock reports. */
export async function modKey(page: Page): Promise<"Meta" | "Control"> {
  const label = await page.locator("aside .new kbd").textContent();
  return label?.startsWith("⌘") ? "Meta" : "Control";
}

export function fileText(page: Page, file: string): Promise<string | undefined> {
  return page.evaluate((f) => (window as unknown as MockWindow).mdgjMock.files.get(f), file);
}

export function callCount(page: Page, method: string): Promise<number> {
  return page.evaluate(
    (m) => (window as unknown as MockWindow).mdgjMock.calls.filter((c) => c === m).length,
    method,
  );
}

/** The editor's text as rendered, one `.cm-line` per line (fine for entries that fit the view). */
export async function editorText(page: Page): Promise<string> {
  return (await page.locator(".cm-line").allTextContents()).join("\n");
}

/** Settings → Connect GitHub → device flow → the mock's only repository. */
export async function connect(page: Page): Promise<void> {
  await page.locator("footer").getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Connect GitHub" }).click();
  await page.getByRole("button", { name: "Sign in with GitHub" }).click();
  await expect(page.getByLabel("Your code")).toHaveText("WDJB-MJHT");
  await page.getByRole("button", { name: /octocat\/journal/ }).click({ timeout: 15_000 });
  await expect(page.getByText("Connected to")).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: "Close settings" }).click();
}

/** What the window regaining focus does: re-list, then fetch. */
export async function refocus(page: Page): Promise<void> {
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
}
