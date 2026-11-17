import { describe, expect, it } from "vitest";
import { createEditor } from ".";

const options = { mode: "source", vim: false, dark: false, mobile: false, onChange() {} } as const;

describe("editor", () => {
  it.each(["", "# title\r\nbody\r\n", "no final newline", "mixed\r\nline\nends\r", "lone\rcr"])(
    "returns %j unchanged after mode, vim and theme toggles",
    (doc) => {
      const editor = createEditor(document.createElement("div"), { ...options, doc });
      editor.setMode("styled");
      editor.setVim(true);
      editor.setDark(true);
      editor.setMode("source");
      expect(editor.getDoc()).toBe(doc);
      editor.destroy();
    },
  );
});
