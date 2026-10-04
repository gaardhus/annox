// Annotations as VS Code comment threads, under their own "annox" comment
// controller so they sit beside other comment providers.

import * as vscode from "vscode";
import {
  type Store,
  authorName,
  isConflicted,
  isOrphaned,
  key,
  rangeOf,
} from "./store.ts";
import type { AnnotationView } from "./types.ts";

/** A comment in a thread: the root annotation or one of its replies. */
export class AnnoxComment implements vscode.Comment {
  body: string | vscode.MarkdownString;
  mode = vscode.CommentMode.Preview;
  author: vscode.CommentAuthorInformation;
  label?: string;
  timestamp?: Date;
  contextValue: string;
  /** The text to edit: the annotation's body. */
  text: string;

  constructor(
    readonly view: AnnotationView,
    readonly root: boolean,
    body: vscode.MarkdownString,
    editable: boolean,
  ) {
    this.body = body;
    this.text = view.body ?? "";
    this.author = { name: authorName(view) };
    if (view.created) this.timestamp = new Date(view.created);
    this.contextValue = ["annox", root ? "root" : "reply", editable ? "editable" : ""].join(" ").trim();
  }
}

/** Inline code, fenced so that backticks in `s` don't end it. */
function code(s: string): string {
  if (s === "") return "*(nothing)*";
  const longest = Math.max(0, ...(s.match(/`+/g) ?? []).map((m) => m.length));
  const fence = "`".repeat(longest + 1);
  const pad = s.startsWith("`") || s.endsWith("`") ? " " : "";
  return `${fence}${pad}${s}${pad}${fence}`;
}

/** The proposed change, given the text it replaces when it is known. */
export function changeMarkdown(original: string | undefined, replacement: string): string {
  const multiline = (s: string) => s.includes("\n") || s.length > 80;
  if (original !== undefined && (multiline(original) || multiline(replacement))) {
    const lines = (s: string, sign: string) => (s === "" ? [] : s.split("\n").map((l) => sign + l));
    const body = [...lines(original, "-"), ...lines(replacement, "+")].join("\n");
    const fence = body.includes("```") ? "````" : "```";
    return `${fence}diff\n${body}\n${fence}`;
  }
  if (original === undefined) return `Replace with ${code(replacement)}`;
  if (original === "") return `Insert ${code(replacement)}`;
  if (replacement === "") return `Delete ${code(original)}`;
  return `Replace ${code(original)} with ${code(replacement)}`;
}

function markdown(text: string): vscode.MarkdownString {
  const md = new vscode.MarkdownString(text);
  md.supportThemeIcons = true;
  return md;
}

/** Flags describing a root annotation, matched by `when` clauses in
 * package.json, e.g. `commentThread =~ /\bsuggestion\b/`. */
function flags(a: AnnotationView, reverted: boolean): string {
  const f = ["annox", a.kind, a.status === "open" ? "open" : "closed"];
  if (a.kind === "suggestion" && a.status === "open") f.push(a.applicable ? "applicable" : "stale");
  if (a.status === "accepted") f.push("accepted");
  if (reverted) f.push("reverted");
  if (a.local) f.push("draft");
  if (isConflicted(a)) f.push("conflicted");
  // A closed annotation is expected to lose its text, e.g. once a suggestion
  // is accepted, so only open ones count as orphaned.
  if (a.status === "open" && isOrphaned(a)) f.push("orphaned");
  return f.join(" ");
}

function threadLabel(a: AnnotationView): string {
  const parts: string[] = [];
  if (isConflicted(a)) parts.push("⚠ Conflicting changes");
  if (a.status === "open" && isOrphaned(a)) parts.push("⚠ Text not found");
  if (a.local) parts.push("Draft");
  if (a.kind === "suggestion") {
    const stale = a.status === "open" && !a.applicable ? "Stale suggestion" : "Suggestion";
    parts.push(a.status === "open" ? stale : `${stale} (${a.status})`);
  } else {
    parts.push(a.status === "open" ? "Comment" : `Comment (${a.status})`);
  }
  return parts.join(" · ");
}

interface Entry {
  thread: vscode.CommentThread;
  uri: string;
  /** What the thread was last built from, to skip unchanged updates. */
  signature: string;
  comments: Map<string, AnnoxComment>;
}

export class Threads implements vscode.Disposable {
  readonly controller = vscode.comments.createCommentController("annox", "annox");
  private readonly entries = new Map<string, Entry>();
  private readonly ids = new WeakMap<vscode.CommentThread, string>();
  /** Annotations whose thread opens when it appears, e.g. one just created. */
  private readonly expand = new Set<string>();
  /** Empty threads opened to write a new comment. */
  private readonly pending = new Set<vscode.CommentThread>();
  private readonly disposables: vscode.Disposable[] = [];

  constructor(private readonly store: Store) {
    this.controller.options = { prompt: "Comment with annox…", placeHolder: "Write a comment" };
    this.controller.commentingRangeProvider = {
      provideCommentingRanges: (doc) =>
        this.store.has(doc.uri) ? [new vscode.Range(0, 0, Math.max(doc.lineCount - 1, 0), 0)] : [],
    };
    this.disposables.push(
      this.controller,
      store.onDidChange((uri) => this.sync(uri)),
    );
  }

  /** The annotation a thread or comment shows. */
  idOf(target: vscode.CommentThread | AnnoxComment): string | undefined {
    if (target instanceof AnnoxComment) return target.view.id;
    return this.ids.get(target);
  }

  threadOf(id: string): vscode.CommentThread | undefined {
    return this.entries.get(id)?.thread;
  }

  /** Opens the thread of `id` when it next appears or now if it exists. */
  reveal(id: string): void {
    const entry = this.entries.get(id);
    if (entry) entry.thread.collapsibleState = vscode.CommentThreadCollapsibleState.Expanded;
    else this.expand.add(id);
  }

  /** Opens an empty thread on `range` to write a new comment in. With
   * `draft`, its only button saves a local draft. */
  openNew(uri: vscode.Uri, range: vscode.Range, draft: boolean): vscode.CommentThread {
    for (const t of this.pending) t.dispose();
    this.pending.clear();
    const thread = this.controller.createCommentThread(uri, range, []);
    thread.canReply = true;
    thread.contextValue = draft ? "annox-new-draft" : "annox-new";
    thread.label = draft ? "New draft (only you can see it until published)" : "New comment";
    thread.collapsibleState = vscode.CommentThreadCollapsibleState.Expanded;
    this.pending.add(thread);
    return thread;
  }

  /** Disposes an empty thread once its comment is written. */
  discard(thread: vscode.CommentThread): void {
    if (this.ids.has(thread)) return;
    this.pending.delete(thread);
    thread.dispose();
  }

  /** Starts editing the body of `comment` in place. */
  startEditing(comment: AnnoxComment): void {
    const entry = this.entries.get(comment.root ? comment.view.id : (this.parentOf(comment.view.id) ?? ""));
    if (!entry) return;
    entry.thread.collapsibleState = vscode.CommentThreadCollapsibleState.Expanded;
    comment.mode = vscode.CommentMode.Editing;
    comment.body = comment.text;
    entry.thread.comments = [...entry.thread.comments];
  }

  /** Leaves edit mode, showing the rendered body again. */
  stopEditing(comment: AnnoxComment): void {
    const entry = this.entries.get(comment.root ? comment.view.id : (this.parentOf(comment.view.id) ?? ""));
    if (!entry) return;
    comment.mode = vscode.CommentMode.Preview;
    // Rebuild from the last view, now that the comment isn't being edited.
    entry.signature = "";
    this.sync(entry.uri);
  }

  private parentOf(replyId: string): string | undefined {
    for (const [id, entry] of this.entries) {
      if (entry.comments.has(replyId) && id !== replyId) return id;
    }
    return undefined;
  }

  /** Where the thread goes. An open annotation whose text could not be
   * found goes on the first line, where it asks to be re-attached. A closed
   * one, such as an accepted suggestion whose text was replaced, gets no
   * thread: VS Code would draw it on the first line too. The annox view lists
   * it instead. */
  private range(a: AnnotationView, doc: vscode.TextDocument | undefined): vscode.Range | undefined {
    const r = rangeOf(a) ?? (a.status === "open" ? new vscode.Range(0, 0, 0, 0) : undefined);
    return r && doc ? doc.validateRange(r) : r;
  }

  private rootBody(a: AnnotationView, doc: vscode.TextDocument | undefined): vscode.MarkdownString {
    const lines: string[] = [];
    if (a.kind === "suggestion") {
      const r = rangeOf(a);
      const original = r && doc && a.status === "open" ? doc.getText(doc.validateRange(r)) : undefined;
      lines.push(changeMarkdown(original, a.edit?.replacement ?? ""));
      lines.push(...this.store.revertLinks(a));
      const by = a.retargetedBy?.author;
      if (by && by.id !== a.author?.id) lines.push(`*Re-targeted by ${by.name ?? by.id}*`);
      if (a.status === "open" && !a.applicable) {
        lines.push("$(warning) *The text changed since this was suggested. Re-target it to apply it.*");
      }
    }
    if (a.status === "open" && isOrphaned(a)) {
      lines.push(
        a.kind === "comment"
          ? "$(warning) *The text this was on could not be found. Re-attach it to a selection.*"
          : "$(warning) *The text this was on could not be found.*",
      );
    }
    const conflicted = Object.keys(a.conflicts ?? {});
    if (conflicted.length) {
      lines.push(`$(warning) **Conflicting changes:** ${conflicted.join(", ")}. Use *Resolve Conflicts*.`);
    }
    if (a.body) lines.push(a.body);
    else if (a.label) lines.push(`*${a.label}*`);
    else if (a.kind === "comment") lines.push("*(highlight)*");
    return markdown(lines.join("\n\n"));
  }

  /** Rebuilds the threads of a document from the store. */
  sync(uri: string): void {
    const state = this.store.get(uri);
    const doc = vscode.workspace.textDocuments.find((d) => key(d.uri) === uri);
    const seen = new Set<string>();
    for (const a of state?.annotations ?? []) {
      const range = this.range(a, doc);
      if (a.deleted || !range) continue;
      seen.add(a.id);
      const original = a.kind === "suggestion" && doc && rangeOf(a) ? doc.getText(range) : "";
      const reverts = this.store.revertsOf(a.id).map((r) => [r.id, r.status]);
      const signature = JSON.stringify([a, original, reverts]);
      let entry = this.entries.get(a.id);
      if (!entry) {
        const thread = this.controller.createCommentThread(vscode.Uri.parse(uri), range, []);
        thread.collapsibleState = this.expand.delete(a.id)
          ? vscode.CommentThreadCollapsibleState.Expanded
          : vscode.CommentThreadCollapsibleState.Collapsed;
        entry = { thread, uri, signature: "", comments: new Map() };
        this.entries.set(a.id, entry);
        this.ids.set(thread, a.id);
      } else if (!entry.thread.range || !entry.thread.range.isEqual(range)) {
        entry.thread.range = range;
      }
      if (entry.signature === signature) continue;
      entry.signature = signature;
      const { thread } = entry;
      const editable = a.status === "open";
      const comments = new Map<string, AnnoxComment>();
      const build = (view: AnnotationView, root: boolean, body: vscode.MarkdownString) => {
        const old = entry.comments.get(view.id);
        // Keep a comment that is being edited as it is.
        if (old && old.mode === vscode.CommentMode.Editing) {
          comments.set(view.id, old);
          return old;
        }
        const c = new AnnoxComment(view, root, body, editable);
        if (root) {
          const tags = [a.local ? "draft" : "", a.status !== "open" ? a.status : ""].filter(Boolean);
          if (tags.length) c.label = tags.join(", ");
        } else if (view.local) {
          c.label = "draft";
        }
        comments.set(view.id, c);
        return c;
      };
      const list = [build(a, true, this.rootBody(a, doc))];
      for (const r of a.replies ?? []) list.push(build(r, false, markdown(r.body ?? "")));
      entry.comments = comments;
      thread.comments = list;
      thread.label = threadLabel(a);
      thread.contextValue = flags(a, this.store.isReverted(a.id));
      thread.canReply = true;
      thread.state = a.status === "open" ? vscode.CommentThreadState.Unresolved : vscode.CommentThreadState.Resolved;
    }
    for (const [id, entry] of this.entries) {
      if (entry.uri === uri && !seen.has(id)) {
        entry.thread.dispose();
        this.entries.delete(id);
      }
    }
  }

  dispose(): void {
    for (const entry of this.entries.values()) entry.thread.dispose();
    this.entries.clear();
    for (const t of this.pending) t.dispose();
    for (const d of this.disposables) d.dispose();
  }
}
