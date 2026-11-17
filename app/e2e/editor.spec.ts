// The editor in the real UI, where layout and keys are WebKit's own (jsdom has neither).

import { expect, test, type Page } from "@playwright/test";
import { addFile, fileText, modKey, refocus, SAMPLE, start } from "./helpers.ts";

/** Adds `file` to the journal and opens it from the entry list. */
async function openNew(page: Page, file: string, content: string): Promise<void> {
  await addFile(page, file, content);
  await refocus(page);
  const list = page.getByRole("navigation", { name: "Entries" });
  await list.locator(`button[title="${file}"]`).click();
  await expect(list.locator("button[aria-current=page]")).toHaveAttribute("title", file);
  await expect(page.locator(".cm-line").first()).toContainText(content.split(/\r?\n/)[0]);
}

async function enableVim(page: Page): Promise<void> {
  await page.locator("footer").getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByText("Vim keys").click();
  await page.getByRole("button", { name: "Close settings" }).click();
}

/** Errors the page logs; vim reports a key it failed on there, then types it as text. */
function pageErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("console", (m) => void (m.type() === "error" && errors.push(m.text())));
  page.on("pageerror", (e) => errors.push(String(e)));
  return errors;
}

test("~~ marks are never struck through, in Source or in Styled", async ({ page }) => {
  await start(page);
  await openNew(page, "2026-11-15-1000-todo.md", "Not done: ~~buy gloves~~ and call home.\n");
  const mod = await modKey(page);
  for (const mode of ["source", "styled"]) {
    const struck = await page.locator(".cm-line").first().evaluate((line) => {
      const out: [string, boolean][] = [];
      const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        let through = false;
        for (let e = n.parentElement; e && e !== line; e = e.parentElement) {
          through ||= getComputedStyle(e).textDecorationLine.includes("line-through");
        }
        out.push([n.textContent!, through]);
      }
      return out;
    });
    expect(struck.filter(([text]) => text === "~~"), mode).toEqual([
      ["~~", false],
      ["~~", false],
    ]);
    expect(struck.find(([text]) => text.includes("gloves")), mode).toEqual(["buy gloves", true]);
    await page.keyboard.press(`${mod}+e`);
  }
});

test("in Styled, ~~ never wraps away from the word it marks", async ({ page }) => {
  await start(page);
  await openNew(page, "2026-11-15-1000-todo.md", "Things I meant to do this week and never did: ~~buy gloves~~ and call home.\n");
  await page.getByRole("group", { name: "Editor mode" }).getByRole("button", { name: "Styled" }).click();
  await expect(page.locator(".cm-gutters")).toHaveCount(0);
  // Every width from 300 to 700 px; a line break between two non-spaces is a bad wrap.
  const bad = await page.locator(".editor").evaluate(async (host: HTMLElement) => {
    const out: number[] = [];
    host.style.flex = "none";
    for (let width = 300; width <= 700; width += 2) {
      host.style.width = `${width}px`;
      await new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
      const chars: { c: string; mid: number }[] = [];
      const line = host.querySelector(".cm-line")!;
      const walker = document.createTreeWalker(line, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        for (let i = 0; i < n.textContent!.length; i++) {
          const range = document.createRange();
          range.setStart(n, i);
          range.setEnd(n, i + 1);
          const rects = range.getClientRects();
          const rect = rects[rects.length - 1] ?? range.getBoundingClientRect();
          chars.push({ c: n.textContent![i], mid: (rect.top + rect.bottom) / 2 });
        }
      }
      for (let i = 1; i < chars.length; i++) {
        const [a, b] = [chars[i - 1], chars[i]];
        if (Math.abs(b.mid - a.mid) > 8 && a.c !== " " && b.c !== " ") out.push(width);
      }
    }
    return out;
  });
  expect(bad).toEqual([]);
});

