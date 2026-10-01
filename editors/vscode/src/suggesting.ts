// Suggestion mode: edits to the document become suggestions (§4.6).
//
// While it is on, the document's text from before the edits is kept as the
// base. When the user pauses typing, or saves, the document is compared with
// the base. Each changed stretch, widened to whole words, becomes a new
// suggestion, or extends one made in this session that it touches (a
// `retarget` event), and the document goes back to the base. The file
// therefore never contains suggested text.

import * as vscode from "vscode";
import type { Annox } from "./client.ts";
import { type Change, changes, lineStarts, offsetAt, positionAt } from "./diff.ts";
import { type Store, key } from "./store.ts";
import type { AnnotationView, Range } from "./types.ts";

/** How to undo a suggestion: delete it, or put back its earlier range and
 * replacement. */
interface UndoEntry {
  id: string;
  range?: Range;
  replacement?: string;
}

type Op = { change: Change } | { undo: UndoEntry };

interface Mode {
  uri: vscode.Uri;
  base: string;
  /** Suggestions made in this session, which new changes can extend. */
  views: Map<string, AnnotationView>;
  queue: Op[];
  busy: boolean;
  undo: UndoEntry[];
  timer?: ReturnType<typeof setTimeout>;
  /** Edits of our own in flight, which are not suggestions. */
  restoring: number;
  /** A save is putting the base back; ignore the change that does it. */
  savingBase: boolean;
}

export class Suggesting implements vscode.Disposable {
  private readonly modes = new Map<string, Mode>();
  private serverEdits = 0;
  private readonly changed = new vscode.EventEmitter<string>();
  /** Fires with the key of a document whose mode changed. */
  readonly onDidChange = this.changed.event;
  private readonly status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  private readonly disposables: vscode.Disposable[] = [];

  constructor(
    private readonly annox: Annox,
    private readonly store: Store,
  ) {
    this.status.text = "$(edit) Suggesting";
    this.status.tooltip = "annox suggestion mode: edits become suggestions. Click to turn off.";
    this.status.command = "annox.toggleSuggesting";
    this.status.backgroundColor = new vscode.ThemeColor("statusBarItem.warningBackground");
    this.disposables.push(
      this.status,
      this.changed,
      vscode.workspace.onDidChangeTextDocument((e) => this.onDidChangeText(e)),
      vscode.workspace.onWillSaveTextDocument((e) => this.onWillSave(e)),
      vscode.workspace.onDidCloseTextDocument((doc) => this.disable(doc.uri, false)),
      vscode.window.onDidChangeActiveTextEditor((editor) => {
        for (const mode of this.modes.values()) void this.capture(mode);
        this.updateContext(editor);
      }),
      store.onDidChange((uri) => this.onAnnotations(uri)),
    );
  }

  isOn(uri: string | vscode.Uri): boolean {
    return this.modes.has(key(uri));
  }

  async toggle(editor: vscode.TextEditor, enable?: boolean): Promise<void> {
    const uri = editor.document.uri;
    const on = enable ?? !this.isOn(uri);
    if (!on) return this.disable(uri, true);
    if (this.isOn(uri)) return;
    if (!this.store.has(uri)) {
      void vscode.window.showWarningMessage("annox: this file is not in an annox workspace; run “annox: Initialize Workspace”");
      return;
    }
    this.modes.set(key(uri), {
      uri,
      base: editor.document.getText(),
      views: new Map(),
      queue: [],
      busy: false,
      undo: [],
      restoring: 0,
      savingBase: false,
    });
    this.changed.fire(key(uri));
    this.updateContext(editor);
    void vscode.window.setStatusBarMessage("annox: suggestion mode on", 3000);
  }

  private async disable(uri: vscode.Uri, announce: boolean): Promise<void> {
    const mode = this.modes.get(key(uri));
    if (!mode) return;
    await this.capture(mode);
    clearTimeout(mode.timer);
    this.modes.delete(key(uri));
    this.changed.fire(key(uri));
    this.updateContext(vscode.window.activeTextEditor);
    if (announce) void vscode.window.setStatusBarMessage("annox: suggestion mode off", 3000);
  }

