// End-to-end test: real VS Code, real `annox lsp` server, temporary workspace
// (see run.mjs). Steps run in order, like editors/nvim/tests/e2e.lua.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import type { AnnoxComment } from "../../src/comments.ts";
import type { Api } from "../../src/extension.ts";
import type { AnnotationView, Range } from "../../src/types.ts";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function wait<T>(what: string, cond: () => T | Promise<T>, ms = 10000): Promise<NonNullable<T>> {
  const end = Date.now() + ms;
  let last: T | undefined;
  while (Date.now() < end) {
    last = await cond();
    if (last) return last as NonNullable<T>;
    await sleep(25);
  }
  throw new Error(`timed out waiting for ${what}`);
}

function exec<T = AnnotationView>(command: string, ...args: unknown[]): Thenable<T> {
  return vscode.commands.executeCommand<T>(command, ...args);
}

function walk(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    return e.isDirectory() ? walk(p) : [p];
  });
}

/** Prints, and appends to ANNOX_TEST_LOG, since some VS Code builds don't
 * forward the extension host's output. */
function log(line: string): void {
  console.log(line);
  if (process.env.ANNOX_TEST_LOG) fs.appendFileSync(process.env.ANNOX_TEST_LOG, `${line}\n`);
}

const steps: [string, () => Promise<void>][] = [];
const step = (name: string, fn: () => Promise<void>) => steps.push([name, fn]);

