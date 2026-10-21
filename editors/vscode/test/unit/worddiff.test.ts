import assert from "node:assert/strict";
import { test } from "node:test";
import { pieces, wordChanges } from "../../src/worddiff.ts";

function diff(a: string, b: string): { deleted: string[]; inserted: string[] } {
  const { deleted, inserted } = wordChanges(a, b);
  return { deleted: deleted.map(([i, j]) => a.slice(i, j)), inserted: inserted.map(([i, j]) => b.slice(i, j)) };
}

test("only the changed word", () => {
  assert.deepEqual(diff("the code solves all three.", "the code addresses all three."), {
    deleted: ["solves"],
    inserted: ["addresses"],
  });
});

test("punctuation is a token of its own", () => {
  assert.deepEqual(diff("all three.", "all three!"), { deleted: ["."], inserted: ["!"] });
});

test("changes only spaces apart are one, and spaces at their ends aren't marked", () => {
  assert.deepEqual(diff("a b c d", "a x y d"), { deleted: ["b c"], inserted: ["x y"] });
  assert.deepEqual(diff("prove the old bound", "prove that the new bound"), {
    deleted: ["old"],
    inserted: ["that", "new"],
  });
});

test("nothing is marked when everything changed", () => {
  assert.deepEqual(diff("one two", "three four"), { deleted: [], inserted: [] });
  assert.deepEqual(diff("one two", ""), { deleted: [], inserted: [] });
});

test("pieces split text at the changed spans", () => {
  assert.deepEqual(pieces("a bc d", [[2, 4]]), [
    { text: "a ", changed: false },
    { text: "bc", changed: true },
    { text: " d", changed: false },
  ]);
});