  /** Turns pending edits in `uri` into suggestions now, e.g. before
   * accepting a suggestion. */
  async flush(uri: vscode.Uri): Promise<void> {
    const mode = this.modes.get(key(uri));
    if (mode) await this.capture(mode);
  }

  private updateContext(editor: vscode.TextEditor | undefined): void {
    const on = !!editor && this.isOn(editor.document.uri);
    void vscode.commands.executeCommand("setContext", "annox.suggesting", on);
    if (on) this.status.show();
    else this.status.hide();
  }

  /** Called around edits from the server (accepting a suggestion), which are
   * not suggestions: afterwards the document is the new base. */
  beforeServerEdit(): void {
    this.serverEdits++;
  }

  afterServerEdit(): void {
    this.serverEdits--;
    for (const mode of this.modes.values()) this.rebase(mode);
  }

  private rebase(mode: Mode): void {
    const doc = vscode.workspace.textDocuments.find((d) => key(d.uri) === key(mode.uri));
    if (doc) mode.base = doc.getText();
    mode.queue = [];
    clearTimeout(mode.timer);
  }

  private onDidChangeText(e: vscode.TextDocumentChangeEvent): void {
    const mode = this.modes.get(key(e.document.uri));
    if (!mode || e.contentChanges.length === 0 || mode.restoring > 0) return;
    if (this.serverEdits > 0) return;
    if (mode.savingBase) {
      mode.savingBase = false;
      if (e.document.getText() === mode.base) return;
    }
    clearTimeout(mode.timer);
    const delay = vscode.workspace.getConfiguration("annox").get<number>("suggestionDelay", 750);
    mode.timer = setTimeout(() => void this.capture(mode), delay);
  }

  /** Saving puts the base back as part of the save, so the file never holds
   * suggested text. */
  private onWillSave(e: vscode.TextDocumentWillSaveEvent): void {
    const mode = this.modes.get(key(e.document.uri));
    if (!mode) return;
    clearTimeout(mode.timer);
    const found = changes(mode.base, e.document.getText());
    if (found.length === 0) return;
    mode.savingBase = true;
    const edits = found.map(
      (c) =>
        new vscode.TextEdit(
          new vscode.Range(e.document.positionAt(c.b0), e.document.positionAt(c.b1)),
          mode.base.slice(c.a0, c.a1),
        ),
    );
    e.waitUntil(Promise.resolve(edits));
    this.enqueue(mode, found);
  }

  /** Turns the document's edits into suggestions and puts its text back. */
  private async capture(mode: Mode): Promise<void> {
    clearTimeout(mode.timer);
    const doc = vscode.workspace.textDocuments.find((d) => key(d.uri) === key(mode.uri));
    if (!doc) return;
    const text = doc.getText();
    if (text === mode.base) return;
    const found = changes(mode.base, text);
    if (found.length === 0) return;
    const edit = new vscode.WorkspaceEdit();
    for (const c of found) {
      edit.replace(doc.uri, new vscode.Range(doc.positionAt(c.b0), doc.positionAt(c.b1)), mode.base.slice(c.a0, c.a1));
    }
    mode.restoring++;
    try {
      await vscode.workspace.applyEdit(edit);
    } finally {
      mode.restoring--;
    }
    // Put the cursor where the last edit ended, so typing goes on there.
    const last = found[found.length - 1];
    const editor = vscode.window.visibleTextEditors.find((e) => key(e.document.uri) === key(mode.uri));
    if (editor && doc.getText() === mode.base) {
      const p = positionAt(lineStarts(mode.base), last.editedEnd);
      const pos = new vscode.Position(p.line, p.character);
      editor.selection = new vscode.Selection(pos, pos);
    }
    this.enqueue(mode, found);
  }

  private enqueue(mode: Mode, found: Change[]): void {
    for (const c of found) {
      if (c.a0 !== c.a1 || c.replacement !== "") mode.queue.push({ change: c });
    }
    void this.pump(mode);
  }

