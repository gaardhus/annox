import assert from "node:assert/strict";
import { test } from "node:test";
import { changes, lineStarts, offsetAt, positionAt } from "../../src/diff.ts";

function apply(a: string, b: string): { from: string; to: string }[] {
  return changes(a, b).map((c) => ({ from: a.slice(c.a0, c.a1), to: c.replacement }));
}

test("a change inside a word is widened to the word", () => {
  assert.deepEqual(apply("we use pd here\n", "we use pl here\n"), [{ from: "pd", to: "pl" }]);
});

test("an insertion between words stays an insertion", () => {
  assert.deepEqual(apply("one three\n", "one two three\n"), [{ from: "", to: "two " }]);
});

test("a deletion of a whole word", () => {
  assert.deepEqual(apply("one two three\n", "one three\n"), [{ from: "two ", to: "" }]);
});

test("separate lines give separate changes", () => {
  assert.deepEqual(apply("alpha\nbeta\ngamma\ndelta\n", "alphx\nbeta\ngamma\ndeltx\n"), [
    { from: "alpha", to: "alphx" },
    { from: "delta", to: "deltx" },
  ]);
});

test("a change spanning a line break", () => {
  assert.deepEqual(apply("first line\nsecond line\n", "first second line\n"), [{ from: "line\n", to: "" }]);
});

test("non-ASCII text is not split", () => {
  assert.deepEqual(apply("naïve café\n", "naïve cafe\n"), [{ from: "café", to: "cafe" }]);
  assert.deepEqual(apply("a 😀 b\n", "a 😃 b\n"), [{ from: "😀", to: "😃" }]);
});

test("CRLF text", () => {
  assert.deepEqual(apply("one\r\ntwo\r\n", "one\r\ntoo\r\n"), [{ from: "two", to: "too" }]);
});

test("editedEnd marks where the edit ended before widening", () => {
  const [c] = changes("abcdef\n", "abXdef\n");
  assert.equal(c.a0, 0);
  assert.equal(c.a1, 6);
  assert.equal(c.editedEnd, 3);
});

test("positions and offsets round-trip", () => {
  const text = "ab\r\ncd\ne\rf";
  const starts = lineStarts(text);
  assert.deepEqual(starts, [0, 4, 7, 9]);
  for (let off = 0; off <= text.length; off++) {
    const pos = positionAt(starts, off);
    if (text[off - 1] === "\r" && text[off] === "\n") continue;
    assert.equal(offsetAt(text, starts, pos), Math.min(off, offsetAt(text, starts, { line: pos.line, character: 1e9 })));
  }
  assert.deepEqual(positionAt(starts, 5), { line: 1, character: 1 });
  assert.equal(offsetAt(text, starts, { line: 0, character: 10 }), 2);
  assert.equal(offsetAt(text, starts, { line: 9, character: 0 }), text.length);
});

test("typing a word at the end of a line", () => {
  assert.deepEqual(apply("hello\n", "hello world\n"), [{ from: "", to: " world" }]);
});

test("doubling a letter stays inside its word", () => {
  assert.deepEqual(apply("a bok here\n", "a book here\n"), [{ from: "bok", to: "book" }]);
});
