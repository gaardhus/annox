// The annox sidebar: the annotations of the active file as cards, in a
// webview in its own activity bar container. The cards' buttons run the
// same commands as comment threads, with `{ annotation: id }`.

import * as crypto from "node:crypto";
import * as path from "node:path";
import { diffWordsWithSpace } from "diff";
import * as vscode from "vscode";
import { type Store, authorName, editedNote, isConflicted, isOrphaned, key, oneLine, rangeOf, toRange } from "../store.ts";
import type { AnnotationView } from "../types.ts";
import type { Action, Card, Command, Group, HostMessage, State, ViewMessage } from "./protocol.ts";
import { ago } from "./render.ts";

const COMMANDS: Record<Action, string> = {
  open: "annox.open",
  reply: "annox.reply",
  resolve: "annox.resolveThread",
  accept: "annox.acceptSuggestion",
  reject: "annox.rejectSuggestion",
  edit: "annox.editAnnotation",
  publish: "annox.publish",
  reopen: "annox.reopenThread",
  revert: "annox.revertSuggestion",
  conflicts: "annox.resolveConflicts",
  history: "annox.history",
};

const FILE_COMMANDS: Record<Command, string> = {
  init: "annox.init",
  acceptAll: "annox.acceptAll",
  publishAll: "annox.publishAll",
};

function group(a: AnnotationView): Group {
  if (a.status !== "open") return "closed";
  if (isConflicted(a) || isOrphaned(a) || (a.kind === "suggestion" && !a.applicable)) return "attention";
  return a.local ? "drafts" : "open";
}

function actions(a: AnnotationView, store: Store): Action[] {
  if (a.status !== "open") {
    if (a.status !== "accepted") return ["reopen", "history"];
    return store.isReverted(a.id) ? ["history"] : ["revert", "history"];
  }
  const out: Action[] = [];
  if (isConflicted(a)) out.push("conflicts");
  if (a.local) out.push("publish");
  if (a.kind === "suggestion") {
    if (a.applicable) out.push("accept", "edit");
    out.push("reject");
  } else {
    out.push("resolve");
  }
  out.push("reply");
  return out;
}

function notes(a: AnnotationView, store: Store): string[] {
  const out = store.revertLinks(a).map((l) => l.replace(/\*/g, ""));
  if (a.status !== "open") return out;
  const conflicted = Object.keys(a.conflicts ?? {});
  if (conflicted.length) out.push(`⚠ Conflicting changes: ${conflicted.join(", ")}.`);
  if (isOrphaned(a)) out.push("⚠ The text this was on could not be found.");
  else if (a.kind === "suggestion" && !a.applicable) out.push("⚠ The text changed since this was suggested.");
  return out;
}

/** Open annotations in document order, orphans at their suggested place or
 * last, then closed ones, newest first. */
function order(a: AnnotationView, b: AnnotationView): number {
  const closed = (x: AnnotationView) => (x.status === "open" ? 0 : 1);
  if (closed(a) !== closed(b)) return closed(a) - closed(b);
  if (closed(a)) return (b.created ?? "").localeCompare(a.created ?? "");
  const at = (x: AnnotationView) => {
    const s = x.resolution?.suggested?.range;
    return rangeOf(x)?.start ?? (s ? toRange(s).start : undefined);
  };
  const pa = at(a);
  const pb = at(b);
  return pa && pb ? pa.compareTo(pb) : pa ? -1 : pb ? 1 : 0;
}

