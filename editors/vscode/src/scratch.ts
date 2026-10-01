// Editors for text that belongs to an annotation, such as a suggestion's
// replacement. They live in an in-memory file system: saving one (Ctrl+S)
// sends the text to the server, the way `:w` does in annox.nvim.

import * as vscode from "vscode";

export const SCHEME = "annox-text";

interface Entry {
  data: Uint8Array;
  mtime: number;
  save: (text: string) => Promise<void>;
}

export class Scratch implements vscode.FileSystemProvider, vscode.Disposable {
  private readonly entries = new Map<string, Entry>();
  private readonly changed = new vscode.EventEmitter<vscode.FileChangeEvent[]>();
  readonly onDidChangeFile = this.changed.event;
  private next = 0;
  private readonly registration = vscode.workspace.registerFileSystemProvider(SCHEME, this, { isCaseSensitive: true });

  /**
   * Opens an editor on `text` beside the current one. Saving it calls
   * `save`, which throws to fail the save.
   */
  async open(
    title: string,
    text: string,
    languageId: string | undefined,
    save: (text: string) => Promise<void>,
  ): Promise<vscode.TextEditor> {
    const uri = vscode.Uri.from({ scheme: SCHEME, path: `/${++this.next}/${title}` });
    this.entries.set(uri.toString(), { data: new TextEncoder().encode(text), mtime: Date.now(), save });
    let doc = await vscode.workspace.openTextDocument(uri);
    if (languageId && doc.languageId !== languageId) {
      doc = await vscode.languages.setTextDocumentLanguage(doc, languageId);
    }
    return vscode.window.showTextDocument(doc, { viewColumn: vscode.ViewColumn.Beside, preview: false });
  }

  private entry(uri: vscode.Uri): Entry {
    const entry = this.entries.get(uri.toString());
    if (!entry) throw vscode.FileSystemError.FileNotFound(uri);
    return entry;
  }

  stat(uri: vscode.Uri): vscode.FileStat {
    if (uri.path.split("/").length < 3) {
      return { type: vscode.FileType.Directory, ctime: 0, mtime: 0, size: 0 };
    }
    const entry = this.entry(uri);
    return { type: vscode.FileType.File, ctime: entry.mtime, mtime: entry.mtime, size: entry.data.byteLength };
  }

  readFile(uri: vscode.Uri): Uint8Array {
    return this.entry(uri).data;
  }

  async writeFile(uri: vscode.Uri, content: Uint8Array): Promise<void> {
    const entry = this.entry(uri);
    await entry.save(new TextDecoder().decode(content));
    entry.data = content;
    entry.mtime = Date.now();
    this.changed.fire([{ type: vscode.FileChangeType.Changed, uri }]);
  }

  watch(): vscode.Disposable {
    return new vscode.Disposable(() => undefined);
  }

  readDirectory(): [string, vscode.FileType][] {
    return [];
  }

  createDirectory(): void {
    throw vscode.FileSystemError.NoPermissions();
  }

  delete(uri: vscode.Uri): void {
    this.entries.delete(uri.toString());
  }

  rename(): void {
    throw vscode.FileSystemError.NoPermissions();
  }

  dispose(): void {
    this.registration.dispose();
    this.changed.dispose();
  }
}

/** Read-only documents with an annotation's history. */
export class HistoryDocs implements vscode.TextDocumentContentProvider, vscode.Disposable {
  static readonly scheme = "annox-history";
  private readonly docs = new Map<string, string>();
  private readonly registration = vscode.workspace.registerTextDocumentContentProvider(HistoryDocs.scheme, this);
  private next = 0;

  async show(title: string, text: string): Promise<void> {
    const name = title.replace(/[\\/\n]/g, " ");
    const uri = vscode.Uri.from({ scheme: HistoryDocs.scheme, path: `/${++this.next}/${name}` });
    this.docs.set(uri.toString(), text);
    const doc = await vscode.workspace.openTextDocument(uri);
    await vscode.window.showTextDocument(doc, { viewColumn: vscode.ViewColumn.Beside, preview: true });
  }

  provideTextDocumentContent(uri: vscode.Uri): string {
    return this.docs.get(uri.toString()) ?? "";
  }

  dispose(): void {
    this.registration.dispose();
  }
}
