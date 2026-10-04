// <annox-demo>: an editable document with annox comments and suggestions,
// running the real annox-core in WebAssembly. No framework, no build step:
//
//   <script type="module" src="annox-demo/annox-demo.js"></script>
//   <annox-demo path="essay.md">Some text to annotate.</annox-demo>
//
// Properties: `text`, `annotations` (seed notes, see `seed`), `events` (read
// only). Attributes: `path`, `author` (your display name). Fires
// `annoxchange` with `{ text, events }` after every change. Theme it with the
// `--annox-*` custom properties below.

import * as annox from "./annox.js";

const YOU = { id: "urn:annox-demo:you", name: "You" };

/** A comment with nothing to read yet: no body and no replies (§2.2). */
const isHighlight = (n) => n.kind === "comment" && !n.body && !n.replies.length;

const esc = (s) =>
  String(s ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );

/** Strips the common indentation of inline text content. */
function dedent(s) {
  const lines = s
    .replace(/^\s*\n/, "")
    .replace(/\s+$/, "")
    .split("\n");
  if (!lines.some((l) => l.trim())) return "";
  const indent = Math.min(
    ...lines.filter((l) => l.trim()).map((l) => l.match(/^\s*/)[0].length),
  );
  return lines.map((l) => l.slice(indent)).join("\n") + "\n";
}

