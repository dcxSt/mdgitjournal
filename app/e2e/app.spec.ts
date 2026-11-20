import { devices, expect, test, type Page } from "@playwright/test";
import {
  addFile,
  callCount,
  connect,
  editorText,
  failNext,
  fileText,
  modKey,
  refocus,
  SAMPLE,
  start,
} from "./helpers.ts";

const entries = (page: Page) => page.getByRole("navigation", { name: "Entries" });
const selected = (page: Page) => entries(page).locator("button[aria-current=page]");

test("the entry list shows month groups, newest first, and Other last", async ({ page }) => {
  await start(page);
  const list = entries(page);
  await expect(list.locator("h2")).toHaveText(["November 2026", "October 2026", "Other"]);
  await expect(list.locator("button .title")).toHaveText(["morning-run", "ideas", "reading-list", "notes"]);
  await expect(selected(page)).toHaveAttribute("title", SAMPLE);
});

test("Mod-N creates an entry, and typing autosaves it once per pause", async ({ page }) => {
  await start(page);
  const mod = await modKey(page);
  await page.keyboard.press(`${mod}+n`);
  const title = page.getByLabel("New entry title");
  await expect(title).toBeFocused();
  await title.fill("Evening walk");
  await title.press("Enter");

  await expect(selected(page).locator(".title")).toHaveText("evening-walk");
  const file = (await selected(page).getAttribute("title"))!;
  expect(file).toMatch(/^\d{4}-\d{2}-\d{2}-\d{4}-evening-walk\.md$/);
  await expect(page.locator(".cm-content")).toBeFocused();
  expect(await fileText(page, file)).toBe("");

  const writes = await callCount(page, "writeEntry");
  await page.keyboard.type("Dusk by the canal.");
  await expect.poll(() => fileText(page, file)).toBe("Dusk by the canal.");
  await expect(page.locator("footer .save")).toHaveText("Saved");
  expect(await callCount(page, "writeEntry")).toBe(writes + 1);
  await expect(page.locator("footer .words")).toHaveText("4 words");
});

test("switching Source and Styled never changes the text", async ({ page }) => {
  await start(page);
  const mod = await modKey(page);
  const original = (await fileText(page, SAMPLE))!;
  const mode = page.getByRole("group", { name: "Editor mode" });

  for (let i = 0; i < 3; i++) {
    await mode.getByRole("button", { name: "Styled" }).click();
    await expect(page.locator(".cm-gutters")).toHaveCount(0);
    expect(await editorText(page)).toBe(original);
    await page.keyboard.press(`${mod}+e`);
    await expect(page.locator(".cm-gutters")).toHaveCount(1);
    expect(await editorText(page)).toBe(original);
  }
  expect(await callCount(page, "writeEntry")).toBe(0);
  expect(await fileText(page, SAMPLE)).toBe(original);
});

test("the commit sheet needs a message", async ({ page }) => {
  await start(page);
  const mod = await modKey(page);
  const commit = page.locator("footer").getByRole("button", { name: /^Commit/ });
  await expect(commit).toBeDisabled();

  await page.locator(".cm-content").click();
  await page.keyboard.press(`${mod}+End`);
  await page.keyboard.type("Ran again at dusk.");
  await expect(commit).toBeEnabled();
  await expect(commit.locator(".badge")).toHaveText("1");

  await page.keyboard.press(`${mod}+k`);
  const sheet = page.getByRole("dialog", { name: "Commit" });
  const message = sheet.getByLabel("Commit message");
  await expect(message).toBeFocused();
  await expect(message).toHaveValue("");
  await expect(sheet.getByText(SAMPLE)).toBeVisible();
  const confirm = sheet.getByRole("button", { name: "Commit", exact: true });
  await expect(confirm).toBeDisabled();
  await message.press("Enter");
  await expect(sheet).toBeVisible();
  await message.fill("   ");
  await expect(confirm).toBeDisabled();

  await message.fill("Evening run");
  await expect(confirm).toBeEnabled();
  await message.press("Enter");
  await expect(sheet).toBeHidden();
  await expect(page.getByText("Committed.")).toBeVisible();
  await expect(commit).toBeDisabled();
});

