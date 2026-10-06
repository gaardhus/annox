// Annotated text in the editor: tinted ranges, suggestions drawn inline,
// labels at the end of lines, others' cursors, and a notice about
// annotations that could not be located.

import * as vscode from "vscode";
import { type Store, firstLine, isConflicted, isOrphaned, key, rangeOf, toRange } from "./store.ts";
import type { AnnotationView } from "./types.ts";

const color = (id: string) => new vscode.ThemeColor(id);

function underline(themeColor: string): vscode.DecorationRenderOptions {
  return {
    textDecoration: `underline wavy var(--vscode-${themeColor.replace(".", "-")})`,
    overviewRulerColor: color(themeColor),
    overviewRulerLane: vscode.OverviewRulerLane.Right,
  };
}

export class Decorations implements vscode.Disposable {
  private readonly types = {
    // A comment's text is tinted and underlined: the underline says there's a
    // note to read. A highlight, a comment with nothing to read, is only tinted.
    comment: vscode.window.createTextEditorDecorationType({
      backgroundColor: color("annox.commentBackground"),
      textDecoration: "underline solid var(--vscode-annox-commentUnderline)",
      overviewRulerColor: color("annox.commentBackground"),
      overviewRulerLane: vscode.OverviewRulerLane.Center,
    }),
    highlight: vscode.window.createTextEditorDecorationType({
      backgroundColor: color("annox.commentBackground"),
      overviewRulerColor: color("annox.commentBackground"),
      overviewRulerLane: vscode.OverviewRulerLane.Center,
    }),
    suggestion: vscode.window.createTextEditorDecorationType({
      backgroundColor: color("annox.suggestionBackground"),
      overviewRulerColor: color("annox.suggestionBackground"),
      overviewRulerLane: vscode.OverviewRulerLane.Center,
    }),
    draft: vscode.window.createTextEditorDecorationType({
      backgroundColor: color("annox.draftBackground"),
      overviewRulerColor: color("annox.draftBackground"),
      overviewRulerLane: vscode.OverviewRulerLane.Center,
    }),
    stale: vscode.window.createTextEditorDecorationType(underline("editorWarning.foreground")),
    conflict: vscode.window.createTextEditorDecorationType(underline("editorError.foreground")),
    point: vscode.window.createTextEditorDecorationType({
      before: { contentText: "◆", color: color("annox.pointForeground"), margin: "0 1px" },
    }),
    deletion: vscode.window.createTextEditorDecorationType({
      textDecoration: "line-through",
      color: color("annox.deletionForeground"),
      backgroundColor: color("diffEditor.removedTextBackground"),
    }),
    insertion: vscode.window.createTextEditorDecorationType({
      after: { color: color("annox.insertionForeground"), backgroundColor: color("diffEditor.insertedTextBackground") },
      rangeBehavior: vscode.DecorationRangeBehavior.ClosedClosed,
    }),
    label: vscode.window.createTextEditorDecorationType({
      after: { color: color("annox.labelForeground"), margin: "0 0 0 2em", fontStyle: "italic" },
    }),
    presenceRange: vscode.window.createTextEditorDecorationType({
      backgroundColor: color("annox.presenceBackground"),
    }),
    presence: vscode.window.createTextEditorDecorationType({
      before: {
        color: color("annox.presenceForeground"),
        backgroundColor: color("annox.presenceBackground"),
        margin: "0 2px 0 0",
      },
    }),
  };
  private readonly disposables: vscode.Disposable[] = [];
  private readonly codeLensChanged = new vscode.EventEmitter<void>();
  /** Whether annotations and others' cursors are drawn. Documents in
   * suggestion mode draw theirs regardless. */
  private shown = true;

  constructor(
    private readonly store: Store,
    /** Whether suggestions in the document are drawn inline. */
    private readonly inline: (uri: string) => boolean,
  ) {
    this.disposables.push(
      store.onDidChange((uri) => {
        this.renderUri(uri);
        this.codeLensChanged.fire();
      }),
      store.onDidChangePeers(() => this.renderAll()),
      vscode.window.onDidChangeVisibleTextEditors(() => this.renderAll()),
      vscode.workspace.onDidChangeConfiguration((e) => {
        if (e.affectsConfiguration("annox")) this.renderAll();
      }),
      vscode.languages.registerCodeLensProvider(
        { scheme: "file" },
        {
          onDidChangeCodeLenses: this.codeLensChanged.event,
          provideCodeLenses: (doc) => this.codeLenses(doc),
        },
      ),
      this.codeLensChanged,
    );
  }