const STYLE = `
:host {
  --annox-font: system-ui, sans-serif;
  --annox-doc-font: Georgia, serif;
  --annox-mono: ui-monospace, monospace;
  --annox-paper: #ffffff;
  --annox-surface: #f3f4f7;
  --annox-ink: #1f2937;
  --annox-muted: #5b6474;
  --annox-line: #d9dde5;
  --annox-comment: #facc15;
  --annox-comment-line: #ca8a04;
  --annox-accent: #6366f1;
  --annox-on-accent: #ffffff;
  --annox-del: #c2410c;
  --annox-ins: #15803d;
  --annox-radius: 10px;
  display: block;
  color: var(--annox-ink);
  font: 15px/1.5 var(--annox-font);
  container-type: inline-size;
}
@media (prefers-color-scheme: dark) {
  :host {
    --annox-paper: #1b1f27;
    --annox-surface: #232834;
    --annox-ink: #e8eaf0;
    --annox-muted: #9aa3b5;
    --annox-line: #353c4b;
    --annox-accent: #8b8ff7;
    --annox-on-accent: #12151c;
    --annox-del: #fb923c;
    --annox-ins: #4ade80;
    --annox-comment-line: #facc15;
  }
}
* { box-sizing: border-box; }
button, input, textarea { font: inherit; color: inherit; }
button {
  cursor: pointer; border: 1px solid var(--annox-line); background: var(--annox-paper);
  border-radius: 6px; padding: .25rem .7rem; font-size: .875rem;
}
button:hover:not(:disabled) { border-color: var(--annox-muted); }
button:disabled { cursor: default; opacity: .45; }
button.primary { background: var(--annox-accent); border-color: var(--annox-accent); color: var(--annox-on-accent); }
/* Primary buttons shift toward the text color: lighter in dark mode, deeper in light mode. */
button.primary:hover:not(:disabled) {
  background: color-mix(in srgb, var(--annox-accent) 78%, var(--annox-ink));
  border-color: color-mix(in srgb, var(--annox-accent) 78%, var(--annox-ink));
}
:focus-visible { outline: 2px solid var(--annox-accent); outline-offset: 2px; }

.frame {
  display: grid; grid-template-columns: minmax(0, 1fr) minmax(16rem, 22rem);
  border: 1px solid var(--annox-line); border-radius: var(--annox-radius);
  background: var(--annox-surface); overflow: hidden;
  /* A fixed height side by side, so switching tabs doesn't resize the demo. */
  height: var(--annox-height, 40rem);
}
@container (max-width: 44rem) {
  .frame { grid-template-columns: 1fr; height: auto; }
  .editor { overflow: visible; }
  .margin { border-left: 0; border-top: 1px solid var(--annox-line); height: var(--annox-height, 40rem); }
}

.doc { background: var(--annox-paper); display: flex; flex-direction: column; min-width: 0; min-height: 0; }
.bar { display: flex; gap: .5rem; align-items: center; padding: .6rem .9rem; border-bottom: 1px solid var(--annox-line); }
.bar .hint { color: var(--annox-muted); font-size: .8125rem; margin-left: auto; }
.bar button { white-space: nowrap; }
@container (max-width: 30rem) { .bar .hint { display: none; } }

.editor { display: grid; flex: 1; min-height: 0; overflow: auto; }
.editor > * {
  grid-area: 1 / 1; margin: 0; border: 0; padding: 1.4rem 1.6rem 1.8rem;
  font: 1.0625rem/1.7 var(--annox-doc-font); letter-spacing: normal; tab-size: 4;
  white-space: pre-wrap; overflow-wrap: break-word;
}
.backdrop { color: transparent; pointer-events: none; }
/* The document shows focus as a bar down its left edge, not an outline that would overlap the bars above and below. */
.editor:focus-within { box-shadow: inset 3px 0 var(--annox-accent); }
.editor textarea:focus-visible { outline: none; }
textarea { resize: none; overflow: hidden; background: transparent; outline: none; min-height: 12rem; caret-color: var(--annox-accent); }
mark { color: transparent; border-radius: 2px; background: color-mix(in srgb, var(--annox-comment) 45%, transparent); box-shadow: inset 0 -2px var(--annox-comment-line); }
mark.highlight { box-shadow: none; }
mark.suggestion { background: color-mix(in srgb, var(--annox-accent) 22%, transparent); box-shadow: inset 0 -2px var(--annox-accent); }
mark.active { background: color-mix(in srgb, var(--annox-comment) 85%, transparent); }
mark.suggestion.active { background: color-mix(in srgb, var(--annox-accent) 40%, transparent); }
.pin { display: inline-block; width: 0; height: 1.2em; vertical-align: text-bottom; box-shadow: 0 0 0 1.5px var(--annox-accent); }

.meta { padding: .5rem .9rem; border-top: 1px solid var(--annox-line); color: var(--annox-muted); font: .75rem var(--annox-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.margin { border-left: 1px solid var(--annox-line); display: flex; flex-direction: column; min-width: 0; min-height: 0; }
.tabs { display: flex; border-bottom: 1px solid var(--annox-line); }
.tabs button { flex: 1; border: 0; border-radius: 0; background: none; padding: .7rem; color: var(--annox-muted); box-shadow: inset 0 -2px transparent; }
.tabs button[aria-selected="true"] { color: var(--annox-ink); box-shadow: inset 0 -2px var(--annox-accent); }
.panel { overflow: auto; padding: .75rem; display: flex; flex-direction: column; gap: .6rem; }
.empty { color: var(--annox-muted); font-size: .875rem; padding: .5rem .25rem; }
.section { color: var(--annox-muted); font-size: .8125rem; margin: .5rem .25rem 0; }

.note { background: var(--annox-paper); border: 1px solid var(--annox-line); border-radius: 8px; padding: .7rem .8rem; cursor: pointer; }
.note.active { border-color: var(--annox-accent); box-shadow: 0 0 0 1px var(--annox-accent); cursor: default; }
.note.closed { opacity: .7; }
.note header { display: flex; gap: .4rem; align-items: baseline; flex-wrap: wrap; font-size: .8125rem; color: var(--annox-muted); }
.note header strong { color: var(--annox-ink); font-weight: 600; }
.state { margin-left: auto; font-size: .75rem; padding: 0 .4rem; border-radius: 99px; border: 1px solid var(--annox-line); white-space: nowrap; }
.state.relocated { border-color: var(--annox-accent); color: var(--annox-accent); }
.state.orphaned { border-color: var(--annox-del); color: var(--annox-del); }
.quote { margin: .45rem 0; padding-left: .6rem; border-left: 3px solid var(--annox-comment); font-family: var(--annox-doc-font); font-size: .9375rem; overflow-wrap: anywhere; }
.suggestion .quote { border-left-color: var(--annox-accent); }
.orphan .quote { color: var(--annox-muted); }
.orphan .quote > :not(ins) { text-decoration: line-through; }
del { color: var(--annox-del); }
ins { color: var(--annox-ins); text-decoration: none; }
.point { color: var(--annox-muted); font-style: italic; }
.body { margin: .3rem 0 0; white-space: pre-wrap; overflow-wrap: anywhere; }
.reply { margin-top: .5rem; padding-top: .5rem; border-top: 1px solid var(--annox-line); font-size: .9rem; }
.reply strong { font-weight: 600; }
.actions { display: flex; gap: .4rem; margin-top: .65rem; flex-wrap: wrap; align-items: center; }
.actions .status { color: var(--annox-muted); font-size: .8125rem; margin-right: auto; }
form { display: flex; flex-direction: column; gap: .45rem; margin-top: .6rem; }
form input, form textarea {
  border: 1px solid var(--annox-line); border-radius: 6px; padding: .35rem .5rem;
  background: var(--annox-paper); width: 100%; min-height: 0; overflow: auto; caret-color: auto;
}
form textarea { resize: vertical; }
form label { font-size: .8125rem; color: var(--annox-muted); display: flex; flex-direction: column; gap: .2rem; }
form .row { display: flex; gap: .4rem; justify-content: flex-end; }

.event { background: var(--annox-paper); border: 1px solid var(--annox-line); border-radius: 8px; font-size: .8125rem; }
.event summary { cursor: pointer; padding: .45rem .6rem; display: flex; gap: .5rem; align-items: baseline; }
.event summary b { font-weight: 600; }
.event summary code { color: var(--annox-muted); font: .72rem var(--annox-mono); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.event pre { margin: 0; padding: .6rem; border-top: 1px solid var(--annox-line); font: .72rem/1.45 var(--annox-mono); overflow: auto; }
.error { padding: 1rem; color: var(--annox-del); }
`;

