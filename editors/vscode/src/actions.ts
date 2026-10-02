// User actions, sent to the server as `annox/*` requests (spec §6.6.2).
//
// Commands take what VS Code passes from a menu (a comment thread, a comment,
// or a reply being written), or an options object when run from a script or
// a test, or nothing, in which case they act on the cursor or selection.

import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import type { Annox } from "./client.ts";
import { AnnoxComment, type Threads } from "./comments.ts";
import type { HistoryDocs, Scratch } from "./scratch.ts";
import { threadMarkdown } from "./view.ts";
import {
  type Store,
  authorName,
  describe,
  firstLine,
  fromRange,
  isConflicted,
  isOrphaned,
  key,
  rangeOf,
} from "./store.ts";
import type { Suggesting } from "./suggesting.ts";
import type {
  AcceptResult,
  AnnotationView,
  CommitResult,
  ConflictEntry,
  HistoryEvent,
  Range,
} from "./types.ts";

/** Options a command accepts when run with arguments. */
export interface Options {
  annotation?: string;
  body?: string;
  replacement?: string;
  range?: Range;
  local?: boolean;
  /** Act on every matching annotation in the document. */
  all?: boolean;
  /** Answer confirmation prompts in advance. */
  confirmed?: boolean;
  field?: string;
  value?: unknown;
  revert?: boolean;
  root?: string;
  confirm?: boolean;
  /** A commit message, which skips the prompt. */
  message?: string;
}

type Arg = vscode.CommentThread | AnnoxComment | vscode.CommentReply | Options | undefined;

function isReply(arg: Arg): arg is vscode.CommentReply {
  return !!arg && typeof arg === "object" && "thread" in arg && "text" in arg;
}

function isThread(arg: Arg): arg is vscode.CommentThread {
  return !!arg && typeof arg === "object" && "uri" in arg && "comments" in arg && !(arg instanceof AnnoxComment);
}

function options(arg: Arg): Options {
  return arg && !isReply(arg) && !isThread(arg) && !(arg instanceof AnnoxComment) ? arg : {};
}

const info = (message: string) => void vscode.window.showInformationMessage(`annox: ${message}`);
const warn = (message: string) => void vscode.window.showWarningMessage(`annox: ${message}`);

/** Asks a yes/no question in a modal dialog. */
async function confirm(message: string, yes: string, detail?: string): Promise<boolean> {
  return (await vscode.window.showWarningMessage(message, { modal: true, detail }, yes)) === yes;
}

export class Actions {
  constructor(
    private readonly annox: Annox,
    private readonly store: Store,
    private readonly threads: Threads,
    private readonly suggesting: Suggesting,
    private readonly scratch: Scratch,
    private readonly history: HistoryDocs,
  ) {}

  register(): vscode.Disposable[] {
    const commands: Record<string, (arg?: Arg) => unknown> = {
      "annox.init": (arg) => this.init(options(arg)),
      "annox.comment": (arg) => this.comment(arg, false),
      "annox.draft": (arg) => this.comment(arg, true),
      "annox.submitComment": (arg) => this.submitNew(arg as vscode.CommentReply, false),
      "annox.submitDraft": (arg) => this.submitNew(arg as vscode.CommentReply, true),
      "annox.cancelNewThread": (arg) => this.cancelNew(arg),
      "annox.suggest": (arg) => this.suggest(options(arg)),
      "annox.reply": (arg) => this.reply(arg),
      "annox.editAnnotation": (arg) => this.edit(arg),
      "annox.editComment": (arg) => this.threads.startEditing(arg as AnnoxComment),
      "annox.saveComment": (arg) => this.saveComment(arg as AnnoxComment),
      "annox.cancelEditComment": (arg) => this.threads.stopEditing(arg as AnnoxComment),
      "annox.toggleSuggesting": () => this.toggleSuggesting(),
      "annox.undo": () => this.undo(),
      "annox.acceptSuggestion": (arg) => this.accept(arg),
      "annox.acceptAll": (arg) => this.accept({ ...options(arg), all: true }),
      "annox.rejectSuggestion": (arg) => this.setStatus(arg, "suggestion", "rejected", "Reject"),
      "annox.rejectAll": (arg) => this.setStatus({ ...options(arg), all: true }, "suggestion", "rejected", "Reject"),
      "annox.resolveThread": (arg) => this.setStatus(arg, "comment", "resolved", "Resolve"),
      "annox.resolveAll": (arg) => this.setStatus({ ...options(arg), all: true }, "comment", "resolved", "Resolve"),
      "annox.reopenThread": (arg) => this.reopen(arg),
      "annox.publish": (arg) => this.publish(arg),
      "annox.publishAll": (arg) => this.publish({ ...options(arg), all: true }),
      "annox.showThread": (arg) => this.showThread(arg),
      "annox.list": () => this.list(),
      "annox.orphans": () => this.orphans(),
      "annox.reattach": (arg) => this.reattach(arg),
      "annox.retarget": (arg) => this.retarget(arg),
      "annox.resolveConflicts": (arg) => this.resolveConflict(arg),
      "annox.history": (arg) => this.showHistory(arg),
      "annox.open": (arg) => this.open(arg),
      "annox.commit": (arg) => this.commit(options(arg)),
      "annox.restartServer": () => this.annox.restart(),
    };
    return Object.entries(commands).map(([id, fn]) => vscode.commands.registerCommand(id, fn));
  }

