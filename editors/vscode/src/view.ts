// The annox view: every annotation of the open files, including closed ones
// whose text is gone, which have no comment thread (see comments.ts).

import * as path from "node:path";
import * as vscode from "vscode";
import { changeMarkdown } from "./comments.ts";
import { type Store, authorName, describe, isConflicted, isOrphaned, rangeOf, summary } from "./store.ts";
import type { AnnotationView } from "./types.ts";

type Node = { uri: string } | { uri: string; a: AnnotationView };

function icon(a: AnnotationView): vscode.ThemeIcon {
  if (isConflicted(a)) return new vscode.ThemeIcon("git-merge", new vscode.ThemeColor("editorError.foreground"));
  if (a.status === "open" && isOrphaned(a)) {
    return new vscode.ThemeIcon("warning", new vscode.ThemeColor("editorWarning.foreground"));
  }
  if (a.status === "accepted") return new vscode.ThemeIcon("check");
  if (a.status === "rejected" || a.status === "withdrawn") return new vscode.ThemeIcon("close");
  if (a.status === "resolved") return new vscode.ThemeIcon("pass");
  if (a.local) return new vscode.ThemeIcon("comment-draft");
  return new vscode.ThemeIcon(a.kind === "suggestion" ? "diff" : "comment");
}

/** Open annotations first, in document order, then closed ones, newest first. */
function order(a: AnnotationView, b: AnnotationView): number {
  const open = (x: AnnotationView) => (x.status === "open" ? 0 : 1);
  if (open(a) !== open(b)) return open(a) - open(b);
  if (open(a) === 1) return (b.created ?? "").localeCompare(a.created ?? "");
  const ra = rangeOf(a);
  const rb = rangeOf(b);
  if (ra && rb) return ra.start.compareTo(rb.start);
  return ra ? 1 : rb ? -1 : 0;
}

/** The thread as Markdown, for annotations without a place in the text. */
export function threadMarkdown(a: AnnotationView): string {
  const person = (x: AnnotationView) => `**${authorName(x)}** · ${x.created ?? ""}`;
  const lines = [`# ${describe(a)}`, ""];
  lines.push(`${person(a)} · *${a.status}*`, "");
  if (a.kind === "suggestion") lines.push(changeMarkdown(undefined, a.edit?.replacement ?? ""), "");
  if (a.body) lines.push(a.body, "");
  for (const r of a.replies ?? []) lines.push("---", "", person(r), "", r.body ?? "", "");
  return lines.join("\n");
}

export class AnnotationsView implements vscode.TreeDataProvider<Node>, vscode.Disposable {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  private readonly disposables: vscode.Disposable[] = [];

  constructor(private readonly store: Store) {
    const view = vscode.window.createTreeView("annox.annotations", { treeDataProvider: this, showCollapseAll: true });
    this.disposables.push(view, this.changed, store.onDidChange(() => this.changed.fire()));
  }

  getChildren(node?: Node): Node[] {
    if (!node) {
      return this.store
        .keys()
        .filter((uri) => this.store.annotations(uri).length > 0)
        .sort()
        .map((uri) => ({ uri }));
    }
    if ("a" in node) return [];
    return this.store
      .annotations(node.uri)
      .filter((a) => !a.deleted)
      .sort(order)
      .map((a) => ({ uri: node.uri, a }));
  }

  getTreeItem(node: Node): vscode.TreeItem {
    if (!("a" in node)) {
      const uri = vscode.Uri.parse(node.uri);
      const item = new vscode.TreeItem(uri, vscode.TreeItemCollapsibleState.Expanded);
      const n = this.store.annotations(node.uri).filter((a) => a.status === "open").length;
      item.description = `${vscode.workspace.asRelativePath(path.dirname(uri.fsPath))} · ${n} open`;
      return item;
    }
    const { a } = node;
    const item = new vscode.TreeItem(summary(a), vscode.TreeItemCollapsibleState.None);
    const r = rangeOf(a);
    const where = r ? `line ${r.start.line + 1}` : "text gone";
    item.description = [authorName(a), a.status === "open" ? "" : a.status, where].filter(Boolean).join(" · ");
    item.iconPath = icon(a);
    item.tooltip = new vscode.MarkdownString(threadMarkdown(a));
    item.contextValue = `annox ${a.status === "open" ? "open" : "closed"}`;
    item.command = { command: "annox.open", title: "Open", arguments: [{ annotation: a.id }] };
    return item;
  }

  dispose(): void {
    for (const d of this.disposables) d.dispose();
  }
}
