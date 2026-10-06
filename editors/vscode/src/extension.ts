// annox for VS Code: a thin client for the annox language server (spec §6).
//
// The server owns all annox logic. This extension shows the annotations it
// pushes (`annox/didChangeAnnotations`) as comment threads and decorations,
// and sends user actions as `annox/*` requests. Suggested edits come back as
// `workspace/applyEdit`, which the language client applies to the document,
// so they can be undone and saved as usual.

import * as vscode from "vscode";
import { Actions } from "./actions.ts";
import { Annox } from "./client.ts";
import { Threads } from "./comments.ts";
import { Decorations } from "./decorations.ts";
import { HistoryDocs, Scratch } from "./scratch.ts";
import { Store, fromRange } from "./store.ts";
import { Suggesting } from "./suggesting.ts";
import { AnnotationsView } from "./view.ts";

/** What the extension exposes to tests. */
export interface Api {
  annox: Annox;
  store: Store;
  threads: Threads;
  suggesting: Suggesting;
}

export async function activate(context: vscode.ExtensionContext): Promise<Api> {
  const store = new Store();
  const hooks = { beforeServerEdit: () => {}, afterServerEdit: () => {} };
  const annox = new Annox(store, hooks);
  const suggesting = new Suggesting(annox, store);
  hooks.beforeServerEdit = () => suggesting.beforeServerEdit();
  hooks.afterServerEdit = () => suggesting.afterServerEdit();
  const threads = new Threads(store);
  const decorations = new Decorations(store, (uri) => suggesting.isOn(uri));
  const scratch = new Scratch();
  const history = new HistoryDocs();
  const actions = new Actions(annox, store, threads, suggesting, scratch, history);
  context.subscriptions.push(
    store,
    annox,
    suggesting,
    threads,
    decorations,
    scratch,
    history,
    new AnnotationsView(store),
    ...actions.register(),
    suggesting.onDidChange((uri) => decorations.renderUri(uri)),
    vscode.commands.registerCommand("annox.toggleOverlay", () => {
      const shown = decorations.setShown();
      vscode.window.setStatusBarMessage(`annox: overlay ${shown ? "on" : "off"}`, 2000);
    }),
    ...presence(annox, store),
    ...workspaceContext(store),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (!e.affectsConfiguration("annox.path") && !e.affectsConfiguration("annox.author")) return;
      const choice = await vscode.window.showInformationMessage(
        "annox: restart the annox server to use the new settings?",
        "Restart",
      );
      if (choice) await annox.restart();
    }),
  );
  await annox.start();
  return { annox, store, threads, suggesting };
}

/** Shares the cursor with collaborators on a sync hub (§7.8), at most every
 * 150 ms, unless `annox.presence` is off. */
function presence(annox: Annox, store: Store): vscode.Disposable[] {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const send = () => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      if (!vscode.workspace.getConfiguration("annox").get<boolean>("presence", true)) return;
      const editor = vscode.window.activeTextEditor;
      if (!editor || !store.has(editor.document.uri)) return;
      const pos = editor.selection.active;
      annox.notify("annox/setPresence", {
        textDocument: { uri: annox.uri(editor.document.uri) },
        selection: fromRange(new vscode.Range(pos, pos)),
      });
    }, 150);
  };
  return [
    vscode.window.onDidChangeTextEditorSelection(send),
    vscode.window.onDidChangeActiveTextEditor(send),
    new vscode.Disposable(() => clearTimeout(timer)),
  ];
}

/** Sets `annox.inWorkspace` while the active file is in an annox workspace,
 * for menus and the command palette. */
function workspaceContext(store: Store): vscode.Disposable[] {
  const update = () => {
    const doc = vscode.window.activeTextEditor?.document;
    void vscode.commands.executeCommand("setContext", "annox.inWorkspace", !!doc && store.has(doc.uri));
  };
  update();
  return [vscode.window.onDidChangeActiveTextEditor(update), store.onDidChange(update)];
}

export function deactivate(): void {}