  // Context ---------------------------------------------------------------

  /** The active editor, once its document is known to be in an annox
   * workspace. Says why not otherwise. */
  private async editor(): Promise<vscode.TextEditor | undefined> {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      warn("open a file first");
      return undefined;
    }
    if (!this.annox.client) {
      await this.annox.start();
      if (!this.annox.client) return undefined;
    }
    if (!this.store.has(editor.document.uri)) await this.annox.fetch(editor.document.uri);
    if (!this.store.has(editor.document.uri)) {
      warn("this file is not in an annox workspace; run “annox: Initialize Workspace”");
      return undefined;
    }
    return editor;
  }

  /** Annotations whose range contains the cursor. */
  private underCursor(editor: vscode.TextEditor): AnnotationView[] {
    const pos = editor.selection.active;
    return this.store.annotations(editor.document.uri).filter((a) => {
      const r = rangeOf(a);
      return r !== undefined && editor.document.validateRange(r).contains(pos);
    });
  }

  private async pick(items: AnnotationView[], placeHolder: string): Promise<AnnotationView | undefined> {
    if (items.length <= 1) return items[0];
    const choice = await vscode.window.showQuickPick(
      items.map((a) => ({ label: describe(a), description: authorName(a), a })),
      { placeHolder },
    );
    return choice?.a;
  }

  /**
   * The annotation an action applies to: the thread or comment it was run
   * from, `annotation` in its options, or the one under the cursor matching
   * `keep` (asking if there are several).
   */
  private async target(
    arg: Arg,
    keep: (a: AnnotationView) => boolean,
    what: string,
  ): Promise<{ uri: vscode.Uri; view: AnnotationView } | undefined> {
    let id: string | undefined;
    if (isReply(arg)) id = this.threads.idOf(arg.thread);
    else if (isThread(arg) || arg instanceof AnnoxComment) id = this.threads.idOf(arg);
    else id = options(arg).annotation ?? (arg as { a?: AnnotationView } | undefined)?.a?.id;
    if (id) {
      let found = this.store.find(id);
      const active = vscode.window.activeTextEditor?.document.uri;
      if (!found && active) {
        // Pushed state can lag behind a request that just created it.
        await this.annox.fetch(active);
        found = this.store.find(id);
      }
      if (!found) {
        warn("that annotation is no longer here");
        return undefined;
      }
      return { uri: vscode.Uri.parse(found.uri), view: found.view };
    }
    const editor = await this.editor();
    if (!editor) return undefined;
    const hits = this.underCursor(editor).filter(keep);
    if (hits.length === 0) {
      info(`no ${what} under the cursor`);
      return undefined;
    }
    const view = await this.pick(hits, `Which ${what}?`);
    return view && { uri: editor.document.uri, view };
  }

  private textDocument(uri: vscode.Uri) {
    return { uri: this.annox.uri(uri) };
  }

  // Workspace -------------------------------------------------------------

  /** Creates an annox workspace (§5.3), by default at the git root of the
   * active file or the workspace folder, and restarts the server. */
  async init(opts: Options): Promise<void> {
    const root = opts.root ?? this.defaultRoot();
    if (!root) {
      warn("open a folder or a file first");
      return;
    }
    const dir = path.join(root, ".annox");
    if (fs.existsSync(path.join(dir, "annox.json"))) {
      info(`${root} is already an annox workspace`);
      return;
    }
    if (opts.confirm !== false && !(await confirm(`Create an annox workspace in ${root}?`, "Create"))) return;
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, "annox.json"), '{ "format": 1 }\n');
    fs.writeFileSync(path.join(dir, ".gitignore"), "cache/\nlocal/\nsynced/\n");
    await this.annox.restart();
    info(`created a workspace in ${root}`);
  }

  private defaultRoot(): string | undefined {
    const uri = vscode.window.activeTextEditor?.document.uri;
    if (uri?.scheme === "file") {
      let dir = path.dirname(uri.fsPath);
      for (;;) {
        if (fs.existsSync(path.join(dir, ".git"))) return dir;
        const parent = path.dirname(dir);
        if (parent === dir) break;
        dir = parent;
      }
      const folder = vscode.workspace.getWorkspaceFolder(uri);
      if (folder) return folder.uri.fsPath;
      return path.dirname(uri.fsPath);
    }
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  }

  // Writing -----------------------------------------------------------------

  /** Comments on the selection or the cursor. Without a body, opens an empty
   * thread there to write it in. */
  async comment(arg: Arg, local: boolean): Promise<AnnotationView | undefined> {
    const opts = options(arg);
    const editor = await this.editor();
    if (!editor) return undefined;
    const range = opts.range ?? fromRange(editor.selection);
    if (opts.body === undefined) {
      const r = new vscode.Range(range.start.line, range.start.character, range.end.line, range.end.character);
      this.threads.openNew(editor.document.uri, r, local || !!opts.local);
      return undefined;
    }
    return this.create(editor.document.uri, range, opts.body, local || !!opts.local);
  }

  private async create(uri: vscode.Uri, range: Range, body: string, local: boolean) {
    const view = await this.annox.request<AnnotationView>("annox/create", {
      textDocument: this.textDocument(uri),
      kind: "comment",
      range,
      body,
      local,
    });
    if (view) this.threads.reveal(view.id);
    return view;
  }

  /** Sends the comment written in a new, empty thread. */
  private async submitNew(reply: vscode.CommentReply, local: boolean): Promise<void> {
    const text = reply.text.trim();
    if (!text) return;
    const range = reply.thread.range ?? new vscode.Range(0, 0, 0, 0);
    const view = await this.create(reply.thread.uri, fromRange(range), reply.text, local);
    if (view) this.threads.discard(reply.thread);
  }

  private cancelNew(arg: Arg): void {
    const thread = isReply(arg) ? arg.thread : isThread(arg) ? arg : undefined;
    if (thread) this.threads.discard(thread);
  }

  /** Suggests replacing the selection, or inserting at the cursor. */
  async suggest(opts: Options): Promise<AnnotationView | undefined> {
    const editor = await this.editor();
    if (!editor) return undefined;
    const doc = editor.document;
    const range = opts.range ?? fromRange(editor.selection);
    const send = (replacement: string) =>
      this.annox.request<AnnotationView>("annox/create", {
        textDocument: this.textDocument(doc.uri),
        kind: "suggestion",
        range,
        replacement,
      });
    if (opts.replacement !== undefined) return send(opts.replacement);
    const original = doc.getText(
      new vscode.Range(range.start.line, range.start.character, range.end.line, range.end.character),
    );
    if (original.includes("\n")) {
      // Several lines: write the replacement in an editor, and save to send it.
      let created: AnnotationView | undefined;
      await this.scratch.open("Suggested text", original, doc.languageId, async (text) => {
        const view = created
          ? await this.retargetRequest(created.id, text)
          : await send(text);
        if (!view) throw new Error("the suggestion was not saved");
        created = view;
      });
      info("edit the suggested text and save (Ctrl+S) to send it");
      return undefined;
    }
    const replacement = await vscode.window.showInputBox({
      prompt: original ? `Replace “${original}” with` : "Insert",
      value: original,
    });
    if (replacement === undefined || replacement === original) return undefined;
    return send(replacement);
  }

  /** Re-targets suggestion `id` at its current range, with a new replacement. */
  private retargetRequest(id: string, replacement: string) {
    const range = this.store.find(id)?.view.resolution?.range;
    if (!range) return Promise.resolve(undefined);
    return this.annox.request<AnnotationView>("annox/retarget", { annotation: id, range, replacement });
  }

  /** Replies to a thread: the text written in its reply box, or `body`.
   * From the cursor without a body, opens the thread to reply in. */
  async reply(arg: Arg): Promise<AnnotationView | undefined> {
    const t = await this.target(arg, () => true, "thread");
    if (!t) return undefined;
    const body = isReply(arg) ? arg.text : options(arg).body;
    if (body === undefined) {
      this.threads.reveal(t.view.id);
      return undefined;
    }
    if (!body.trim()) return undefined;
    return this.annox.request<AnnotationView>("annox/reply", { parent: t.view.id, body });
  }

  private async saveComment(comment: AnnoxComment): Promise<void> {
    const body = typeof comment.body === "string" ? comment.body : comment.body.value;
    const view = await this.annox.request<AnnotationView>("annox/edit", {
      annotation: comment.view.id,
      body: body.trim() === "" ? null : body,
    });
    if (view) this.threads.stopEditing(comment);
  }

  /** Edits the replacement of a suggestion in an editor beside this one, or
   * the text of a comment in its thread. */
  async edit(arg: Arg): Promise<void> {
    if (arg instanceof AnnoxComment && !(arg.root && arg.view.kind === "suggestion")) {
      this.threads.startEditing(arg);
      return;
    }
    const t = await this.target(arg, (a) => a.status === "open", "annotation to edit");
    if (!t) return;
    const a = t.view;
    if (a.kind === "comment") {
      this.threads.reveal(a.id);
      const thread = this.threads.threadOf(a.id);
      const root = thread?.comments[0];
      if (root instanceof AnnoxComment) this.threads.startEditing(root);
      return;
    }
    if (!a.applicable) {
      warn("this suggestion is stale; re-target it at the text it should replace");
      return;
    }
    const opts = options(arg);
    if (opts.replacement !== undefined) {
      await this.retargetRequest(a.id, opts.replacement);
      return;
    }
    const doc = await vscode.workspace.openTextDocument(t.uri);
    await this.scratch.open("Suggested text", a.edit?.replacement ?? "", doc.languageId, async (text) => {
      const before = this.store.find(a.id)?.view;
      const view = await this.retargetRequest(a.id, text);
      if (!view) throw new Error("the suggestion was not saved");
      const range = before?.resolution?.range;
      if (range) this.suggesting.pushUndo(t.uri, { id: a.id, range, replacement: before?.edit?.replacement });
    });
  }

  // Suggestion mode -----------------------------------------------------------

  private async toggleSuggesting(): Promise<void> {
    const editor = await this.editor();
    if (editor) await this.suggesting.toggle(editor);
  }

  private async undo(): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (editor) await this.suggesting.undo(editor);
  }

  // Status ----------------------------------------------------------------

  /** The open annotations of `kind` in the document: those touching the
   * selection unless `all`. */
  private openInDocument(editor: vscode.TextEditor, kind: string, all: boolean): AnnotationView[] {
    const sel = editor.selection;
    return this.store.annotations(editor.document.uri).filter((a) => {
      const r = rangeOf(a);
      if (a.kind !== kind || a.status !== "open" || !r) return false;
      return all || editor.document.validateRange(r).intersection(sel) !== undefined;
    });
  }

  /** Accepts the suggestion of the thread, under the cursor, every one
   * touching the selection, or with `all` every one in the document. */
  async accept(arg: Arg): Promise<unknown> {
    const opts = options(arg);
    const fromThread = isThread(arg) || isReply(arg) || arg instanceof AnnoxComment || !!opts.annotation;
    const editor = vscode.window.activeTextEditor;
    if (!fromThread && (opts.all || (editor && !editor.selection.isEmpty))) {
      const e = await this.editor();
      if (!e) return undefined;
      await this.suggesting.flush(e.document.uri);
      return this.acceptAll(e, this.openInDocument(e, "suggestion", !!opts.all), opts.confirmed);
    }
    const t = await this.target(arg, (a) => a.kind === "suggestion" && a.status === "open", "suggestion");
    if (!t) return undefined;
    await this.suggesting.flush(t.uri);
    return this.annox.request<AnnotationView>("annox/accept", { annotation: t.view.id });
  }

  /**
   * Accepts several suggestions as one edit that a single undo reverts, and
   * reports the ones the server skipped. Suggestions found by partial context
   * (steps 3 and 5) need `confirmed`, which is asked for once for the whole
   * batch if it isn't given (§4.3).
   */
  private async acceptAll(editor: vscode.TextEditor, suggestions: AnnotationView[], confirmed?: boolean) {
    if (suggestions.length === 0) {
      info("no suggestions to accept");
      return undefined;
    }
    // Pushed state can lag behind the document; resolve against it now. The
    // server remembers `includeClosed` for later pushes, so keep asking for it.
    const fresh = await this.annox.request<{ annotations: AnnotationView[] }>("annox/annotations", {
      textDocument: this.textDocument(editor.document.uri),
      includeClosed: true,
    });
    if (!fresh) return undefined;
    const wanted = new Set(suggestions.map((a) => a.id));
    let current = fresh.annotations.filter((a) => wanted.has(a.id) && a.resolution?.range);
    const partial = current.filter((a) => a.resolution?.step === 3 || a.resolution?.step === 5).length;
    if (partial > 0 && confirmed === undefined) {
      const all = `Accept All ${current.length}`;
      const rest = `Accept Only the Other ${current.length - partial}`;
      const one = partial === 1;
      const choice = await vscode.window.showWarningMessage(
        `${partial} of these suggestion${one ? "" : "s"} moved because the text around ${one ? "it" : "them"} changed.`,
        { modal: true, detail: `Check ${one ? "it is" : "they are"} shown in the right place.` },
        ...(partial < current.length ? [all, rest] : [all]),
      );
      if (!choice) return undefined;
      confirmed = choice === all;
    }
    current = current.sort((a, b) => {
      const ra = rangeOf(a) as vscode.Range;
      const rb = rangeOf(b) as vscode.Range;
      return ra.start.compareTo(rb.start);
    });
    const result = await this.annox.request<{ results: AcceptResult[] }>("annox/acceptAll", {
      annotations: current.map((a) => a.id),
      confirmed: confirmed ?? false,
    });
    if (!result) return undefined;
    let accepted = 0;
    const skipped = new Map<string, number>();
    for (const r of result.results ?? []) {
      if ("error" in r) {
        const e = r.error as { message?: string } | string;
        const message = typeof e === "object" && e?.message ? e.message : String(e);
        skipped.set(message, (skipped.get(message) ?? 0) + 1);
      } else {
        accepted++;
      }
    }
    const parts = [`accepted ${accepted}`];
    for (const [message, n] of skipped) parts.push(`skipped ${n} (${message})`);
    (skipped.size ? warn : info)(parts.join("; "));
    return result;
  }

  /**
   * Sets `status` on the annotation of the thread or under the cursor, or,
   * with a selection or `all`, on every open annotation of `kind` touching
   * the selection or in the document, after asking once.
   */
  private async setStatus(arg: Arg, kind: string, status: string, verb: string): Promise<void> {
    const opts = options(arg);
    const fromThread = isThread(arg) || isReply(arg) || arg instanceof AnnoxComment || !!opts.annotation;
    const editor = vscode.window.activeTextEditor;
    if (!fromThread && (opts.all || (editor && !editor.selection.isEmpty))) {
      const e = await this.editor();
      if (!e) return;
      const items = this.openInDocument(e, kind, !!opts.all);
      if (items.length === 0) {
        info(`no ${kind}s to ${verb.toLowerCase()}`);
        return;
      }
      if (items.length > 1 && !opts.confirmed) {
        if (!(await confirm(`${verb} ${items.length} ${kind}s?`, `${verb} All ${items.length}`))) return;
      }
      const results = await Promise.all(
        items.map((a) => this.annox.request("annox/setStatus", { annotation: a.id, status }, true)),
      );
      const failed = results.filter((r) => r === undefined).length;
      (failed ? warn : info)(`${status} ${items.length - failed}${failed ? `; ${failed} failed` : ""}`);
      return;
    }
    const t = await this.target(arg, (a) => a.kind === kind && a.status === "open", kind);
    if (t) await this.annox.request("annox/setStatus", { annotation: t.view.id, status });
  }

  private async reopen(arg: Arg): Promise<void> {
    const t = await this.target(arg, (a) => a.status !== "open", "closed thread");
    if (t) await this.annox.request("annox/setStatus", { annotation: t.view.id, status: "open" });
  }

  /** Publishes local drafts (§5.11): the one of the thread or under the
   * cursor, or with `all` every draft in the document. */
  private async publish(arg: Arg): Promise<void> {
    const opts = options(arg);
    if (opts.all) {
      const editor = await this.editor();
      if (!editor) return;
      const ids = this.store
        .annotations(editor.document.uri)
        .filter((a) => a.local)
        .map((a) => a.id);
      if (ids.length === 0) {
        info("no drafts to publish");
        return;
      }
      await this.annox.request("annox/publish", { annotations: ids });
      return;
    }
    const t = await this.target(arg, (a) => !!a.local, "draft");
    if (t) await this.annox.request("annox/publish", { annotations: [t.view.id] });
  }

  /** Commits the workspace's annotation files to git (`annox/commit`), after
   * showing how many there are and letting the user edit the message. */
  private async commit(opts: Options): Promise<void> {
    const editor = await this.editor();
    if (!editor) return;
    const textDocument = { uri: this.annox.uri(editor.document.uri) };
    const planned = await this.annox.request<CommitResult>("annox/commit", { textDocument, dryRun: true });
    if (!planned) return;
    const n = planned.files;
    if (n === 0) {
      info("no annotation changes to commit");
      return;
    }
    const message =
      opts.message ??
      (await vscode.window.showInputBox({
        prompt: `Commit ${n} annotation file${n === 1 ? "" : "s"} to git`,
        value: planned.message ?? "",
      }));
    if (!message?.trim()) return;
    const result = await this.annox.request<CommitResult>("annox/commit", { textDocument, message });
    if (!result) return;
    if (!result.commit) info("no annotation changes to commit");
    else info(`committed ${result.commit.slice(0, 7)} ${message}`);
  }

  // Viewing -----------------------------------------------------------------

  /** Opens a thread and puts the cursor on its text. */
  private async revealAnnotation(uri: vscode.Uri, a: AnnotationView): Promise<void> {
    const doc = await vscode.workspace.openTextDocument(uri);
    const r = rangeOf(a);
    const editor = await vscode.window.showTextDocument(doc, { preserveFocus: false });
    if (r) {
      const range = doc.validateRange(r);
      editor.selection = new vscode.Selection(range.start, range.start);
      editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
    }
    this.threads.reveal(a.id);
  }

  /** Goes to an annotation and opens its thread, or, if it has no place in
   * the text, shows the thread and its history beside the editor. */
  private async open(arg: Arg): Promise<void> {
    const t = await this.target(arg, () => true, "annotation");
    if (!t) return;
    if (this.threads.threadOf(t.view.id)) {
      await this.revealAnnotation(t.uri, t.view);
      return;
    }
    const events = (await this.annox.request<HistoryEvent[]>("annox/history", { annotation: t.view.id })) ?? [];
    const history = events.map((e) => {
      const detail = e.body ?? e.status ?? e.edit?.replacement ?? "";
      return `| ${e.time ?? ""} | ${e.type} | ${authorName(e)} | ${(firstLine(String(detail)) ?? "").replace(/\|/g, "\\|")} |`;
    });
    const text = [threadMarkdown(t.view), "## History", "", "| Time | Event | Author | |", "| --- | --- | --- | --- |", ...history, ""];
    await this.history.show(`${describe(t.view).slice(0, 40)}.md`, text.join("\n"));
  }

  private async showThread(arg: Arg): Promise<void> {
    const t = await this.target(arg, () => true, "annotation");
    if (t) this.threads.reveal(t.view.id);
  }

  private async list(): Promise<void> {
    const editor = await this.editor();
    if (!editor) return;
    const items = this.store.annotations(editor.document.uri).map((a) => {
      const r = rangeOf(a);
      const where = r ? `line ${r.start.line + 1}` : "could not be located";
      const status = a.status === "open" ? "" : ` · ${a.status}`;
      return { label: describe(a), description: `${authorName(a)} · ${where}${status}`, a };
    });
    if (items.length === 0) {
      info("no annotations in this file");
      return;
    }
    const choice = await vscode.window.showQuickPick(items, { placeHolder: "annox annotations", matchOnDescription: true });
    if (choice) await this.revealAnnotation(editor.document.uri, choice.a);
  }

  /** Lists annotations whose text could not be found (§3.7.3) and opens the
   * chosen thread. */
  private async orphans(): Promise<void> {
    const editor = await this.editor();
    if (!editor) return;
    const orphans = this.store.annotations(editor.document.uri).filter((a) => a.status === "open" && isOrphaned(a));
    if (orphans.length === 0) {
      info("no annotations that could not be located");
      return;
    }
    const choice = await vscode.window.showQuickPick(
      orphans.map((a) => ({
        label: describe(a),
        description: authorName(a),
        detail: a.kind === "comment" ? "Select text and run “annox: Re-attach” to place it again" : undefined,
        a,
      })),
      { placeHolder: "Annotations that could not be located" },
    );
    if (choice) this.threads.reveal(choice.a.id);
  }

  /** Shows every event of an annotation (§2.5.5). */
  private async showHistory(arg: Arg): Promise<void> {
    const t = await this.target(arg, () => true, "annotation");
    if (!t) return;
    const events = await this.annox.request<HistoryEvent[]>("annox/history", { annotation: t.view.id });
    if (!events) return;
    const lines = events.map((e) => {
      const detail = e.body ?? e.status ?? e.edit?.replacement ?? "";
      return `${e.time ?? ""}  ${e.type.padEnd(9)} ${authorName(e)}  ${firstLine(String(detail)) ?? ""}`;
    });
    await this.history.show(`History of ${describe(t.view).slice(0, 40)}`, lines.join("\n"));
  }

  // Repairs ---------------------------------------------------------------

  /** The current selection, or says to make one. */
  private selection(uri: vscode.Uri, what: string): Range | undefined {
    const editor = vscode.window.visibleTextEditors.find((e) => key(e.document.uri) === key(uri));
    if (!editor) {
      warn(`open the file and select the text to ${what}`);
      return undefined;
    }
    return fromRange(editor.selection);
  }

  /** Re-attaches an orphaned comment to the selection (§3.7.4). */
  private async reattach(arg: Arg): Promise<void> {
    const opts = options(arg);
    let t: { uri: vscode.Uri; view: AnnotationView } | undefined;
    if (isThread(arg) || arg instanceof AnnoxComment || opts.annotation) {
      t = await this.target(arg, () => true, "comment");
    } else {
      const editor = await this.editor();
      if (!editor) return;
      const orphans = this.store
        .annotations(editor.document.uri)
        .filter((a) => a.kind === "comment" && a.status === "open" && isOrphaned(a));
      if (orphans.length === 0) {
        info("no comments to re-attach");
        return;
      }
      const view = await this.pick(orphans, "Re-attach which comment to the selection?");
      t = view && { uri: editor.document.uri, view };
    }
    if (!t) return;
    const range = opts.range ?? this.selection(t.uri, "attach it to");
    if (!range) return;
    await this.annox.request("annox/reattach", { annotation: t.view.id, range });
    this.threads.reveal(t.view.id);
  }

  /** Points a stale suggestion at the selection (§4.2.1). */
  private async retarget(arg: Arg): Promise<void> {
    const opts = options(arg);
    let t: { uri: vscode.Uri; view: AnnotationView } | undefined;
    if (isThread(arg) || arg instanceof AnnoxComment || opts.annotation) {
      t = await this.target(arg, () => true, "suggestion");
    } else {
      const editor = await this.editor();
      if (!editor) return;
      const stale = this.store
        .annotations(editor.document.uri)
        .filter((a) => a.kind === "suggestion" && a.status === "open" && !a.applicable);
      if (stale.length === 0) {
        info("no stale suggestions to re-target");
        return;
      }
      const view = await this.pick(stale, "Re-target which suggestion at the selection?");
      t = view && { uri: editor.document.uri, view };
    }
    if (!t) return;
    const range = opts.range ?? this.selection(t.uri, "re-target it at");
    if (!range) return;
    const replacement =
      opts.replacement ??
      (await vscode.window.showInputBox({ prompt: "Replace the selection with", value: t.view.edit?.replacement ?? "" }));
    if (replacement === undefined) return;
    await this.annox.request("annox/retarget", { annotation: t.view.id, range, replacement });
    this.threads.reveal(t.view.id);
  }

  /** Resolves the conflicts of an annotation (§2.5.4), one field at a time. */
  private async resolveConflict(arg: Arg): Promise<void> {
    const opts = options(arg);
    let t: { uri: vscode.Uri; view: AnnotationView } | undefined;
    if (isThread(arg) || arg instanceof AnnoxComment || opts.annotation) {
      t = await this.target(arg, isConflicted, "conflicted annotation");
    } else {
      const editor = await this.editor();
      if (!editor) return;
      const conflicted = this.store.annotations(editor.document.uri).filter(isConflicted);
      if (conflicted.length === 0) {
        info("nothing to resolve");
        return;
      }
      const view = await this.pick(conflicted, "Resolve the conflicts of which annotation?");
      t = view && { uri: editor.document.uri, view };
    }
    if (!t) return;
    const a = t.view;
    const send = (field: string, value: unknown, revert?: boolean) =>
      this.annox.request("annox/resolveConflict", { annotation: a.id, field, value, revert });
    if (opts.field) {
      await send(opts.field, opts.value, opts.revert);
      return;
    }
    const fields = Object.keys(a.conflicts ?? {});
    if (fields.length === 0) {
      info("this annotation has no conflicts");
      return;
    }
    const field =
      fields.length === 1
        ? fields[0]
        : await vscode.window.showQuickPick(fields, { placeHolder: "Resolve which conflicting field?" });
    if (!field) return;
    const entries = a.conflicts?.[field] ?? [];
    const merge = { label: "$(edit) Write a merged version…", entry: undefined as ConflictEntry | undefined };
    const choices = entries.map((e) => ({
      label: field === "target" ? "(anchor)" : JSON.stringify(e.value),
      description: authorName(e),
      detail: e.time,
      entry: e as ConflictEntry | undefined,
    }));
    const choice = await vscode.window.showQuickPick(
      field === "body" || field === "label" ? [...choices, merge] : choices,
      { placeHolder: `Conflicting ${field}: pick the value to keep` },
    );
    if (!choice) return;
    if (!choice.entry) {
      const current = (a as unknown as Record<string, unknown>)[field];
      await this.scratch.open(`Merged ${field}`, typeof current === "string" ? current : "", "markdown", async (text) => {
        if (!(await send(field, text))) throw new Error("the conflict was not resolved");
      });
      info(`write the merged ${field} and save (Ctrl+S) to resolve the conflict`);
      return;
    }
    let value = choice.entry.value;
    if (field === "deleted") value = value === "delete";
    if (field === "status" && value !== "accepted" && entries.some((e) => e.value === "accepted")) {
      const revert = "Revert the Accepted Edit";
      const keep = "Keep the Document As It Is";
      const c = await vscode.window.showWarningMessage(
        "The document already contains this suggestion.",
        { modal: true },
        revert,
        keep,
      );
      if (!c) return;
      if (c === revert) await this.suggesting.flush(t.uri);
      await send(field, value, c === revert);
      return;
    }
    await send(field, value);
  }
}
