import { expect, test, type Page } from "@playwright/test";
import { callCount, connect, editorText, fileText, modKey, refocus, SAMPLE, start } from "./helpers.ts";

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
