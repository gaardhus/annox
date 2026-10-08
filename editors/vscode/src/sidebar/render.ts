// The sidebar as HTML, from its state. Pure: no VS Code API and no DOM, so
// the webview draws with it and the unit tests check it.
//
// Buttons say what they do in `data-*` attributes; webview.ts turns clicks
// on them into messages. The webview's CSP forbids inline handlers.

import type { Action, Card, Group, State } from "./protocol.ts";

/** What only the webview knows: folded groups, and reply boxes being written. */
export interface Ui {
  collapsed: Partial<Record<Group, boolean>>;
  replying: Map<string, string>;
}

const GROUPS: [Group, string][] = [
  ["attention", "Needs attention"],
  ["drafts", "Drafts"],
  ["open", "Open"],
  ["closed", "Closed"],
];

const LABELS: Record<Action, string> = {
  open: "Go to",
  reply: "Reply",
  resolve: "Resolve",
  accept: "Accept",
  reject: "Reject",
  edit: "Edit",
  publish: "Publish",
  reopen: "Reopen",
  revert: "Revert",
  conflicts: "Resolve conflicts",
  history: "History",
};

export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

/** How long ago `iso` was, briefly: `now`, `5m`, `3h`, `2d`, or the date. */
export function ago(iso: string | undefined, now = Date.now()): string {
  const t = iso ? Date.parse(iso) : Number.NaN;
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, (now - t) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 30 * 86400) return `${Math.floor(s / 86400)}d`;
  return new Date(t).toISOString().slice(0, 10);
}

export function render(state: State, ui: Ui): string {
  if (!state.file) return hint("Open a file to see its annotations.");
  if (!state.inWorkspace) {
    return (
      hint(`${escapeHtml(state.file)} is not in an annox workspace.`) +
      `<div class="center"><button class="primary" data-command="init">Initialize Workspace</button></div>`
    );
  }
  const open = state.cards.filter((c) => c.group !== "closed").length;
  const bulk = [
    state.acceptable > 1 ? `<button data-command="acceptAll">Accept all ${state.acceptable}</button>` : "",
    state.drafts > 1 ? `<button data-command="publishAll">Publish all ${state.drafts}</button>` : "",
  ].join("");
  const head = `
    <header class="head">
      <span class="file" title="${escapeHtml(state.file)}">${escapeHtml(state.file)}</span>
      <span class="count">${open} open</span>
      ${bulk ? `<div class="bulk">${bulk}</div>` : ""}
    </header>`;
  if (state.cards.length === 0) {
    return head + hint("No annotations yet. Select some text and run <b>annox: Comment</b> or <b>annox: Suggest Replacement</b>.");
  }
  const selected = new Set(state.selected);
  const groups = GROUPS.map(([g, label]) => {
    const cards = state.cards.filter((c) => c.group === g);
    if (cards.length === 0) return "";
    // Closed annotations are folded unless unfolded, the others the other way round.
    const collapsed = ui.collapsed[g] ?? g === "closed";
    return `
      <section class="group ${g}">
        <button class="ghead" data-toggle="${g}" aria-expanded="${!collapsed}">
          <span class="chev">${collapsed ? "▸" : "▾"}</span>${label}<span class="n">${cards.length}</span>
        </button>
        ${collapsed ? "" : cards.map((c) => card(c, selected.has(c.id), ui.replying.has(c.id))).join("")}
      </section>`;
  });
  return head + groups.join("");
}

function edited(note: string | undefined): string {
  return note ? `<span class="edited">${escapeHtml(note)}</span>` : "";
}

function card(c: Card, selected: boolean, replying: boolean): string {
  const id = escapeHtml(c.id);
  const badges = [c.draft ? "draft" : "", c.group === "closed" ? c.status : ""].filter(Boolean);
  const meta = `
    <div class="meta">
      <span class="kind">${c.kind}</span>
      <span class="author">${escapeHtml(c.author)}</span>
      ${c.when ? `<span class="when">${escapeHtml(c.when)}</span>` : ""}
      ${edited(c.edited)}
      ${badges.map((b) => `<span class="badge">${escapeHtml(b)}</span>`).join("")}
      <span class="where">${c.line ? `L${c.line}` : "text gone"}</span>
    </div>`;
  const quote = c.quote ? `<blockquote class="quote">${escapeHtml(c.quote)}</blockquote>` : "";
  const diff = c.diff ? `<div class="diff">${c.diff.map(segment).join("") || "<i>(no change)</i>"}</div>` : "";
  const body = c.body ? `<p class="body">${escapeHtml(c.body)}</p>` : "";
  const notes = c.notes.map((n) => `<p class="note">${escapeHtml(n)}</p>`).join("");
  const replies = c.replies.length
    ? `<ol class="replies">${c.replies
        .map(
          (r) => `
          <li>
            <div class="meta"><span class="author">${escapeHtml(r.author)}</span><span class="when">${escapeHtml(r.when)}</span>${edited(r.edited)}</div>
            <p class="body">${escapeHtml(r.body)}</p>
          </li>`,
        )
        .join("")}</ol>`
    : "";
  const box = replying
    ? `<div class="reply-box">
        <textarea data-reply="${id}" rows="3" placeholder="Reply… (Ctrl+Enter to send, Escape to cancel)"></textarea>
        <div class="actions">
          <button class="primary" data-send="${id}">Send</button>
          <button data-cancel="${id}">Cancel</button>
        </div>
      </div>`
    : "";
  const buttons = (["open", ...c.actions] as Action[])
    .filter((a) => !(replying && a === "reply"))
    .map((a) => {
      const primary = a === "accept" || a === "publish" ? "primary" : "";
      return `<button class="${primary}" data-action="${a}" data-id="${id}">${LABELS[a]}</button>`;
    })
    .join("");
  const classes = ["card", c.kind, c.group, selected ? "selected" : ""].filter(Boolean).join(" ");
  return `
    <article class="${classes}" data-card="${id}">
      ${meta}${quote}${diff}${body}${notes}${replies}${box}
      <div class="actions">${buttons}</div>
    </article>`;
}

function segment(s: { op: string; text: string }): string {
  const text = escapeHtml(s.text);
  return s.op === "-" ? `<del>${text}</del>` : s.op === "+" ? `<ins>${text}</ins>` : `<span>${text}</span>`;
}

function hint(html: string): string {
  return `<p class="hint">${html}</p>`;
}
