// A small JavaScript API over annox-core, compiled to WebAssembly.
// Build ./pkg with `just site` (wasm-pack). Offsets are UTF-16, like
// JavaScript strings and textarea selections; anchors keep code points.

import init, * as wasm from "./pkg/annox_wasm.js";

let ready;

/** Loads the WebAssembly module once. Every other function needs it. */
export function load() {
  ready ??= init();
  return ready;
}

/** The version of a text: `sha256:` and its hex digest (§3.4). */
export const version = (text) => wasm.version(text);

/** An anchor for `[start, end)` of `text` (§3.6). */
export const createAnchor = (text, start, end, path) =>
  JSON.parse(wasm.createAnchor(text, start, end, path));

/** Finds an anchor in `text` (§3.7): `{ state, step, start?, end? }`. */
export const resolve = (text, anchor) =>
  JSON.parse(wasm.resolve(text, JSON.stringify(anchor)));

/** Applies a suggestion (§4.3): the resolution, `applied`, and the new `text` and `version`. */
export const applySuggestion = (text, anchor, replacement) =>
  JSON.parse(wasm.applySuggestion(text, JSON.stringify(anchor), replacement));

/** Finds the text an accepted suggestion put in `text` (§4.3.3): `{ start, end }`, or `null`. */
export const appliedTextSearch = (text, anchor, replacement) =>
  JSON.parse(wasm.appliedTextSearch(text, JSON.stringify(anchor), replacement));

/** Derives every annotation from a list of events (§2.5.6), each with its `heads`. */
export const derive = (events) =>
  JSON.parse(wasm.derive(JSON.stringify(events)));

/** A new id: a UUIDv7 (§2.1). */
export function newId() {
  const b = crypto.getRandomValues(new Uint8Array(16));
  let t = Date.now();
  for (let i = 5; i >= 0; i--, t = Math.floor(t / 256)) b[i] = t % 256;
  b[6] = (b[6] & 0x0f) | 0x70;
  b[8] = (b[8] & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

/** The current time as an RFC 3339 UTC timestamp (§2.1). */
export const now = () => new Date().toISOString().replace(/\.\d+Z$/, "Z");