test("Vim: after G in a long entry, gg and G in a short one move and type nothing", async ({ page }) => {
  const errors = pageErrors(page);
  await start(page);
  await enableVim(page);
  const long = `${Array.from({ length: 120 }, (_, i) => `Paragraph ${i} of a long entry.`).join("\n\n")}\n`;
  await openNew(page, "2026-11-16-1500-long.md", long);
  await page.locator(".cm-content").click();
  await page.keyboard.press("Escape");
  await page.keyboard.press("Shift+G");

  const before = await fileText(page, SAMPLE);
  await page.getByRole("navigation", { name: "Entries" }).locator(`button[title="${SAMPLE}"]`).click();
  await expect(page.locator(".cm-content")).toContainText("Morning run");
  await page.locator(".cm-content").click();
  await page.keyboard.press("Escape");
  await page.keyboard.type("gg");
  await page.keyboard.press("Shift+G");
  await page.keyboard.type("gg");
  await page.waitForTimeout(800); // longer than autosave's pause
  expect(await fileText(page, SAMPLE)).toBe(before);
  expect(errors).toEqual([]);
});

test("Vim: a put next to an emoji keeps it whole", async ({ page }) => {
  await start(page);
  await enableVim(page);
  await openNew(page, "2026-11-15-1000-run.md", "great run 🙂\n");
  await page.locator(".cm-line").first().click();
  await page.keyboard.press("Escape");
  await page.keyboard.type("0yiw$p");
  await expect.poll(() => fileText(page, "2026-11-15-1000-run.md")).toBe("great run 🙂great\n");
});

test("Vim: a put over a backward visual selection replaces it, and types nothing", async ({ page }) => {
  const errors = pageErrors(page);
  await start(page);
  await enableVim(page);
  const file = "2026-11-15-1000-hello.md";
  await openNew(page, file, "hello world\n");
  await page.locator(".cm-line").first().click();
  await page.keyboard.press("Escape");
  // codemirror-vim used to throw on the put, after deleting the selection; then "p" was typed.
  await page.keyboard.type("0yiwwvbp");
  await expect.poll(() => fileText(page, file)).toBe("helloorld\n");
  expect(errors).toEqual([]);
});

test("in a file with mixed line breaks, text typed or a new line at the end of a CRLF line keeps the CRLF together", async ({
  page,
}) => {
  await start(page);
  const file = "2026-11-15-1000-mixed.md";
  await openNew(page, file, "alpha\r\nbeta\ngamma\n");
  await page.locator(".cm-line").first().click();
  await page.keyboard.press("End");
  await page.keyboard.type("XY");
  await expect.poll(() => fileText(page, file)).toBe("alphaXY\r\nbeta\ngamma\n");
  await page.keyboard.press("Enter");
  await expect.poll(() => fileText(page, file)).toBe("alphaXY\r\n\nbeta\ngamma\n");
});

test("Settings › About shows the third-party notices", async ({ page }) => {
  await start(page);
  await page.locator("footer").getByRole("button", { name: "Settings", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "Settings" });
  await expect(sheet.getByText("based in part on the work of the Independent JPEG Group")).toBeVisible();
  await sheet.getByText("Third-party notices").click();
  await expect(sheet.locator("pre")).toContainText("# Third-party notices");
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, hasTouch: true });

  test("a photo from the picker goes next to the selected text, which stays", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: /morning-run/ }).click();
    const line = page.locator(".cm-line").filter({ hasText: "Five miles" });
    await line.click();
    await page.keyboard.press("Home");
    for (let i = 0; i < 4; i++) await page.keyboard.press("Shift+ArrowRight");
    const png = Buffer.from(
      "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
      "base64",
    );
    await page.locator('input[type="file"]').setInputFiles({ name: "shot.png", mimeType: "image/png", buffer: png });
    await expect
      .poll(() => fileText(page, SAMPLE))
      .toMatch(/\n\nFive\n!\[\]\(assets\/2026-11-16-1402-morning-run-[0-9a-f]{8}\.jpg\)\n miles along the river\./);
  });
});
