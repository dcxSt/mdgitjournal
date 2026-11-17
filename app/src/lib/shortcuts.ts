// Global keyboard shortcuts (PLAN §7). Mod is Cmd on Apple platforms and Ctrl elsewhere.

export type Action = "new" | "toggleMode" | "quickOpen" | "search" | "pull" | "commit" | "toggleSidebar" | "settings";

export type KeyInput = Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey"> & {
  isComposing?: boolean;
};

const BINDINGS: { key: string; shift: boolean; action: Action }[] = [
  { key: "n", shift: false, action: "new" },
  { key: "e", shift: false, action: "toggleMode" },
  { key: "p", shift: false, action: "quickOpen" },
  { key: "f", shift: true, action: "search" },
  { key: "l", shift: true, action: "pull" },
  { key: "k", shift: false, action: "commit" },
  { key: "\\", shift: false, action: "toggleSidebar" },
  { key: ",", shift: false, action: "settings" },
];

const CODE_KEYS: Record<string, string> = { Backslash: "\\", Comma: "," };

/** The binding key for an event: the layout's character if it is one we bind, else the physical key. */
function bindingKey(e: KeyInput): string {
  const key = e.key.toLowerCase();
  if (/^[a-z]$/.test(key) || key === "\\" || key === ",") return key;
  if (/^Key[A-Z]$/.test(e.code)) return e.code.slice(3).toLowerCase();
  return CODE_KEYS[e.code] ?? key;
}

export function shortcutAction(e: KeyInput, mac: boolean): Action | null {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey || e.isComposing) return null;
  const key = bindingKey(e);
  return BINDINGS.find((b) => b.key === key && b.shift === e.shiftKey)?.action ?? null;
}

/** Mod keys Vim binds outside insert mode: Ctrl-E scrolls, Ctrl-N and Ctrl-P move down and up. */
const VIM_KEYS: readonly Action[] = ["toggleMode", "new", "quickOpen"];

/** On Linux (Mod is Ctrl) with Vim keys on, the editor keeps the Ctrl keys Vim uses. */
export function yieldsToVim(action: Action, target: EventTarget | null, mac: boolean, vim: boolean): boolean {
  return !mac && vim && VIM_KEYS.includes(action) && target instanceof Element && target.closest(".cm-editor") !== null;
}

export function shortcutLabel(action: Action, mac: boolean): string {
  const binding = BINDINGS.find((b) => b.action === action)!;
  const key = binding.key.toUpperCase();
  if (mac) return `⌘${binding.shift ? "⇧" : ""}${key}`;
  return `Ctrl+${binding.shift ? "Shift+" : ""}${key}`;
}

/** Commit sheet keys: Enter or Mod-Enter confirms, Shift-Enter is a newline, Esc cancels. */
export function commitSheetKey(e: KeyInput): "submit" | "cancel" | null {
  if (e.isComposing) return null;
  if (e.key === "Escape") return "cancel";
  if (e.key === "Enter" && !e.shiftKey && !e.altKey) return "submit";
  return null;
}
