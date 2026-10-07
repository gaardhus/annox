/// <reference lib="dom" />
/// <reference lib="dom.iterable" />
// The sidebar's script, bundled for the browser into dist/sidebar.js. It owns
// no annotations: it draws the state the extension sends and sends back what
// was clicked. Folded groups are kept across reloads with `setState`.

import type { Command, Group, HostMessage, State, ViewMessage } from "./protocol.ts";
import { type Ui, render } from "./render.ts";

declare function acquireVsCodeApi(): {
  postMessage(message: ViewMessage): void;
  getState(): { collapsed?: Ui["collapsed"] } | undefined;
  setState(state: { collapsed: Ui["collapsed"] }): void;
};

const vscode = acquireVsCodeApi();
const app = document.getElementById("app") as HTMLElement;
const ui: Ui = { collapsed: vscode.getState()?.collapsed ?? {}, replying: new Map() };
let state: State | undefined;

function boxes(): HTMLTextAreaElement[] {
  return [...app.querySelectorAll<HTMLTextAreaElement>("textarea[data-reply]")];
}

/** Draws `state`, keeping what is being written in reply boxes. */
function draw(focus?: string): void {
  if (!state) return;
  // Only boxes still open: a sent or cancelled one is no longer in `replying`.
  for (const t of boxes()) {
    const id = t.dataset.reply as string;
    if (ui.replying.has(id)) ui.replying.set(id, t.value);
  }
  focus ??= (document.activeElement as HTMLElement | null)?.dataset?.reply;
  // A card that is gone takes its reply box with it.
  const ids = new Set(state.cards.map((c) => c.id));
  for (const id of ui.replying.keys()) if (!ids.has(id)) ui.replying.delete(id);
  app.innerHTML = render(state, ui);
  for (const t of boxes()) {
    const id = t.dataset.reply as string;
    t.value = ui.replying.get(id) ?? "";
    if (id === focus) t.focus();
  }
}

function select(ids: string[]): void {
  if (state) state.selected = ids;
  let first: Element | undefined;
  for (const card of app.querySelectorAll<HTMLElement>("[data-card]")) {
    const on = ids.includes(card.dataset.card as string);
    card.classList.toggle("selected", on);
    if (on) first ??= card;
  }
  first?.scrollIntoView({ block: "nearest" });
}

function send(id: string): void {
  const body = app.querySelector<HTMLTextAreaElement>(`textarea[data-reply="${CSS.escape(id)}"]`)?.value ?? "";
  if (body.trim()) vscode.postMessage({ type: "reply", id, body });
  ui.replying.delete(id);
  draw();
}

function cancel(id: string): void {
  ui.replying.delete(id);
  draw();
}

app.addEventListener("click", (e) => {
  const el = (e.target as HTMLElement).closest<HTMLElement>("button");
  if (!el) return;
  const d = el.dataset;
  if (d.toggle) {
    const g = d.toggle as Group;
    ui.collapsed[g] = el.getAttribute("aria-expanded") === "true";
    vscode.setState({ collapsed: ui.collapsed });
    draw();
  } else if (d.action === "reply" && d.id) {
    ui.replying.set(d.id, "");
    draw(d.id);
  } else if (d.action && d.id) {
    vscode.postMessage({ type: "action", id: d.id, action: d.action as never });
  } else if (d.send) {
    send(d.send);
  } else if (d.cancel) {
    cancel(d.cancel);
  } else if (d.command) {
    vscode.postMessage({ type: "command", command: d.command as Command });
  }
});

app.addEventListener("keydown", (e) => {
  const id = (e.target as HTMLElement).dataset?.reply;
  if (!id) return;
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    send(id);
  } else if (e.key === "Escape") {
    e.preventDefault();
    cancel(id);
  }
});

window.addEventListener("message", (e: MessageEvent<HostMessage>) => {
  const m = e.data;
  if (m.type === "state") {
    state = m.state;
    draw();
  } else if (m.type === "select") {
    select(m.ids);
  }
});

vscode.postMessage({ type: "ready" });