  /** Undoes the last suggestion made in this session: deletes it, or puts
   * back what it was before it was extended. Edits not yet turned into
   * suggestions are undone as usual. */
  async undo(editor: vscode.TextEditor): Promise<void> {
    const mode = this.modes.get(key(editor.document.uri));
    if (!mode || editor.document.getText() !== mode.base) {
      await vscode.commands.executeCommand("undo");
      return;
    }
    const entry = mode.undo.pop();
    if (!entry) {
      void vscode.window.setStatusBarMessage("annox: no suggestion to undo", 3000);
      return;
    }
    mode.queue.push({ undo: entry });
    await this.pump(mode);
  }

  /** A suggestion made in this session that the change `[a0, a1)` touches. */
  private extendable(mode: Mode, a0: number, a1: number, starts: number[]) {
    for (const [id, v] of mode.views) {
      const r = v.resolution?.range;
      if (!r || v.status !== "open" || !v.applicable) continue;
      const r0 = offsetAt(mode.base, starts, r.start);
      const r1 = offsetAt(mode.base, starts, r.end);
      if (a1 >= r0 && a0 <= r1) return { id, v, r0, r1 };
    }
    return undefined;
  }

  /** Sends queued operations one at a time, so that each change can extend
   * the suggestion the previous one created. */
  private async pump(mode: Mode): Promise<void> {
    while (!mode.busy && mode.queue.length > 0) {
      const op = mode.queue.shift() as Op;
      const starts = lineStarts(mode.base);
      let method: string;
      let params: unknown;
      let undo: UndoEntry | undefined;
      if ("undo" in op) {
        const u = op.undo;
        if (u.range) {
          method = "annox/retarget";
          params = { annotation: u.id, range: u.range, replacement: u.replacement };
        } else {
          method = "annox/delete";
          params = { annotation: u.id };
        }
      } else {
        let { a0, a1, replacement } = op.change;
        const hit = this.extendable(mode, a0, a1, starts);
        if (hit) {
          // Compose with the existing suggestion. Ties go after it, so typing
          // at the end of an insertion continues it. A change covering all of
          // the suggestion's text (struck through on screen) replaces it.
          const { v, r0, r1 } = hit;
          const current = v.edit?.replacement ?? "";
          const covers = r0 < r1 && a0 <= r0 && a1 >= r1;
          if (covers) {
            // Keep the new text as it is.
          } else if (a0 >= r1 || (a1 > r0 && a0 >= r0)) {
            replacement = current + replacement;
          } else {
            replacement = replacement + current;
          }
          a0 = Math.min(a0, r0);
          a1 = Math.max(a1, r1);
          if (a0 === r0 && a1 === r1 && replacement === current) continue;
          undo = { id: hit.id, range: v.resolution?.range, replacement: current };
        }
        const range = { start: positionAt(starts, a0), end: positionAt(starts, a1) };
        if (hit) {
          method = "annox/retarget";
          params = { annotation: hit.id, range, replacement };
        } else {
          method = "annox/create";
          params = { textDocument: { uri: this.annox.uri(mode.uri) }, kind: "suggestion", range, replacement };
        }
      }
      mode.busy = true;
      const result = await this.annox.request<AnnotationView>(method, params);
      mode.busy = false;
      if (!result && method !== "annox/delete") continue;
      if ("undo" in op) {
        if (op.undo.range && result) mode.views.set(op.undo.id, result);
        else mode.views.delete(op.undo.id);
      } else if (result) {
        mode.views.set(result.id, result);
        mode.undo.push(undo ?? { id: result.id });
      }
    }
  }

  /** Keeps only the suggestions that can still be extended, as last pushed. */
  private onAnnotations(uri: string): void {
    const mode = this.modes.get(uri);
    if (!mode) return;
    const fresh = new Map<string, AnnotationView>();
    for (const a of this.store.annotations(uri)) {
      if (mode.views.has(a.id)) fresh.set(a.id, a);
    }
    mode.views = fresh;
  }

  /** Records a change made outside the session (editing a replacement), so
   * that undo puts it back. */
  pushUndo(uri: vscode.Uri, entry: UndoEntry): void {
    this.modes.get(key(uri))?.undo.push(entry);
  }

  dispose(): void {
    for (const mode of this.modes.values()) clearTimeout(mode.timer);
    for (const d of this.disposables) d.dispose();
  }
}
