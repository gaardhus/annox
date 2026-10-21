// The words that changed within a suggestion, diff-so-fancy style, the same as
// the Neovim plugin's and the PDF export's. Free of the VS Code API so it can
// be unit tested. Offsets are UTF-16 code units.

import { diffArrays } from "diff";

/** A range `[start, end)` of a string. */
export type Span = [number, number];

/** Letters, digits, underscore, and anything outside ASCII. */
function isWord(code: number): boolean {
  return code >= 0x80 || (code >= 0x30 && code <= 0x39) || (code >= 0x41 && code <= 0x5a) || (code >= 0x61 && code <= 0x7a) || code === 0x5f;
}

function isSpace(code: number): boolean {
  return code === 0x20 || code === 0x09;
}

/** `s` split into words, runs of spaces and tabs, and single other characters. */
function tokens(s: string): Span[] {
  const out: Span[] = [];
  let i = 0;
  while (i < s.length) {
    const first = s.charCodeAt(i);
    const same = isWord(first) ? isWord : isSpace(first) ? isSpace : () => false;
    let j = i + 1;
    while (j < s.length && same(s.charCodeAt(j))) j++;
    out.push([i, j]);
    i = j;
  }
  return out;
}

/**
 * The words that differ between `old` and `new`, as spans of each. None when
 * either is empty, or when the two share no word, as then everything changed.
 * Changes only spaces apart are joined.
 */
export function wordChanges(old: string, next: string): { deleted: Span[]; inserted: Span[] } {
  const none = { deleted: [], inserted: [] };
  if (old === "" || next === "") return none;
  const ta = tokens(old);
  const tb = tokens(next);
  const deleted: Span[] = [];
  const inserted: Span[] = [];
  let shared = false;
  let ia = 0;
  let ib = 0;
  const add = (out: Span[], s: string, span: Span) => {
    const last = out[out.length - 1];
    if (last && /^[ \t]*$/.test(s.slice(last[1], span[0]))) last[1] = span[1];
    else out.push([...span]);
  };
  const parts = diffArrays(
    ta.map(([i, j]) => old.slice(i, j)),
    tb.map(([i, j]) => next.slice(i, j)),
  );
  for (const part of parts) {
    const n = part.count ?? part.value.length;
    if (part.removed) {
      for (let k = 0; k < n; k++) add(deleted, old, ta[ia + k]);
      ia += n;
    } else if (part.added) {
      for (let k = 0; k < n; k++) add(inserted, next, tb[ib + k]);
      ib += n;
    } else {
      for (let k = 0; k < n; k++) shared ||= isWord(old.charCodeAt(ta[ia + k][0]));
      ia += n;
      ib += n;
    }
  }
  if (!shared) return none;
  return { deleted: trim(old, deleted), inserted: trim(next, inserted) };
}

/** `spans` without spaces at their ends: which of two equal spaces counts as
 * changed is arbitrary, so neither does. */
function trim(s: string, spans: Span[]): Span[] {
  return spans
    .map(([i, j]): Span => {
      while (i < j && isSpace(s.charCodeAt(i))) i++;
      while (j > i && isSpace(s.charCodeAt(j - 1))) j--;
      return [i, j];
    })
    .filter(([i, j]) => j > i);
}

/** `text` split at `spans` into pieces, each marked whether it changed. */
export function pieces(text: string, spans: Span[]): { text: string; changed: boolean }[] {
  const out: { text: string; changed: boolean }[] = [];
  let at = 0;
  for (const [i, j] of spans) {
    if (i > at) out.push({ text: text.slice(at, i), changed: false });
    out.push({ text: text.slice(i, j), changed: true });
    at = j;
  }
  if (at < text.length) out.push({ text: text.slice(at), changed: false });
  return out;
}