test("when GitHub is ahead, Pull lights up and Commit & Push says Pull first", async ({ page }) => {
  await start(page);
  await connect(page);
  const footer = page.locator("footer");
  await expect(footer.getByRole("button", { name: "octocat/journal · main" })).toBeVisible();

  // The mock lands two commits on GitHub after connecting; a fetch on focus sees them.
  await refocus(page);
  const pull = footer.getByRole("button", { name: /^Pull/ });
  await expect(pull.locator(".badge")).toHaveText("2");
  await expect(pull).toHaveClass(/accent/);

  await page.locator(".cm-content").click();
  await page.keyboard.type("Edited here too. ");
  const commit = footer.getByRole("button", { name: /^Commit & Push/ });
  await expect(commit.locator(".badge")).toHaveText("1");
  await expect(commit).toBeDisabled();
  await expect(footer.getByText("Pull first")).toBeVisible();

  await pull.click();
  await expect(pull.locator(".badge")).toHaveCount(0);
  await expect(footer.getByText("Pull first")).toBeHidden();
  await expect(commit).toBeEnabled();
  await expect(entries(page).getByRole("button", { name: /dawn/ })).toBeVisible();
});

test("with the red dot showing, Pull still pulls", async ({ page }) => {
  await start(page);
  await connect(page);
  await failNext(page, "fetch", "network", "Couldn't reach github.com.");
  await refocus(page);
  const pull = page.locator("footer").getByRole("button", { name: /^Pull/ });
  await expect(pull.locator(".dot")).toBeVisible();

  const pulls = await callCount(page, "pull");
  await pull.click();
  await expect(entries(page).getByRole("button", { name: /dawn/ })).toBeVisible();
  expect(await callCount(page, "pull")).toBe(pulls + 1);
  await expect(pull.locator(".dot")).toHaveCount(0);
});

test("focusing and leaving the title renames nothing, even a name not in slug form", async ({ page }) => {
  await start(page);
  const files = ["2026-11-15-0900-My_Notes.md", "2026-11-14-2130-ideas.from-github-1825.md"];
  for (const file of files) await addFile(page, file, "Text.\n");
  await refocus(page);
  const title = page.getByLabel("Title");
  for (const file of files) {
    await entries(page).locator(`button[title="${file}"]`).click();
    await expect(title).toHaveValue(file.slice(16, -3));
    await title.click();
    await page.locator(".cm-content").click();
  }
  expect(await callCount(page, "renameEntry")).toBe(0);
  for (const file of files) expect(await fileText(page, file)).toBe("Text.\n");

  // Typing a title does rename.
  await title.fill("My notes");
  await title.press("Enter");
  await expect(selected(page)).toHaveAttribute("title", "2026-11-14-2130-my-notes.md");
});

test("Settings keeps what was typed when closed with Esc or Mod-,", async ({ page }) => {
  await start(page);
  const mod = await modKey(page);
  const sheet = page.getByRole("dialog", { name: "Settings" });
  const fields = [
    { label: "Author name", value: "Stephen Fay", close: "Escape" },
    { label: "Author email", value: "s@example.com", close: `${mod}+Comma` },
    { label: "Check GitHub every", value: "10", close: "Escape" },
  ];
  for (const { label, value, close } of fields) {
    await page.keyboard.press(`${mod}+Comma`);
    await sheet.getByLabel(label).fill(value);
    await page.keyboard.press(close);
    await expect(sheet).toBeHidden();
    await page.keyboard.press(`${mod}+Comma`);
    await expect(sheet.getByLabel(label)).toHaveValue(value);
    await sheet.getByRole("button", { name: "Close settings" }).click();
  }
});

test("shortcuts do nothing behind the delete confirmation", async ({ page }) => {
  await start(page);
  const mod = await modKey(page);
  await page.locator(".cm-content").click();
  await page.keyboard.type("x");
  await page.getByRole("button", { name: "Entry actions" }).click();
  await page.getByRole("menuitem", { name: "Delete entry…" }).click();
  const sheet = page.getByRole("dialog", { name: "Delete entry" });
  await expect(sheet.getByText(SAMPLE)).toBeVisible();

  for (const key of ["k", "p", "Comma", "n", "e", "Backslash"]) await page.keyboard.press(`${mod}+${key}`);
  await expect(page.locator("dialog[open]")).toHaveCount(1);
  await expect(page.getByLabel("New entry title")).toHaveCount(0);
  await expect(page.locator(".cm-gutters")).toHaveCount(1);
  await expect(page.locator("aside")).toHaveCount(1);

  await sheet.getByRole("button", { name: "Cancel" }).click();
  await expect(sheet).toBeHidden();
  await expect.poll(() => fileText(page, SAMPLE)).toContain("x");
});