class AnnoxDemo extends HTMLElement {
  #text = null;
  #initialText = "";
  #seed = [];
  #events = [];
  #docId = null;
  #notes = []; // derived roots, with replies and resolutions
  #active = null;
  #draft = null;
  #tab = "notes";
  #ready = false;

  constructor() {
    super();
    this.attachShadow({ mode: "open" });
  }

  get path() {
    return this.getAttribute("path") || "document.md";
  }

  get you() {
    return { ...YOU, name: this.getAttribute("author") || YOU.name };
  }

  /** The document text. Setting it is an edit: notes stay anchored if they can. */
  get text() {
    return this.#text ?? "";
  }
  set text(value) {
    this.#text = String(value);
    if (!this.#ready) this.#initialText = this.#text;
    else {
      this.#editor.value = this.#text;
      this.#changed();
    }
  }

  /** Every event written so far, as it would be stored in `.annox/`. */
  get events() {
    return structuredClone(this.#events);
  }

  /**
   * Seed notes, created when the demo loads or resets. Each is
   * `{ kind: "comment" | "suggestion", quote, body, replacement, author, replies, status }`,
   * where `quote` is the text to anchor to and `replies` is `[{ author, body }]`.
   */
  get annotations() {
    return this.#seed;
  }
  set annotations(value) {
    this.#seed = value ?? [];
    if (this.#ready) this.reset();
  }

  async connectedCallback() {
    if (this.shadowRoot.childElementCount) return;
    if (this.#text === null) this.#initialText = dedent(this.textContent);
    this.shadowRoot.innerHTML = `<style>${STYLE}</style><div class="empty">Loading annox…</div>`;
    try {
      await annox.load();
    } catch (e) {
      this.shadowRoot.innerHTML = `<style>${STYLE}</style><div class="error">Couldn't load annox-core from
        <code>annox-demo/pkg/</code>. Build it with <code>just site</code>. (${esc(e.message)})</div>`;
      return;
    }
    this.#mount();
    this.#ready = true;
    this.reset();
  }

  /** Restores the initial text and seed notes. */
  reset() {
    this.#text = this.#initialText;
    this.#editor.value = this.#text;
    this.#events = [];
    this.#active = this.#draft = null;
    this.#docId = annox.newId();
    this.#events.push({
      id: this.#docId,
      document: this.#docId,
      after: [],
      type: "document",
      author: this.you,
      time: annox.now(),
      path: this.path,
    });
    for (const s of this.#seed) {
      const start = this.#text.indexOf(s.quote);
      if (start < 0) continue;
      const author = s.author ?? this.you;
      const target = annox.createAnchor(
        this.text,
        start,
        start + s.quote.length,
        this.path,
      );
      const id = this.#create(s.kind, target, s.body, s.replacement, author);
      for (const r of s.replies ?? [])
        this.#reply(id, r.body, r.author ?? this.you);
      if (s.status) this.#setStatus(id, s.status, author);
    }
    this.#changed();
  }

  // Event writers (§2.4). Each returns after appending; #changed() re-renders.

  #append(annotation, type, fields, author = this.you) {
    const note = annox.derive(this.#events).find((a) => a.id === annotation);
    const id = type === "create" ? annotation : annox.newId();
    this.#events.push({
      id,
      annotation,
      after: note?.heads ?? [],
      type,
      author,
      time: annox.now(),
      ...fields,
    });
    return id;
  }