  /** Shows or hides the decorations in every editor, toggling by default. */
  setShown(shown = !this.shown): boolean {
    this.shown = shown;
    this.renderAll();
    this.codeLensChanged.fire();
    return shown;
  }

  renderAll(): void {
    for (const editor of vscode.window.visibleTextEditors) this.render(editor);
  }

  renderUri(uri: string): void {
    for (const editor of vscode.window.visibleTextEditors) {
      if (key(editor.document.uri) === uri) this.render(editor);
    }
  }

  private codeLenses(doc: vscode.TextDocument): vscode.CodeLens[] {
    if (!this.shown) return [];
    const orphans = this.store.annotations(doc.uri).filter((a) => a.status === "open" && isOrphaned(a)).length;
    if (orphans === 0) return [];
    const title = `$(warning) ${orphans} annotation${orphans === 1 ? "" : "s"} could not be located`;
    return [new vscode.CodeLens(new vscode.Range(0, 0, 0, 0), { title, command: "annox.orphans" })];
  }

  render(editor: vscode.TextEditor): void {
    const doc = editor.document;
    const uri = key(doc.uri);
    const config = vscode.workspace.getConfiguration("annox", doc.uri);
    const labels = config.get<boolean>("labels", true);
    const inline = this.inline(uri) || config.get<boolean>("inlineSuggestions", false);
    const ranges: Record<keyof typeof this.types, vscode.DecorationOptions[]> = {
      comment: [],
      highlight: [],
      suggestion: [],
      draft: [],
      stale: [],
      conflict: [],
      point: [],
      deletion: [],
      insertion: [],
      label: [],
      presenceRange: [],
      presence: [],
    };
    const labelled = new Map<number, string[]>();
    const shown = this.shown || this.inline(uri);
    for (const a of shown ? this.store.annotations(doc.uri) : []) {
      const r = rangeOf(a);
      if (!r || a.status !== "open" || a.deleted) continue;
      const range = doc.validateRange(r);
      let label = a.kind === "suggestion" ? `→ ${a.edit?.replacement ?? ""}` : (firstLine(a.body) ?? a.label);
      if (a.kind === "suggestion" && a.applicable && inline) {
        // Deleted text struck through, followed by the inserted text.
        if (!range.isEmpty) ranges.deletion.push({ range });
        const replacement = a.edit?.replacement ?? "";
        if (replacement !== "") {
          ranges.insertion.push({
            range: new vscode.Range(range.end, range.end),
            renderOptions: { after: { contentText: replacement.replace(/\r?\n/g, "↵") } },
          });
        }
        label = firstLine(a.body);
        if (isConflicted(a)) ranges.conflict.push({ range });
      } else if (range.isEmpty) {
        ranges.point.push({ range });
      } else {
        ranges[group(a)].push({ range });
      }
      if (labels && label) {
        const replies = a.replies?.length ?? 0;
        let text = replies > 0 ? `${label} (+${replies})` : label;
        if (a.local) text = `[draft] ${text}`;
        const line = range.start.line;
        labelled.set(line, [...(labelled.get(line) ?? []), text]);
      }
    }
    for (const [line, texts] of labelled) {
      const end = doc.lineAt(line).range.end;
      ranges.label.push({
        range: new vscode.Range(end, end),
        renderOptions: { after: { contentText: truncate(texts.join(" · "), 120) } },
      });
    }
    for (const peer of this.shown ? this.store.peers : []) {
      if (!peer.textDocument || !peer.range || key(peer.textDocument.uri) !== uri) continue;
      const range = doc.validateRange(toRange(peer.range));
      const name = peer.author?.name ?? peer.author?.id ?? "someone";
      if (!range.isEmpty) ranges.presenceRange.push({ range, hoverMessage: name });
      ranges.presence.push({
        range: new vscode.Range(range.start, range.start),
        renderOptions: { before: { contentText: `▏${name}` } },
      });
    }
    for (const [name, type] of Object.entries(this.types)) {
      editor.setDecorations(type, ranges[name as keyof typeof this.types]);
    }
  }

  dispose(): void {
    for (const type of Object.values(this.types)) type.dispose();
    for (const d of this.disposables) d.dispose();
  }
}

function group(a: AnnotationView): "comment" | "highlight" | "suggestion" | "draft" | "stale" | "conflict" {
  if (isConflicted(a)) return "conflict";
  if (a.local) return "draft";
  if (a.kind === "suggestion") return a.applicable ? "suggestion" : "stale";
  return a.body || a.replies?.length ? "comment" : "highlight";
}

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}
