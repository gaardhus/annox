// The latest state pushed by the server, per document.

import * as vscode from "vscode";
import type { AnnotationView, AnnotationsResult, HistoryEvent, Peer, Range } from "./types.ts";

/** A canonical string for a document URI, so URIs from the server and from
 * VS Code compare equal. */
export function key(uri: string | vscode.Uri): string {
  return (typeof uri === "string" ? vscode.Uri.parse(uri) : uri).toString();
}

export function toRange(r: Range): vscode.Range {
  return new vscode.Range(r.start.line, r.start.character, r.end.line, r.end.character);
}

export function fromRange(r: vscode.Range): Range {
  return {
    start: { line: r.start.line, character: r.start.character },
    end: { line: r.end.line, character: r.end.character },
  };
}

/** The range an annotation resolves to, or undefined if it is orphaned. */
export function rangeOf(a: AnnotationView): vscode.Range | undefined {
  const r = a.resolution?.range;
  return r ? toRange(r) : undefined;
}

export function isOrphaned(a: AnnotationView): boolean {
  return a.resolution?.state === "orphaned" || !a.resolution?.range;
}

export function isConflicted(a: AnnotationView): boolean {
  return Object.keys(a.conflicts ?? {}).length > 0;
}

export function firstLine(text: string | null | undefined): string | undefined {
  return typeof text === "string" ? text.split("\n", 1)[0] : undefined;
}

/** `s` on one line, with runs of whitespace collapsed, at most `max` long. */
export function oneLine(s: string, max = 100): string {
  const t = s.replace(/\s+/g, " ").trim();
  return t.length > max ? `${t.slice(0, max - 1)}…` : t;
}

/** A short summary, without the kind. */
export function summary(a: AnnotationView): string {
  if (a.kind === "suggestion") return `→ ${oneLine(a.edit?.replacement ?? "") || "(delete)"}`;
  return oneLine(firstLine(a.body) || a.label || "(highlight)");
}

export function describe(a: AnnotationView): string {
  return `${a.kind}: ${summary(a)}`;
}

export function authorName(a: { author?: { id: string; name?: string } }): string {
  return a.author?.name ?? a.author?.id ?? "unknown";
}

/** What each event of a thread's history did, as a past-tense verb, with the
 * first line of its text if it has any. Events that change a reply say so. */
export function eventActions(events: HistoryEvent[]): string[] {
  const verbs: Record<string, string> = {
    edit: "edited",
    reanchor: "re-anchored (automatic)",
    retarget: "retargeted",
    delete: "deleted",
    restore: "restored",
  };
  const statuses: Record<string, string> = { open: "reopened", withdrawn: "withdrew" };
  const created: Record<string, string> = { comment: "commented", suggestion: "suggested", reply: "replied" };
  const replies = new Set(events.filter((e) => e.type === "create" && e.kind === "reply").map((e) => e.id));
  return events.map((e) => {
    const verb =
      e.type === "create"
        ? (created[String(e.kind)] ?? "created")
        : e.type === "status"
          ? (statuses[String(e.status)] ?? String(e.status))
          : (verbs[e.type] ?? e.type) + (replies.has(e.annotation) ? " reply" : "");
    const detail = firstLine(e.body ?? e.edit?.replacement);
    return detail ? `${verb}: ${detail}` : verb;
  });
}

export class Store implements vscode.Disposable {
  private readonly docs = new Map<string, AnnotationsResult>();
  private readonly changed = new vscode.EventEmitter<string>();
  private readonly peersChanged = new vscode.EventEmitter<void>();
  /** Fires with the key of a document whose annotations changed. */
  readonly onDidChange = this.changed.event;
  readonly onDidChangePeers = this.peersChanged.event;
  /** Others' presence, as last pushed by the server. */
  peers: Peer[] = [];

  /** Whether the document is in an annox workspace, as far as the server said. */
  has(uri: string | vscode.Uri): boolean {
    return this.docs.has(key(uri));
  }

  get(uri: string | vscode.Uri): AnnotationsResult | undefined {
    return this.docs.get(key(uri));
  }

  annotations(uri: string | vscode.Uri): AnnotationView[] {
    return this.get(uri)?.annotations ?? [];
  }

  set(uri: string | vscode.Uri, state: AnnotationsResult): void {
    const k = key(uri);
    this.docs.set(k, { annotations: state.annotations, document: state.document });
    this.changed.fire(k);
  }

  delete(uri: string | vscode.Uri): void {
    const k = key(uri);
    if (this.docs.delete(k)) this.changed.fire(k);
  }

  clear(): void {
    for (const k of [...this.docs.keys()]) this.delete(k);
    this.setPeers([]);
  }

  keys(): string[] {
    return [...this.docs.keys()];
  }

  /** The root annotation `id` and its document. */
  find(id: string): { uri: string; view: AnnotationView } | undefined {
    for (const [uri, state] of this.docs) {
      const view = state.annotations.find((a) => a.id === id);
      if (view) return { uri, view };
    }
    return undefined;
  }

  /** The suggestions that revert suggestion `id` (§4.3.4), leaving out
   * deleted ones. */
  revertsOf(id: string): AnnotationView[] {
    return [...this.docs.values()].flatMap((s) => s.annotations.filter((a) => a.reverts === id && !a.deleted));
  }

  /** Whether accepted suggestion `id` was already reverted (§4.3.4). */
  isReverted(id: string): boolean {
    return this.revertsOf(id).some((a) => a.status === "accepted");
  }

  /** Lines linking the threads of a revert and the suggestion it reverts, in
   * both directions (§4.3.4). */
  revertLinks(a: AnnotationView): string[] {
    const lines: string[] = [];
    if (a.reverts) {
      const reverted = this.find(a.reverts)?.view;
      lines.push(reverted ? `*Reverts the accepted suggestion “${summary(reverted)}”*` : "*Reverts an accepted suggestion*");
    }
    if (a.status === "accepted") {
      for (const r of this.revertsOf(a.id)) {
        if (r.status === "accepted") lines.push(`*Reverted by “${summary(r)}”*`);
        else if (r.status === "open") lines.push(`*A revert is suggested: “${summary(r)}”*`);
      }
    }
    return lines;
  }

  setPeers(peers: Peer[]): void {
    this.peers = peers;
    this.peersChanged.fire();
  }

  dispose(): void {
    this.changed.dispose();
    this.peersChanged.dispose();
  }
}
