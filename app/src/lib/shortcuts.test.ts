import { describe, expect, it } from "vitest";
import { commitSheetKey, shortcutAction, shortcutLabel, type Action, type KeyInput } from "./shortcuts";

function key(k: string, mods: Partial<KeyInput> = {}): KeyInput {
  const code = /^[a-z]$/i.test(k) ? `Key${k.toUpperCase()}` : ({ "\\": "Backslash", ",": "Comma" }[k] ?? k);
  return { key: k, code, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...mods };
}

const TABLE: [string, boolean, Action][] = [
  ["n", false, "new"],
  ["e", false, "toggleMode"],
  ["p", false, "quickOpen"],
  ["F", true, "search"],
  ["L", true, "pull"],
  ["k", false, "commit"],
  ["\\", false, "toggleSidebar"],
  [",", false, "settings"],
];

describe("shortcutAction", () => {
  it.each(TABLE)("maps Cmd+%s (shift %s) to %s on macOS", (k, shiftKey, action) => {
    expect(shortcutAction(key(k, { metaKey: true, shiftKey }), true)).toBe(action);
  });

  it.each(TABLE)("maps Ctrl+%s (shift %s) to %s on Linux", (k, shiftKey, action) => {
    expect(shortcutAction(key(k, { ctrlKey: true, shiftKey }), false)).toBe(action);
  });

  it("uses only the platform's Mod key", () => {
    expect(shortcutAction(key("n", { ctrlKey: true }), true)).toBeNull();
    expect(shortcutAction(key("n", { metaKey: true }), false)).toBeNull();
    expect(shortcutAction(key("n", { metaKey: true, ctrlKey: true }), true)).toBeNull();
  });

  it("needs the exact Shift state and no Alt", () => {
    expect(shortcutAction(key("f", { metaKey: true }), true)).toBeNull();
    expect(shortcutAction(key("N", { metaKey: true, shiftKey: true }), true)).toBeNull();
    expect(shortcutAction(key("n", { metaKey: true, altKey: true }), true)).toBeNull();
  });

  it("ignores plain keys and IME composition", () => {
    expect(shortcutAction(key("n"), true)).toBeNull();
    expect(shortcutAction(key("n", { metaKey: true, isComposing: true }), true)).toBeNull();
  });

  it("falls back to the physical key on non-Latin layouts", () => {
    expect(shortcutAction({ ...key("n", { ctrlKey: true }), key: "т" }, false)).toBe("new");
  });
});

describe("shortcutLabel", () => {
  it("uses symbols on Apple platforms and words elsewhere", () => {
    expect(shortcutLabel("new", true)).toBe("⌘N");
    expect(shortcutLabel("search", true)).toBe("⌘⇧F");
    expect(shortcutLabel("search", false)).toBe("Ctrl+Shift+F");
    expect(shortcutLabel("toggleSidebar", false)).toBe("Ctrl+\\");
  });
});

describe("commitSheetKey", () => {
  it("confirms with Enter or Mod-Enter, cancels with Esc", () => {
    expect(commitSheetKey(key("Enter"))).toBe("submit");
    expect(commitSheetKey(key("Enter", { metaKey: true }))).toBe("submit");
    expect(commitSheetKey(key("Enter", { ctrlKey: true }))).toBe("submit");
    expect(commitSheetKey(key("Escape"))).toBe("cancel");
  });

  it("leaves Shift-Enter (a new line) and composition alone", () => {
    expect(commitSheetKey(key("Enter", { shiftKey: true }))).toBeNull();
    expect(commitSheetKey(key("Enter", { isComposing: true }))).toBeNull();
    expect(commitSheetKey(key("a"))).toBeNull();
  });
});
