// The markdown language both editors use, and its highlighting.

import { markdown } from "@codemirror/lang-markdown";
import { languageDataProp } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { GFM } from "@lezer/markdown";
import { highlighters } from "./theme";

// No autocompletion, list continuation, link-on-paste or tag closing: every inserted byte is one
// the user typed, and Enter never renumbers the lines below it.
export const markdownSupport = markdown({
  extensions: GFM,
  codeLanguages: languages,
  addKeymap: false,
  completeHTMLTags: false,
  pasteURLAsLink: false,
});

export const highlighting = highlighters(
  (type) => type.prop(languageDataProp) === markdownSupport.language.data,
);