test("a full-text match deep inside a long line is visible in its result", async ({ page }) => {
  await start(page);
  const mod = await modKey(page);
  await page.locator(".cm-content").click();
  await page.keyboard.press(`${mod}+End`);
  await page.keyboard.type("We walked along the towpath to the lock, and saw a heron standing in the reeds by the far bank.");
  await expect.poll(() => fileText(page, SAMPLE)).toContain("heron");

  await page.keyboard.press(`${mod}+Shift+f`);
  await page.keyboard.type("heron");
  const snippet = page.locator(".snippet").first();
  const mark = snippet.locator("mark");
  await expect(mark).toHaveText("heron");
  const [row, match] = [await snippet.boundingBox(), await mark.boundingBox()];
  expect(match!.x + match!.width).toBeLessThanOrEqual(row!.x + row!.width);
});

test("a failed save says why in a notice, which goes once a retry succeeds", async ({ page }) => {
  await start(page);
  await failNext(page, "writeEntry", "io", "Disk full (os error 28)");
  await page.locator(".cm-content").click();
  await page.keyboard.type("x");
  const alert = page.getByRole("alert").filter({ hasText: "Disk full (os error 28)" });
  await expect(alert).toBeVisible();
  await expect(page.locator("footer .save")).toHaveText("Not saved");
  await expect(alert).toBeHidden({ timeout: 10_000 });
  await expect(page.locator("footer .save")).toHaveText("Saved");
});

test("keyboard focus shows a ring in the accent colour, inside list rows", async ({ page }) => {
  await start(page);
  const row = entries(page).getByRole("button", { name: /ideas/ });
  await page.keyboard.press("Shift"); // keyboard modality, so programmatic focus is focus-visible
  await row.focus();
  const ring = await row.evaluate((el) => {
    const style = getComputedStyle(el);
    const accent = document.createElement("i");
    accent.style.color = "var(--color-accent)";
    el.append(accent);
    const color = getComputedStyle(accent).color;
    accent.remove();
    return { outline: style.outlineColor, offset: style.outlineOffset, accent: color };
  });
  expect(ring.outline).toBe(ring.accent);
  expect(ring.offset).toBe("-2px");
});

test.describe("phone layout", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test("lists entries, opens one in Styled mode at the phone size, and goes back", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: /morning-run/ }).click();
    const editor = page.locator(".cm-editor");
    await expect(editor).toBeVisible();
    await expect(page.locator(".cm-gutters")).toHaveCount(0);
    await expect(editor).toHaveCSS("font-size", "17px");

    await page.getByRole("button", { name: "Entries" }).click();
    await expect(page.getByRole("button", { name: "New entry" })).toBeVisible();
  });
});

test.describe("iPhone in landscape", () => {
  const { viewport, userAgent, deviceScaleFactor, isMobile, hasTouch } = devices["iPhone 13 landscape"];
  test.use({ viewport, userAgent, deviceScaleFactor, isMobile, hasTouch });

  test("still gets the phone's sheets and 16px fields, though wider than 700px", async ({ page }) => {
    await page.goto("/");
    expect(page.viewportSize()!.width).toBeGreaterThan(700);
    await page.getByRole("button", { name: "Settings" }).click();
    const sheet = page.getByRole("dialog", { name: "Settings" });
    await expect(sheet).toBeVisible();
    expect((await sheet.boundingBox())!.width).toBe(page.viewportSize()!.width);
    await expect(sheet.getByLabel("Author name")).toHaveCSS("font-size", "16px");
  });
});

test("desktop fields keep the desktop text size", async ({ page }) => {
  await start(page);
  await page.locator("footer").getByRole("button", { name: "Settings", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Settings" }).getByLabel("Author name")).toHaveCSS("font-size", "13px");
});
