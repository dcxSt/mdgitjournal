// Pure helpers over entry metadata and text. No I/O.

import type { EntryMeta, SearchHit } from "./types";

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

export interface MonthGroup {
  /** "2026-11", or "other" for files that don't follow the naming scheme. */
  key: string;
  label: string;
  entries: EntryMeta[];
}

/** Groups entries by month in the order given (the backend sorts newest first); "Other" last. */
export function groupByMonth(entries: EntryMeta[]): MonthGroup[] {
  const groups = new Map<string, MonthGroup>();
  const other: EntryMeta[] = [];
  for (const entry of entries) {
    const key = entry.date?.slice(0, 7);
    if (!key) {
      other.push(entry);
      continue;
    }
    let group = groups.get(key);
    if (!group) {
      group = { key, label: monthLabel(key), entries: [] };
      groups.set(key, group);
    }
    group.entries.push(entry);
  }
  const result = [...groups.values()];
  if (other.length) result.push({ key: "other", label: "Other", entries: other });
  return result;
}

function monthLabel(key: string): string {
  const [year, month] = key.split("-");
  return `${MONTHS[Number(month) - 1] ?? month} ${year}`;
}

/** Day of month without a leading zero, or "" for "Other" entries. */
export function dayOfMonth(entry: EntryMeta): string {
  return entry.date ? String(Number(entry.date.slice(8, 10))) : "";
}

/** "2026-11-16 14:02", or null for "Other" entries. */
export function entryTimestamp(entry: EntryMeta): string | null {
  return entry.date && entry.time ? `${entry.date} ${entry.time}` : null;
}

/** Case-insensitive filename filter; every whitespace-separated word must occur in the name. */
export function filterByName(entries: EntryMeta[], query: string): EntryMeta[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return entries;
  return entries.filter((e) => {
    const name = e.file.toLowerCase();
    return words.every((w) => name.includes(w));
  });
}

/**
 * Subsequence match for quick-open. Higher is better; null means no match.
 * Consecutive characters and characters that start a "-" separated word score extra.
 */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  const t = text.toLowerCase();
  let score = 0;
  let from = 0;
  let prev = -2;
  for (const ch of q) {
    const i = t.indexOf(ch, from);
    if (i < 0) return null;
    score += i === prev + 1 ? 3 : 1;
    if (i === 0 || t[i - 1] === "-") score += 2;
    prev = i;
    from = i + ch.length;
  }
  // Prefer shorter names when everything else is equal.
  return score - t.length / 1000;
}

export function quickOpen(entries: EntryMeta[], query: string, limit = 50): EntryMeta[] {
  if (!query.trim()) return entries.slice(0, limit);
  return entries
    .map((entry) => ({ entry, score: fuzzyScore(query, entry.file) }))
    .filter((m): m is { entry: EntryMeta; score: number } => m.score !== null)
    .sort((a, b) => b.score - a.score)
    .slice(0, limit)
    .map((m) => m.entry);
}

/**
 * Best-effort preview of the backend's slugify. Returns null when the title contains characters
 * only the backend's transliteration knows how to fold, so the UI never shows a wrong preview.
 */
export function slugPreview(title: string): string | null {
  const folded = title.normalize("NFKD").replace(/\p{M}/gu, "");
  if (/[^\x00-\x7f]/.test(folded)) return null;
  const slug = folded
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug || "untitled";
}

const segmenter = new Intl.Segmenter(undefined, { granularity: "word" });

/** Unicode word count; markdown punctuation ("#", "-", "*") is not word-like, so it is ignored. */
export function wordCount(text: string): number {
  let count = 0;
  for (const segment of segmenter.segment(text)) if (segment.isWordLike) count++;
  return count;
}

/** Splits a search snippet around its match, converting code-point offsets to string slices. */
export function snippetParts(hit: SearchHit): { before: string; match: string; after: string } {
  const chars = Array.from(hit.snippet);
  return {
    before: chars.slice(0, hit.matchStart).join(""),
    match: chars.slice(hit.matchStart, hit.matchEnd).join(""),
    after: chars.slice(hit.matchEnd).join(""),
  };
}

export interface FileHits {
  file: string;
  title: string;
  /** Matches inside the text; a filename match (line 0) only puts the file in the list. */
  lines: SearchHit[];
}

/** Groups search hits by file, keeping the backend's order. */
export function groupHits(hits: SearchHit[]): FileHits[] {
  const byFile = new Map<string, FileHits>();
  for (const hit of hits) {
    let group = byFile.get(hit.file);
    if (!group) {
      group = { file: hit.file, title: hit.title, lines: [] };
      byFile.set(hit.file, group);
    }
    if (hit.line > 0) group.lines.push(hit);
  }
  return [...byFile.values()];
}