  #create(kind, target, body, replacement, author, reverts) {
    const fields = { kind, target };
    if (body) fields.body = body;
    if (kind === "suggestion") fields.edit = { replacement: replacement ?? "" };
    if (reverts) fields.reverts = reverts;
    return this.#append(annox.newId(), "create", fields, author);
  }

  #reply(parent, body, author) {
    return this.#append(
      annox.newId(),
      "create",
      { kind: "reply", parent, body },
      author,
    );
  }

  #setStatus(id, status, author) {
    const fields = { status };
    if (status === "accepted") {
      const note =
        this.#notes.find((n) => n.id === id) ??
        annox.derive(this.#events).find((a) => a.id === id);
      const r = annox.applySuggestion(
        this.text,
        note.target,
        note.edit.replacement,
      );
      if (!r.applied) return;
      this.#text = r.text;
      this.#editor.value = r.text;
      fields.appliedVersion = r.version;
    }
    this.#append(id, "status", fields, author);
  }

  /** Suggests putting back the text an accepted suggestion replaced (§4.3.4). */
  #revert(id) {
    const n = this.#notes.find((x) => x.id === id);
    const found = annox.appliedTextSearch(this.text, n.target, n.edit.replacement);
    if (!found) return this.#active;
    const target = annox.createAnchor(this.text, found.start, found.end, this.path);
    const original = n.target.selectors.quote.exact;
    return this.#create("suggestion", target, "", original, this.you, n.id);
  }

  /** How far reverting an accepted suggestion has got, if at all. */
  #revertState(n) {
    const reverts = this.#notes.filter((x) => x.reverts === n.id);
    if (reverts.some((x) => x.status === "accepted")) return ", then reverted";
    if (reverts.some((x) => x.status === "open")) return ", revert suggested";
    return "";
  }

  // Rendering.

  get #editor() {
    return this.shadowRoot.querySelector(".editor textarea");
  }

  #mount() {
    this.shadowRoot.innerHTML = `<style>${STYLE}</style>
      <div class="frame">
        <div class="doc">
          <div class="bar">
            <button data-action="highlight" disabled>Highlight</button>
            <button data-action="draft" data-kind="comment" disabled>Comment</button>
            <button data-action="draft" data-kind="suggestion" disabled>Suggest edit</button>
            <span class="hint">Select text to annotate it</span>
          </div>
          <div class="editor">
            <div class="backdrop" aria-hidden="true"></div>
            <textarea spellcheck="false" aria-label="Document text"></textarea>
          </div>
          <div class="meta"></div>
        </div>
        <aside class="margin">
          <div class="tabs" role="tablist">
            <button role="tab" data-tab="notes"></button>
            <button role="tab" data-tab="events"></button>
          </div>
          <div class="panel" role="tabpanel"></div>
        </aside>
      </div>`;

    const editor = this.#editor;
    editor.addEventListener("input", () => {
      this.#text = editor.value;
      this.#changed();
    });
    const onSelect = () => this.#selectionChanged();
    for (const type of ["select", "click", "keyup"])
      editor.addEventListener(type, onSelect);

    this.shadowRoot.addEventListener("click", (e) => this.#onClick(e));
    this.shadowRoot.addEventListener("submit", (e) => this.#onSubmit(e));
    this.shadowRoot.addEventListener("input", (e) => {
      if (this.#draft && e.target.name in this.#draft)
        this.#draft[e.target.name] = e.target.value;
    });
    this.shadowRoot.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey) && e.target.form)
        e.target.form.requestSubmit();
      if (e.key === "Escape" && this.#draft) {
        this.#draft = null;
        this.#renderPanel();
      }
    });
  }

  /** Re-derives notes and re-resolves anchors after any edit or new event. */
  #changed() {
    const derived = annox.derive(this.#events);
    const replies = derived.filter((a) => a.kind === "reply" && !a.deleted);
    this.#notes = derived
      .filter((a) => a.kind !== "reply" && !a.deleted)
      .map((a) => ({
        ...a,
        resolution: annox.resolve(this.text, a.target),
        replies: replies
          .filter((r) => r.parent === a.id)
          .sort((x, y) => x.created.localeCompare(y.created)),
      }))
      .sort(
        (a, b) =>
          (a.resolution.start ?? Infinity) - (b.resolution.start ?? Infinity),
      );
    this.#renderDoc();
    this.#renderPanel();
    this.dispatchEvent(
      new CustomEvent("annoxchange", {
        detail: { text: this.text, events: this.events },
      }),
    );
  }

  #selectionChanged() {
    const { selectionStart: s, selectionEnd: e } = this.#editor;
    for (const b of this.shadowRoot.querySelectorAll(
      '[data-action="draft"], [data-action="highlight"]',
    ))
      b.disabled = s === e;
    if (s !== e) return;
    const hit = this.#notes
      .filter(
        (n) =>
          n.status === "open" &&
          n.resolution.start <= s &&
          s <= n.resolution.end,
      )
      .sort(
        (a, b) =>
          a.resolution.end -
          a.resolution.start -
          (b.resolution.end - b.resolution.start),
      )[0];
    if (hit && hit.id !== this.#active) this.#activate(hit.id);
  }

  #activate(id) {
    this.#active = id;
    this.#renderDoc();
    this.#renderPanel();
    this.shadowRoot
      .querySelector(`.note[data-id="${id}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }

  #renderDoc() {
    const text = this.text;
    const spans = this.#notes
      .filter((n) => n.status === "open" && n.resolution.start !== undefined)
      .map((n) => ({
        id: n.id,
        kind: isHighlight(n) ? "highlight" : n.kind,
        start: n.resolution.start,
        end: n.resolution.end,
      }));
    const cuts = [
      ...new Set([0, text.length, ...spans.flatMap((s) => [s.start, s.end])]),
    ].sort((a, b) => a - b);
    let html = "";
    for (let i = 0; i < cuts.length; i++) {
      const [a, b] = [cuts[i], cuts[i + 1]];
      if (spans.some((s) => s.start === a && s.end === a))
        html += '<span class="pin"></span>';
      if (b === undefined) break;
      const cover = spans.filter(
        (s) => s.start <= a && b <= s.end && s.start < s.end,
      );
      const chunk = esc(text.slice(a, b));
      if (!cover.length) {
        html += chunk;
        continue;
      }
      // A note to read wins over a bare highlight.
      const kind = ["suggestion", "comment", "highlight"].find((k) =>
        cover.some((s) => s.kind === k),
      );
      const active = cover.some((s) => s.id === this.#active) ? " active" : "";
      html += `<mark class="${kind}${active}">${chunk}</mark>`;
    }
    // A trailing newline needs something after it to take up its line.
    this.shadowRoot.querySelector(".backdrop").innerHTML =
      html + (text.endsWith("\n") ? " " : "");
    this.shadowRoot.querySelector(".meta").textContent =
      `${this.path}  ${annox.version(text)}`;
  }

  #renderPanel() {
    const open = this.#notes.filter((n) => n.status === "open");
    const [notesTab, eventsTab] =
      this.shadowRoot.querySelectorAll(".tabs button");
    notesTab.textContent = `Notes (${open.length})`;
    eventsTab.textContent = `Event log (${this.#events.length})`;
    notesTab.setAttribute("aria-selected", this.#tab === "notes");
    eventsTab.setAttribute("aria-selected", this.#tab === "events");

    const panel = this.shadowRoot.querySelector(".panel");
    if (this.#tab === "events") {
      panel.innerHTML =
        `<p class="empty">Each change is one immutable JSON file in <code>.annox/</code>, committed next to the document.</p>` +
        this.#events
          .map((e, i) => {
            const folder = `docs/${this.path}~${this.#docId}/${e.document ? "document/" : ""}`;
            return `<details class="event" ${i === this.#events.length - 1 ? "open" : ""}>
              <summary><b>${esc(e.type)}</b><code>${esc(folder + e.id)}.json</code></summary>
              <pre>${esc(JSON.stringify(e, null, 2))}</pre></details>`;
          })
          .reverse()
          .join("");
      return;
    }

    const closed = this.#notes.filter((n) => n.status !== "open");
    let html = this.#draft ? this.#draftHtml() : "";
    if (!this.#notes.length && !this.#draft)
      html += `<p class="empty">No notes yet. Select some text to add one.</p>`;
    html += open.map((n) => this.#noteHtml(n)).join("");
    if (closed.length)
      html +=
        `<p class="section">Closed</p>` +
        closed.map((n) => this.#noteHtml(n)).join("");
    panel.innerHTML = html;
  }

  #draftHtml() {
    const d = this.#draft;
    const suggestion = d.kind === "suggestion";
    return `<article class="note active ${d.kind}">
      <header><strong>${esc(this.you.name)}</strong> ${suggestion ? "suggesting an edit" : "commenting"}</header>
      <div class="quote">${esc(d.quote)}</div>
      <form data-form="draft">
        ${suggestion ? `<label>Replace with<textarea name="replacement" rows="2">${esc(d.replacement)}</textarea></label>` : ""}
        <label>${suggestion ? "Why (optional)" : "Comment"}<textarea name="body" rows="2" ${suggestion ? "" : "required"}>${esc(d.body)}</textarea></label>
        <div class="row"><button type="button" data-action="cancel">Cancel</button>
        <button class="primary">${suggestion ? "Add suggestion" : "Add comment"}</button></div>
      </form></article>`;
  }

  #noteHtml(n) {
    const r = n.resolution;
    const suggestion = n.kind === "suggestion";
    const active = n.id === this.#active;
    const state =
      r.state === "exact"
        ? `<span class="state" title="Anchor found where it was written (§3.7, step ${r.step})">Exact</span>`
        : r.state === "relocated"
          ? `<span class="state relocated" title="The text moved; the anchor found it again (§3.7, step ${r.step})">Relocated, step ${r.step}</span>`
          : `<span class="state orphaned" title="The quoted text is gone (§3.7)">Orphaned</span>`;
    const quote = n.target.selectors.quote.exact;
    const shown = quote
      ? esc(quote)
      : `<span class="point">(insertion point)</span>`;
    const target = suggestion
      ? `<del>${shown}</del> <ins>${esc(n.edit.replacement) || '<span class="point">(delete)</span>'}</ins>`
      : `<span>${shown}</span>`;
    const applicable = r.state !== "orphaned" && r.step !== 4;

    let actions = "";
    if (n.status !== "open") {
      actions = `<span class="status">${esc(n.status[0].toUpperCase() + n.status.slice(1))}${this.#revertState(n)}</span>`;
      if (n.status === "resolved")
        actions += `<button data-action="status" data-status="open">Reopen</button>`;
      if (n.status === "accepted" && !this.#revertState(n)) {
        // Reverting needs the applied text, unchanged (§4.3.3).
        const found = annox.appliedTextSearch(this.text, n.target, n.edit.replacement);
        actions += `<button data-action="revert" ${found ? "" : 'disabled title="The text changed since it was accepted, so revert it by hand"'}>Revert</button>`;
      }
    } else if (suggestion) {
      // An orphaned suggestion has nowhere to apply. Step 4 found it, but
      // only with whitespace ignored, so it isn't applicable either (§4.2).
      if (r.state !== "orphaned") {
        actions = `<button class="primary" data-action="status" data-status="accepted" ${applicable ? "" : 'disabled title="The whitespace around this text changed, so the edit can\'t be applied"'}>Accept</button>`;
      }
      actions += `<button data-action="status" data-status="rejected">Reject</button>`;
    } else {
      actions = `<button data-action="status" data-status="resolved">Resolve</button>`;
    }

    return `<article class="note ${n.kind} ${active ? "active" : ""} ${n.status !== "open" ? "closed" : ""} ${r.state === "orphaned" ? "orphan" : ""}" data-id="${esc(n.id)}">
      <header><strong>${esc(n.author?.name ?? n.author?.id)}</strong> ${suggestion ? (n.reverts ? "suggested a revert" : "suggested") : n.body ? "commented" : "highlighted"} ${n.status === "open" ? state : ""}</header>
      <div class="quote">${target}</div>
      ${n.body ? `<p class="body">${esc(n.body)}</p>` : ""}
      ${n.replies.map((x) => `<div class="reply"><strong>${esc(x.author?.name ?? x.author?.id)}</strong> ${esc(x.body)}</div>`).join("")}
      <div class="actions">${actions}</div>
      ${active && n.status === "open" ? `<form data-form="reply"><input name="reply" placeholder="Reply" aria-label="Reply" required></form>` : ""}
    </article>`;
  }

  #onClick(e) {
    const tab = e.target.closest("[data-tab]");
    if (tab) {
      this.#tab = tab.dataset.tab;
      return this.#renderPanel();
    }
    const action = e.target.closest("[data-action]");
    const note = e.target.closest(".note[data-id]");
    if (action?.dataset.action === "highlight") {
      const { selectionStart: start, selectionEnd: end } = this.#editor;
      const target = annox.createAnchor(this.text, start, end, this.path);
      // A highlight is a comment with no body (§2.2).
      this.#active = this.#create("comment", target, "", null, this.you);
      this.#draft = null;
      this.#tab = "notes";
      this.#changed();
    } else if (action?.dataset.action === "draft") {
      const { selectionStart: start, selectionEnd: end } = this.#editor;
      const quote = this.text.slice(start, end);
      // Anchor now: the text may change before the note is submitted.
      const target = annox.createAnchor(this.text, start, end, this.path);
      this.#draft = {
        kind: action.dataset.kind,
        target,
        quote,
        body: "",
        replacement: quote,
      };
      this.#active = null;
      this.#tab = "notes";
      this.#renderPanel();
      this.shadowRoot
        .querySelector('form[data-form="draft"] textarea')
        ?.focus();
    } else if (action?.dataset.action === "revert" && note) {
      this.#active = this.#revert(note.dataset.id);
      this.#changed();
    } else if (action?.dataset.action === "cancel") {
      this.#draft = null;
      this.#renderPanel();
    } else if (action?.dataset.action === "status" && note) {
      this.#setStatus(note.dataset.id, action.dataset.status, this.you);
      this.#changed();
    } else if (
      note &&
      note.dataset.id !== this.#active &&
      !e.target.closest("form")
    ) {
      this.#activate(note.dataset.id);
    }
  }

  #onSubmit(e) {
    e.preventDefault();
    const form = e.target;
    if (form.dataset.form === "draft") {
      const d = this.#draft;
      this.#active = this.#create(
        d.kind,
        d.target,
        d.body.trim(),
        d.replacement,
        this.you,
      );
      this.#draft = null;
    } else if (form.dataset.form === "reply") {
      const body = form.reply.value.trim();
      if (!body) return;
      this.#reply(form.closest(".note").dataset.id, body, this.you);
    }
    this.#changed();
  }
}

customElements.define("annox-demo", AnnoxDemo);
