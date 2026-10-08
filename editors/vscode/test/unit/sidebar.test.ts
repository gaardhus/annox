import assert from "node:assert/strict";
import { test } from "node:test";
import type { Card, State } from "../../src/sidebar/protocol.ts";
import { type Ui, ago, render } from "../../src/sidebar/render.ts";

const ui = (): Ui => ({ collapsed: {}, replying: new Map() });

function card(fields: Partial<Card>): Card {
  return {
    id: "a1",
    group: "open",
    kind: "comment",
    status: "open",
    draft: false,
    author: "Ada",
    when: "2d",
    line: 4,
    notes: [],
    replies: [],
    actions: ["resolve", "reply"],
    ...fields,
  };
}

function state(cards: Card[]): State {
  return { file: "paper.tex", inWorkspace: true, cards, selected: [], acceptable: 0, drafts: 0 };
}

test("without a file or outside a workspace, says so", () => {
  assert.match(render({ ...state([]), file: undefined }, ui()), /Open a file/);
  const html = render({ ...state([]), inWorkspace: false }, ui());
  assert.match(html, /not in an annox workspace/);
  assert.match(html, /data-command="init"/);
});

test("cards are listed by group, and closed ones are folded", () => {
  const html = render(
    state([
      card({ id: "o", body: "Which section?" }),
      card({ id: "x", group: "attention", notes: ["⚠ The text this was on could not be found."] }),
      card({ id: "c", group: "closed", status: "resolved" }),
    ]),
    ui(),
  );
  assert.ok(html.indexOf("Needs attention") < html.indexOf("Open"), "attention first");
  assert.match(html, /data-card="o"/);
  assert.match(html, /could not be found/);
  assert.doesNotMatch(html, /data-card="c"/, "closed folded");
  assert.match(html, />2 open</, "closed ones are not counted");
  const unfolded = ui();
  unfolded.collapsed.closed = false;
  assert.match(render(state([card({ id: "c", group: "closed", status: "resolved" })]), unfolded), /data-card="c"/);
});

test("a suggestion shows its word diff, and text is escaped", () => {
  const html = render(
    state([
      card({
        kind: "suggestion",
        diff: [
          { op: "=", text: "we " },
          { op: "-", text: "prove" },
          { op: "+", text: "<show>" },
        ],
        actions: ["accept", "reject"],
      }),
    ]),
    ui(),
  );
  assert.match(html, /<del>prove<\/del><ins>&#60;show&#62;<\/ins>/);
  assert.match(html, /class="primary" data-action="accept"/);
});

test("a card being replied to has a reply box instead of a Reply button", () => {
  const u = ui();
  u.replying.set("a1", "draft text");
  const html = render(state([card({})]), u);
  assert.match(html, /<textarea data-reply="a1"/);
  assert.doesNotMatch(html, /data-action="reply"/);
});

test("ago", () => {
  const now = Date.parse("2026-10-07T12:00:00Z");
  assert.equal(ago(undefined, now), "");
  assert.equal(ago("2026-10-07T11:59:30Z", now), "now");
  assert.equal(ago("2026-10-07T11:00:00Z", now), "1h");
  assert.equal(ago("2026-10-05T12:00:00Z", now), "2d");
  assert.equal(ago("2026-01-01T00:00:00Z", now), "2026-01-01");
});

test("an edited card or reply says so, and by whom", () => {
  const html = render(
    state([
      card({
        body: "Which section, exactly?",
        edited: "edited by Bob",
        replies: [{ author: "Bob", when: "1d", edited: "edited", body: "The third." }],
      }),
    ]),
    ui(),
  );
  assert.match(html, /<span class="edited">edited by Bob<\/span>/);
  assert.match(html, /<span class="edited">edited<\/span><\/div>/);
});
