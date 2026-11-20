import { expect, type Page } from "@playwright/test";

/** The parts of `window.mdgjMock` (src/lib/mock.ts) the tests use. */
interface MockWindow {
  mdgjMock: {
    files: Map<string, string>;
    calls: string[];
    failNext(method: string, kind: string, message: string): void;
    remoteCommit(file: string, content: string | null, message?: string): void;
    editOnDisk(file: string, content: string | null): void;
    finishMergeWithGit(): void;
    seedOverlaps(): { clean: string; clash: string };
  };
}

export const SAMPLE = "2026-11-16-1402-morning-run.md";
export const IDEAS = "2026-11-14-2130-ideas.md";
export const LIST = "2026-10-28-0815-reading-list.md";

/** Every file in the mock's journal folder, to compare before and after. */
export function allFiles(page: Page): Promise<[string, string][]> {
  return page.evaluate(() => [...(window as unknown as MockWindow).mdgjMock.files].sort(([a], [b]) => a.localeCompare(b)));
}

/** A commit lands on GitHub (null deletes the file). */
export function remoteCommit(page: Page, file: string, content: string | null, message?: string): Promise<void> {
  return page.evaluate(
    ([f, c, m]) => (window as unknown as MockWindow).mdgjMock.remoteCommit(f!, c, m ?? undefined),
    [file, content, message ?? null] as const,
  );
}

/** An edit made outside the app, as vim or Finder would. */
export function editOnDisk(page: Page, file: string, content: string | null): Promise<void> {
  return page.evaluate(([f, c]) => (window as unknown as MockWindow).mdgjMock.editOnDisk(f!, c), [file, content] as const);
}

/** As `git add -A && git commit` in a terminal. */
export function finishMergeWithGit(page: Page): Promise<void> {
  return page.evaluate(() => (window as unknown as MockWindow).mdgjMock.finishMergeWithGit());
}

/**
 * GitHub and this device both edit two entries: the morning run in different paragraphs (it
 * merges cleanly) and the same line of the reading list (a clash). Then the window regains focus.
 */
export async function seedOverlaps(page: Page): Promise<{ clean: string; clash: string }> {
  const seeded = await page.evaluate(() => (window as unknown as MockWindow).mdgjMock.seedOverlaps());
  await refocus(page);
  return seeded;
}

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

/** Puts a file in the mock's journal folder, as git or another program would. */
export function addFile(page: Page, file: string, content: string): Promise<void> {
  return page.evaluate(([f, c]) => void (window as unknown as MockWindow).mdgjMock.files.set(f, c), [file, content]);
}

/** Makes the next call of `method` fail. */
export function failNext(page: Page, method: string, kind: string, message: string): Promise<void> {
  return page.evaluate(
    ([m, k, text]) => (window as unknown as MockWindow).mdgjMock.failNext(m, k, text),
    [method, kind, message],
  );
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
