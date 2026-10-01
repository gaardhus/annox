// Text helpers for suggestion mode, free of the VS Code API so they can be
// unit tested. Offsets are UTF-16 code units, which is also the position
// encoding the language client negotiates.

import { diffLines } from "diff";
import type { Position } from "./types.ts";

/** A change from `base` to the edited text. */
export interface Change {
  /** `[a0, a1)` is the range of the base text that is replaced. */
  a0: number;
  a1: number;
  /** `[b0, b1)` is the same stretch in the edited text. */
  b0: number;
  b1: number;
  /** Where the edit itself ended in the base text, before widening. */
  editedEnd: number;
  replacement: string;
}

function isLowSurrogate(text: string, i: number): boolean {
  const c = text.charCodeAt(i);
  return c >= 0xdc00 && c <= 0xdfff;
}

/** Letters, digits, underscore, and anything non-ASCII, so that a
 * character outside ASCII is never split from its word. */
function isWord(text: string, i: number): boolean {
  if (i < 0 || i >= text.length) return false;
  const c = text.charCodeAt(i);
  return c >= 0x80 || (c >= 0x30 && c <= 0x39) || (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a) || c === 0x5f;
}

/**
 * The changes from `a` to `b`, each widened to whole words (§4.6): changing
 * `pd` to `pl` is `pd` → `pl`, not `d` → `l`.
 */
export function changes(a: string, b: string): Change[] {
  const out: Change[] = [];
  let ia = 0;
  let ib = 0;
  const parts = diffLines(a, b);
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i];
    if (!part.added && !part.removed) {
      ia += part.value.length;
      ib += part.value.length;
      continue;
    }
    // A hunk is a run of removed and added parts.
    let ra = 0;
    let rb = 0;
    while (i < parts.length && (parts[i].added || parts[i].removed)) {
      if (parts[i].removed) ra += parts[i].value.length;
      else rb += parts[i].value.length;
      i++;
    }
    i--;
    const hunkStart = ia;
    let a0 = ia;
    let a1 = ia + ra;
    let b0 = ib;
    let b1 = ib + rb;
    ia = a1;
    ib = b1;
    // Trim what the old and new text share, keeping whole characters.
    while (a0 < a1 && b0 < b1 && a[a0] === b[b0]) {
      a0++;
      b0++;
    }
    while (a1 > a0 && b1 > b0 && a[a1 - 1] === b[b1 - 1]) {
      a1--;
      b1--;
    }
    // A pure insertion or deletion can often sit in several places ("wo t"
    // or "two " in "one two three"). Prefer one that starts and ends at word
    // boundaries, so that widening doesn't grow it.
    if (a0 === a1 || b0 === b1) {
      const [t, s, e] = a0 === a1 ? [b, b0, b1] : [a, a0, a1];
      const boundary = (i: number) => !(isWord(t, i - 1) && isWord(t, i));
      let left = 0;
      while (s - left > 0 && Math.min(a0, b0) - left > 0 && t[s - left - 1] === t[e - left - 1]) left++;
      let right = 0;
      while (e + right < t.length && t[s + right] === t[e + right]) right++;
      // The aligned spot nearest to where the diff put it, else the leftmost.
      let shift = -left;
      for (let d = 0; d <= Math.max(left, right); d++) {
        const aligned = (x: number) => x >= -left && x <= right && boundary(s + x) && boundary(e + x);
        if (aligned(-d)) {
          shift = -d;
          break;
        }
        if (aligned(d)) {
          shift = d;
          break;
        }
      }
      a0 += shift;
      a1 += shift;
      b0 += shift;
      b1 += shift;
    }
    while (a0 > hunkStart && isLowSurrogate(a, a0)) {
      a0--;
      b0--;
    }
    while (a1 < a.length && isLowSurrogate(a, a1)) {
      a1++;
      b1++;
    }
    const editedEnd = a1;
    // Widen a change that starts or ends inside a word to the whole word.
    // The text around the change is the same in both versions.
    if ((isWord(a, a0) && a0 < a1) || (isWord(b, b0) && b0 < b1)) {
      while (a0 > 0 && b0 > 0 && isWord(a, a0 - 1) && a[a0 - 1] === b[b0 - 1]) {
        a0--;
        b0--;
      }
    }
    if ((isWord(a, a1 - 1) && a0 < a1) || (isWord(b, b1 - 1) && b0 < b1)) {
      while (isWord(a, a1) && a[a1] === b[b1]) {
        a1++;
        b1++;
      }
    }
    if (a0 !== a1 || b0 !== b1) {
      out.push({ a0, a1, b0, b1, editedEnd, replacement: b.slice(b0, b1) });
    }
  }
  return out;
}

/** Offsets of the start of each line of `text`. Line breaks are `\n`, `\r\n`
 * and `\r`, as in LSP. */
export function lineStarts(text: string): number[] {
  const starts = [0];
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === "\r" && text[i + 1] === "\n") {
      i++;
      starts.push(i + 1);
    } else if (c === "\n" || c === "\r") {
      starts.push(i + 1);
    }
  }
  return starts;
}

/** The LSP position of `offset` in the text whose `lineStarts` are `starts`. */
export function positionAt(starts: number[], offset: number): Position {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= offset) lo = mid;
    else hi = mid - 1;
  }
  return { line: lo, character: offset - starts[lo] };
}

/** The offset of LSP position `pos` in `text`, clamped to its line. */
export function offsetAt(text: string, starts: number[], pos: Position): number {
  if (pos.line >= starts.length) return text.length;
  const start = starts[pos.line];
  let end = pos.line + 1 < starts.length ? starts[pos.line + 1] : text.length;
  // Don't count the line break as part of the line.
  while (end > start && (text[end - 1] === "\n" || text[end - 1] === "\r")) end--;
  return Math.min(start + pos.character, end);
}
