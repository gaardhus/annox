// The latest state pushed by the server, per document.

import * as vscode from "vscode";
import type { AnnotationView, AnnotationsResult, Peer, Range } from "./types.ts";

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

  setPeers(peers: Peer[]): void {
    this.peers = peers;
    this.peersChanged.fire();
  }

  dispose(): void {
    this.changed.dispose();
    this.peersChanged.dispose();
  }
}
