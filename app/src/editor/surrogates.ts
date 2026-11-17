// Whole characters under vim. A character outside the Basic Multilingual Plane, most emoji among
// them, takes two UTF-16 units, and codemirror-vim sometimes counts units instead of characters:
// `p` puts between the two halves, `~` leaves the cursor there, and a visual range or `x` can end
// there. Half a character is not text and cannot be saved, so whatever vim reads, writes or
// selects is widened to whole characters. Everything else passes through untouched.

import {
  ChangeSet,
  type ChangeSpec,
  EditorSelection,
  EditorState,
  type Extension,
  type SelectionRange,
  type Text,
} from "@codemirror/state";
import { ViewPlugin } from "@codemirror/view";
import { getCM } from "@replit/codemirror-vim";

const isHigh = (unit: number) => unit >= 0xd800 && unit <= 0xdbff;
const isLow = (unit: number) => unit >= 0xdc00 && unit <= 0xdfff;
const unitAt = (doc: Text, pos: number) => doc.sliceString(pos, pos + 1).charCodeAt(0);

/** Whether `pos` falls between the two halves of a character. */
const splits = (doc: Text, pos: number) =>
  pos > 0 && isHigh(unitAt(doc, pos - 1)) && isLow(unitAt(doc, pos));
const back = (doc: Text, pos: number) => (splits(doc, pos) ? pos - 1 : pos);
const forward = (doc: Text, pos: number) => (splits(doc, pos) ? pos + 1 : pos);

/** A cursor moves past the character it is inside, as vim's own motions do; a range grows. */
function widen(doc: Text, range: SelectionRange): SelectionRange {
  const from = range.empty ? forward(doc, range.from) : back(doc, range.from);
  const to = forward(doc, range.to);
  if (from === range.from && to === range.to) return range;
  return range.anchor <= range.head ? EditorSelection.range(from, to) : EditorSelection.range(to, from);
}

const wholeEdits = EditorState.transactionFilter.of((tr) => {
  if (!tr.docChanged && !tr.selection) return tr;
  const start = tr.startState.doc;
  const doc = tr.newDoc;

  // Repairs, in the new document, of edits that leave half a character at an edge. An insert
  // moves past the character it split; a deletion takes the rest of the character with it.
  const repairs: ChangeSpec[] = [];
  tr.changes.iterChanges((fromA, toA, fromB, toB) => {
    const left = splits(start, fromA) && !isLow(unitAt(doc, fromB));
    const right = splits(start, toA) && !isHigh(unitAt(doc, toB - 1));
    if (fromA === toA) {
      if (left || right) {
        const rest = doc.sliceString(toB, toB + 1);
        repairs.push({ from: fromB, insert: rest }, { from: toB, to: toB + 1 });
      }
    } else {
      if (left) repairs.push({ from: fromB - 1, to: fromB });
      if (right) repairs.push({ from: toB, to: toB + 1 });
    }
  });
  const changes = ChangeSet.of(repairs, doc.length);
  const final = changes.apply(doc);
  const mapped = tr.newSelection.map(changes);
  const ranges = mapped.ranges.map((range) => widen(final, range));
  const moved = ranges.some((range, i) => range !== mapped.ranges[i]);
  if (changes.empty && !moved) return tr;
  const selection = moved ? EditorSelection.create(ranges, mapped.mainIndex) : undefined;
  return [tr, { changes, selection, sequential: true }];
});

// Vim reads the text it yanks and deletes through its CodeMirror 5 adapter, by the same offsets,
// so the reads are widened too and registers only ever hold whole characters.
const wholeReads = ViewPlugin.define((view) => {
  const cm = getCM(view);
  if (cm) {
    cm.getRange = (from, to) => {
      const doc = view.state.doc;
      return doc.sliceString(back(doc, cm.indexFromPos(from)), forward(doc, cm.indexFromPos(to)));
    };
  }
  return {};
});

/** Goes after vim(), whose adapter `wholeReads` patches. */
export const wholeCharacters: Extension = [wholeEdits, wholeReads];