export class Sidebar implements vscode.WebviewViewProvider, vscode.Disposable {
  static readonly viewType = "annox.sidebar";
  private view?: vscode.WebviewView;
  /** Whether the webview's script has run, for tests. */
  loaded = false;
  /** The file shown: the active editor, unless that is not a file, such as a
   * thread's history, which leaves the last one shown. */
  private editor = Sidebar.file(vscode.window.activeTextEditor);
  private readonly disposables: vscode.Disposable[] = [];

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly store: Store,
  ) {
    this.disposables.push(
      vscode.window.registerWebviewViewProvider(Sidebar.viewType, this),
      vscode.window.onDidChangeActiveTextEditor((e) => {
        if (e && !Sidebar.file(e)) return;
        if (!e && this.editor && vscode.window.visibleTextEditors.includes(this.editor)) return;
        this.editor = e;
        this.post();
      }),
      vscode.window.onDidChangeTextEditorSelection((e) => {
        if (e.textEditor === this.editor) this.send({ type: "select", ids: this.selected() });
      }),
      store.onDidChange((k) => {
        if (this.editor && k === key(this.editor.document.uri)) this.post();
      }),
    );
  }

  private static file(e: vscode.TextEditor | undefined): vscode.TextEditor | undefined {
    return e?.document.uri.scheme === "file" ? e : undefined;
  }

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    const dist = vscode.Uri.joinPath(this.extensionUri, "dist");
    const media = vscode.Uri.joinPath(this.extensionUri, "media");
    view.webview.options = { enableScripts: true, localResourceRoots: [dist, media] };
    view.webview.html = this.html(view.webview, dist, media);
    view.webview.onDidReceiveMessage((m: ViewMessage) => this.receive(m));
    view.onDidChangeVisibility(() => this.post());
    view.onDidDispose(() => {
      this.view = undefined;
      this.loaded = false;
    });
  }

  /** What the sidebar shows. */
  state(): State {
    const doc = this.editor?.document;
    if (!doc) return { inWorkspace: false, cards: [], selected: [], acceptable: 0, drafts: 0 };
    const annotations = this.store
      .annotations(doc.uri)
      .filter((a) => !a.deleted)
      .sort(order);
    const cards = annotations.map((a) => this.card(a, doc));
    return {
      file: path.basename(doc.uri.fsPath),
      inWorkspace: this.store.has(doc.uri),
      cards,
      selected: this.selected(),
      acceptable: annotations.filter((a) => a.kind === "suggestion" && a.status === "open" && a.applicable).length,
      drafts: annotations.filter((a) => a.status === "open" && a.local).length,
    };
  }

  private card(a: AnnotationView, doc: vscode.TextDocument): Card {
    const r = rangeOf(a);
    // A closed annotation's text may have changed; `quote` is what it was on.
    const text = a.status === "open" && r ? doc.getText(doc.validateRange(r)) : a.quote;
    const card: Card = {
      id: a.id,
      group: group(a),
      kind: a.kind === "suggestion" ? "suggestion" : a.body ? "comment" : "highlight",
      status: a.status,
      draft: !!a.local,
      author: authorName(a),
      when: ago(a.created),
      edited: editedNote(a),
      line: r ? r.start.line + 1 : undefined,
      body: a.body ?? a.label ?? undefined,
      notes: notes(a, this.store),
      replies: (a.replies ?? [])
        .filter((x) => !x.deleted)
        .map((x) => ({ author: authorName(x), when: ago(x.created), edited: editedNote(x), body: x.body ?? "" })),
      actions: actions(a, this.store),
    };
    const replacement = a.edit?.replacement ?? "";
    if (a.kind !== "suggestion") card.quote = text ? oneLine(text, 200) : undefined;
    else if (text === undefined) card.diff = [{ op: "+", text: replacement }];
    else {
      card.diff = diffWordsWithSpace(text, replacement).map((p) => ({
        op: p.added ? "+" : p.removed ? "-" : "=",
        text: p.value,
      }));
    }
    return card;
  }

  /** The annotations whose text has the cursor. */
  private selected(): string[] {
    const editor = this.editor;
    if (!editor) return [];
    const pos = editor.selection.active;
    return this.store
      .annotations(editor.document.uri)
      .filter((a) => {
        const r = rangeOf(a);
        return r !== undefined && editor.document.validateRange(r).contains(pos);
      })
      .map((a) => a.id);
  }

  private async receive(m: ViewMessage): Promise<void> {
    if (m.type === "ready") {
      this.loaded = true;
      this.post();
    }
    else if (m.type === "action") await vscode.commands.executeCommand(COMMANDS[m.action], { annotation: m.id });
    else if (m.type === "reply") await vscode.commands.executeCommand("annox.reply", { annotation: m.id, body: m.body });
    else if (m.type === "command") {
      // Commands of the whole file act on the active editor, which the
      // sidebar's focus leaves alone, but show it first in case it isn't.
      if (m.command !== "init" && this.editor && vscode.window.activeTextEditor !== this.editor) {
        await vscode.window.showTextDocument(this.editor.document, this.editor.viewColumn);
      }
      await vscode.commands.executeCommand(FILE_COMMANDS[m.command]);
    }
  }

  private post(): void {
    if (this.view?.visible) this.send({ type: "state", state: this.state() });
  }

  private send(m: HostMessage): void {
    void this.view?.webview.postMessage(m);
  }

  private html(webview: vscode.Webview, dist: vscode.Uri, media: vscode.Uri): string {
    const nonce = crypto.randomBytes(16).toString("base64");
    const script = webview.asWebviewUri(vscode.Uri.joinPath(dist, "sidebar.js"));
    const style = webview.asWebviewUri(vscode.Uri.joinPath(media, "sidebar.css"));
    return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource}; script-src 'nonce-${nonce}';">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <link rel="stylesheet" href="${style}">
</head>
<body>
  <div id="app"></div>
  <script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
  }

  dispose(): void {
    for (const d of this.disposables) d.dispose();
  }
}