export async function run(): Promise<void> {
  const root = process.env.ANNOX_TEST_ROOT as string;
  const ext = vscode.extensions.getExtension("gaardhus.annox");
  assert.ok(ext, "extension installed");
  const api = (await ext.activate()) as Api;
  const doc = await vscode.workspace.openTextDocument(path.join(root, "paper.tex"));
  let editor = await vscode.window.showTextDocument(doc);
  const focus = async () => {
    editor = await vscode.window.showTextDocument(doc, vscode.ViewColumn.One);
  };

  const all = () => api.store.annotations(doc.uri);
  const get = (id: string) => all().find((a) => a.id === id);
  /** The range of `text` on `line`. */
  const range = (line: number, text: string): Range => {
    const s = doc.lineAt(line).text.indexOf(text);
    assert.ok(s >= 0, `"${text}" on line ${line}: ${doc.lineAt(line).text}`);
    return { start: { line, character: s }, end: { line, character: s + text.length } };
  };
  const line = (n: number) => doc.lineAt(n).text;
  const edit = (r: Range, text: string) =>
    editor.edit((b) =>
      b.replace(new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character), text),
    );

  await wait("initial annotations", () => api.store.has(doc.uri));

  step("comment, reply, resolve and reopen", async () => {
    const c = await exec("annox.comment", { range: range(1, "In Section 3"), body: "Which section?" });
    assert.ok(c, "comment created");
    await wait("comment pushed", () => get(c.id));
    const thread = await wait("thread", () => api.threads.threadOf(c.id));
    assert.equal(thread.label, "Comment");
    assert.match(thread.contextValue ?? "", /\bcomment\b.*\bopen\b/);
    assert.equal(thread.comments.length, 1);
    assert.match((thread.comments[0].body as vscode.MarkdownString).value, /Which section\?/);
    assert.equal(thread.comments[0].author.name, "Ada");
    assert.ok(thread.range?.isEqual(new vscode.Range(1, 0, 1, 12)), "thread on the commented text");

    await exec("annox.reply", { annotation: c.id, body: "Section 3." });
    await wait("reply", () => get(c.id)?.replies?.length === 1);
    await wait("reply in thread", () => thread.comments.length === 2);

    await exec("annox.resolveThread", { annotation: c.id });
    await wait("resolved", () => get(c.id)?.status === "resolved");
    await wait("thread resolved", () => thread.state === vscode.CommentThreadState.Resolved);
    assert.match(thread.contextValue ?? "", /\bclosed\b/);
    await exec("annox.reopenThread", { annotation: c.id });
    await wait("reopened", () => get(c.id)?.status === "open");

    // Editing a comment in its thread.
    const rootComment = thread.comments[0] as AnnoxComment;
    await exec("annox.editComment", rootComment);
    assert.equal(rootComment.mode, vscode.CommentMode.Editing);
    rootComment.body = "Which section, exactly?";
    await exec("annox.saveComment", rootComment);
    await wait("body edited", () => get(c.id)?.body === "Which section, exactly?");
    await wait("thread shows the edit", () =>
      (thread.comments[0].body as vscode.MarkdownString).value?.includes("exactly"),
    );
  });

  step("reply from the reply box of a thread", async () => {
    const c = all().find((a) => a.kind === "comment") as AnnotationView;
    const thread = api.threads.threadOf(c.id) as vscode.CommentThread;
    await exec("annox.reply", { thread, text: "From the box." });
    await wait("reply", () => get(c.id)?.replies?.some((r) => r.body === "From the box."));
  });

  step("a new comment written in an empty thread", async () => {
    const thread = api.threads.controller.createCommentThread(doc.uri, new vscode.Range(2, 4, 2, 12), []);
    await exec("annox.submitComment", { thread, text: "Which constant?" });
    const c = await wait("comment", () => all().find((a) => a.body === "Which constant?"));
    assert.deepEqual(c.resolution?.range, range(2, "constant"));
  });

  step("suggest and accept", async () => {
    const s = await exec("annox.suggest", { range: range(1, "prove"), replacement: "show" });
    await wait("applicable", () => get(s.id)?.applicable);
    const thread = await wait("thread", () => api.threads.threadOf(s.id));
    assert.match(thread.contextValue ?? "", /\bsuggestion\b.*\bopen\b.*\bapplicable\b/);
    assert.match((thread.comments[0].body as vscode.MarkdownString).value, /Replace `prove` with `show`/);
    await exec("annox.acceptSuggestion", thread);
    await wait("text changed", () => line(1) === "In Section 3, we show that the bound is tight.");
    await wait("accepted", () => get(s.id)?.status === "accepted");
    // Its text is gone now, which is expected, not a problem to flag.
    await wait("thread closed", () => /\bclosed\b/.test(thread.contextValue ?? ""));
    assert.doesNotMatch(thread.contextValue ?? "", /\borphaned\b/);
    assert.doesNotMatch(thread.label ?? "", /not found/);
    assert.equal(thread.collapsibleState, vscode.CommentThreadCollapsibleState.Collapsed);
    assert.match(thread.contextValue ?? "", /\baccepted\b/);

    // Reverting is a new suggestion that restores the text, linked to this one.
    await exec("annox.revertSuggestion", { annotation: s.id, accept: true });
    await wait("text restored", () => line(1) === "In Section 3, we prove that the bound is tight.");
    const revert = await wait("revert accepted", () => all().find((a) => a.reverts === s.id && a.status === "accepted"));
    const revertThread = await wait("revert thread", () => api.threads.threadOf(revert.id));
    assert.match((revertThread.comments[0].body as vscode.MarkdownString).value, /Reverts/);
    assert.ok(api.store.isReverted(s.id), "the original knows it was reverted");
    assert.match(api.store.revertLinks(api.store.find(s.id)!.view).join("\n"), /Reverted by/);
  });

  step("bulk accept is one edit that one undo reverts", async () => {
    await focus();
    const before = doc.getText();
    const a = await exec("annox.suggest", { range: range(2, "small"), replacement: "tiny" });
    const b = await exec("annox.suggest", { range: range(2, "short"), replacement: "brief" });
    await wait("both applicable", () => get(a.id)?.applicable && get(b.id)?.applicable);
    await exec("annox.acceptAll", { confirmed: true });
    await wait("both applied", () => line(2) === "The constant is tiny and the proof is brief.");
    await wait("both accepted", () => get(a.id)?.status === "accepted" && get(b.id)?.status === "accepted");
    await vscode.commands.executeCommand("undo");
    await wait("one undo reverts both", () => doc.getText() === before);
  });

  step("reject, and reject in the selection", async () => {
    await focus();
    const a = await exec("annox.suggest", { range: range(3, "conclude"), replacement: "end" });
    await exec("annox.rejectSuggestion", { annotation: a.id });
    await wait("rejected", () => get(a.id)?.status === "rejected");
    const b = await exec("annox.suggest", { range: range(3, "open"), replacement: "some" });
    const c = await exec("annox.suggest", { range: range(3, "problems"), replacement: "questions" });
    await wait("pushed", () => get(b.id) && get(c.id));
    editor.selection = new vscode.Selection(3, 0, 3, line(3).length);
    await exec("annox.rejectSuggestion", { confirmed: true });
    await wait("both rejected", () => get(b.id)?.status === "rejected" && get(c.id)?.status === "rejected");
    editor.selection = new vscode.Selection(0, 0, 0, 0);
  });

  step("drafts are local until published", async () => {
    const d = await exec("annox.draft", { range: range(3, "We"), body: "Just a note." });
    await wait("draft", () => get(d.id)?.local === true);
    const thread = await wait("thread", () => api.threads.threadOf(d.id));
    assert.match(thread.contextValue ?? "", /\bdraft\b/);
    assert.ok(walk(path.join(root, ".annox/local")).length > 0, "stored in .annox/local");
    await exec("annox.publish", thread);
    await wait("published", () => get(d.id) && !get(d.id)?.local);
  });

  step("a highlight is a comment with no body", async () => {
    const h = await exec("annox.highlight", { range: range(0, "Results") });
    assert.ok(h, "highlight created");
    const a = await wait("highlight", () => get(h.id));
    assert.equal(a.kind, "comment");
    assert.ok(!a.body, "no body");
  });

  step("a stale suggestion is re-targeted", async () => {
    await focus();
    const s = await exec("annox.suggest", { range: range(1, "the bound"), replacement: "this bound" });
    await wait("applicable", () => get(s.id)?.applicable);
    await edit(range(1, "the bound"), "a bound");
    await wait("stale", () => get(s.id)?.applicable === false);
    const thread = await wait("thread", () => api.threads.threadOf(s.id));
    await wait("thread flagged stale", () => /\bstale\b/.test(thread.contextValue ?? ""));
    await exec("annox.retarget", { annotation: s.id, range: range(1, "a bound"), replacement: "this bound" });
    await wait("applicable again", () => get(s.id)?.applicable);
    await exec("annox.rejectSuggestion", { annotation: s.id });
  });

  step("a suggestion's replacement is edited in its own editor", async () => {
    await focus();
    const s = await exec("annox.suggest", { range: range(2, "constant"), replacement: "factor" });
    await wait("applicable", () => get(s.id)?.applicable);
    await exec("annox.editAnnotation", { annotation: s.id });
    const scratch = await wait("scratch editor", () =>
      vscode.window.activeTextEditor?.document.uri.scheme === "annox-text" ? vscode.window.activeTextEditor : undefined,
    );
    assert.equal(scratch.document.getText(), "factor");
    await scratch.edit((b) => b.replace(new vscode.Range(0, 0, 0, 6), "multiplier"));
    assert.ok(await scratch.document.save(), "saved");
    await wait("replacement edited", () => get(s.id)?.edit?.replacement === "multiplier");
    await vscode.commands.executeCommand("workbench.action.closeActiveEditor");
    await focus();
    await exec("annox.rejectSuggestion", { annotation: s.id });
  });

  step("an orphaned comment is re-attached", async () => {
    await focus();
    const c = await exec("annox.comment", { range: range(3, "open problems"), body: "Which ones?" });
    await wait("comment", () => get(c.id));
    const original = line(3);
    await edit({ start: { line: 3, character: 0 }, end: { line: 3, character: original.length } }, "Nothing here.");
    await wait("orphaned", () => get(c.id)?.resolution?.state === "orphaned");
    const thread = await wait("thread", () => api.threads.threadOf(c.id));
    await wait("thread flagged orphaned", () => /\borphaned\b/.test(thread.contextValue ?? ""));
    await exec("annox.reattach", { annotation: c.id, range: range(3, "Nothing") });
    await wait("attached", () => get(c.id)?.resolution?.state !== "orphaned");
    await edit({ start: { line: 3, character: 0 }, end: { line: 3, character: line(3).length } }, original);
    await doc.save();
  });

  step("a closed annotation whose text is gone leaves the editor for the annox view", async () => {
    await focus();
    const c = await exec("annox.comment", { range: range(3, "conclude"), body: "Done." });
    await exec("annox.resolveThread", { annotation: c.id });
    await wait("resolved", () => get(c.id)?.status === "resolved");
    await wait("thread", () => api.threads.threadOf(c.id));
    const original = line(3);
    await edit({ start: { line: 3, character: 0 }, end: { line: 3, character: original.length } }, "Nothing here.");
    await wait("orphaned", () => get(c.id)?.resolution?.state === "orphaned");
    await wait("no thread", () => !api.threads.threadOf(c.id));
    // Opening it from the view shows the thread and its history.
    await exec("annox.open", { a: get(c.id) });
    const shown = await wait("thread page", () => {
      const d = vscode.window.activeTextEditor?.document;
      return d?.uri.scheme === "annox-history" ? d : undefined;
    });
    assert.match(shown.getText(), /Done\./);
    assert.match(shown.getText(), /## History/);
    await vscode.commands.executeCommand("workbench.action.closeActiveEditor");
    await focus();
    await edit({ start: { line: 3, character: 0 }, end: { line: 3, character: line(3).length } }, original);
    await wait("thread back on its text", () => api.threads.threadOf(c.id)?.range);
    await doc.save();
  });

  step("conflicting edits are resolved", async () => {
    const c = all().find((a) => a.body === "Which constant?") as AnnotationView;
    const create = walk(path.join(root, ".annox/docs")).find((p) => path.basename(p) === `${c.id}.json`);
    assert.ok(create, "create event on disk");
    ["mine", "theirs"].forEach((body, i) => {
      const id = `019a0000-0000-7000-8000-00000000000${i + 1}`;
      const event = {
        id,
        annotation: c.id,
        after: [c.id],
        type: "edit",
        author: { id: "mailto:bob@example.org", name: "Bob" },
        time: "2026-09-29T10:00:00Z",
        body,
      };
      // Atomically, as writers must (§5.4): a watcher event for a half-written
      // file can make the server read it empty and never look again.
      const tmp = path.join(path.dirname(create), `.${id}.json.tmp`);
      fs.writeFileSync(tmp, JSON.stringify(event));
      fs.renameSync(tmp, path.join(path.dirname(create), `${id}.json`));
    });
    await wait("conflict pushed", () => get(c.id)?.conflicts?.body, 15000);
    const thread = await wait("thread", () => api.threads.threadOf(c.id));
    await wait("thread flagged conflicted", () => /\bconflicted\b/.test(thread.contextValue ?? ""));
    await exec("annox.resolveConflicts", { annotation: c.id, field: "body", value: "merged" });
    await wait("resolved", () => {
      const a = get(c.id);
      return a && Object.keys(a.conflicts ?? {}).length === 0 && a.body === "merged";
    });
  });

  step("history", async () => {
    const c = all().find((a) => a.body === "merged") as AnnotationView;
    await exec("annox.history", { annotation: c.id });
    const shown = await wait("history editor", () => {
      const d = vscode.window.activeTextEditor?.document;
      return d?.uri.scheme === "annox-history" ? d : undefined;
    });
    assert.match(shown.getText(), /edited: merged/);
    assert.match(shown.getText(), /Bob/);
    await vscode.commands.executeCommand("workbench.action.closeActiveEditor");
  });

  step("presence is sent as the cursor moves", async () => {
    await focus();
    const sent: { selection: Range }[] = [];
    const notify = api.annox.notify.bind(api.annox);
    api.annox.notify = (method, params) => {
      if (method === "annox/setPresence") sent.push(params as { selection: Range });
      notify(method, params);
    };
    editor.selection = new vscode.Selection(1, 5, 1, 5);
    await wait("presence sent", () => sent.length > 0);
    assert.deepEqual(sent[sent.length - 1].selection.start, { line: 1, character: 5 });
    api.annox.notify = notify;
  });

  step("suggestion mode turns edits into suggestions", async () => {
    await focus();
    await doc.save();
    const base = doc.getText();
    const at = (text: string) => new vscode.Position(4, line(4).indexOf(text));
    await exec("annox.toggleSuggesting");
    assert.ok(api.suggesting.isOn(doc.uri), "on");
    const mine = () => all().filter((a) => a.kind === "suggestion" && a.status === "open" && a.applicable);
    const count = mine().length;

    await editor.edit((b) => b.insert(at("here"), "text "));
    await wait("text put back", () => doc.getText() === base);
    const s = await wait("suggestion", () => mine().find((a) => a.edit?.replacement === "text "));
    const point = { line: 4, character: line(4).indexOf("here") };
    assert.deepEqual(s.resolution?.range, { start: point, end: point });

    // Typing where the cursor was left extends the same suggestion.
    const cursor = editor.selection.active;
    assert.deepEqual([cursor.line, cursor.character], [4, line(4).indexOf("here")]);
    await editor.edit((b) => b.insert(cursor, "more "));
    await wait("extended", () => get(s.id)?.edit?.replacement === "text more ");
    assert.equal(mine().length, count + 1, "still one suggestion");
    assert.equal(doc.getText(), base);

    // Undo puts back the earlier replacement, then deletes the suggestion.
    await exec("annox.undo");
    await wait("undo extension", () => get(s.id)?.edit?.replacement === "text ");
    await exec("annox.undo");
    await wait("undo suggestion", () => !get(s.id) || get(s.id)?.deleted);

    // A change inside a word suggests the whole word.
    await editor.edit((b) => b.replace(new vscode.Range(4, 11, 4, 13), "uu"));
    await wait("text put back", () => doc.getText() === base);
    await wait("whole word", () => mine().find((a) => a.edit?.replacement === "uude"));

    // Saving never writes suggested text to the file.
    await editor.edit((b) => b.insert(at("writes"), "quickly "));
    await doc.save();
    assert.equal(fs.readFileSync(doc.uri.fsPath, "utf8"), base, "file has the base text");
    await wait("suggestion from save", () => mine().find((a) => a.edit?.replacement === "quickly "));
    assert.equal(doc.getText(), base);

    // Accepting still edits the document, and that becomes the new base.
    const quick = mine().find((a) => a.edit?.replacement === "quickly ") as AnnotationView;
    await exec("annox.acceptSuggestion", { annotation: quick.id });
    await wait("accepted", () => line(4).includes("quickly writes"));
    await sleep(400);
    assert.ok(line(4).includes("quickly writes"), "accepted text stays");

    await exec("annox.toggleSuggesting");
    assert.ok(!api.suggesting.isOn(doc.uri), "off");
  });

  step("committing annotations", async () => {
    const git = (...args: string[]) => execFileSync("git", ["-C", root, ...args], { encoding: "utf8" });
    git("init", "--quiet");
    git("config", "user.email", "ada@example.org");
    git("config", "user.name", "Ada");
    git("config", "commit.gpgsign", "false");
    await focus();
    await exec("annox.commit", { message: "Review round 1" });
    assert.equal(git("log", "--format=%s").trim(), "Review round 1");
    assert.equal(git("status", "--porcelain", "--", ".annox").trim(), "", "annotation files committed");
    assert.match(git("status", "--porcelain", "paper.tex"), /paper\.tex/, "the document is left alone");
  });

  step("initializing a workspace", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "annox-init-"));
    await exec("annox.init", { root: dir, confirm: false });
    assert.ok(fs.existsSync(path.join(dir, ".annox/annox.json")), "annox.json written");
    assert.match(fs.readFileSync(path.join(dir, ".annox/.gitignore"), "utf8"), /local\//);
    await wait("server back", () => api.store.has(doc.uri));
    fs.rmSync(dir, { recursive: true, force: true });
  });

  for (const [name, fn] of steps) {
    try {
      await fn();
      log(`ok - ${name}`);
    } catch (e) {
      log(`not ok - ${name}\n${e instanceof Error ? e.stack : e}`);
      throw e;
    }
  }
  log("annox vscode e2e: OK");
}
