// The language client for `annox lsp` (spec §6).

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  CancellationToken,
  type ClientCapabilities,
  type FeatureState,
  LanguageClient,
  type LanguageClientOptions,
  type ServerOptions,
  type StaticFeature,
} from "vscode-languageclient/node";
import { Store } from "./store.ts";
import type { AnnotationsResult, DidChangeAnnotations, Peer } from "./types.ts";

/** JSON-RPC error codes of §6.6.4. */
export const NO_WORKSPACE = 1005;

/** Declares the client annox-aware (§6.2). */
class AnnoxFeature implements StaticFeature {
  fillClientCapabilities(capabilities: ClientCapabilities): void {
    capabilities.experimental = { ...(capabilities.experimental as object), annox: { version: "0.1" } };
  }
  initialize(): void {}
  getState(): FeatureState {
    return { kind: "static" };
  }
  clear(): void {}
}

function isFile(p: string): boolean {
  try {
    return fs.statSync(p).isFile();
  } catch {
    return false;
  }
}

/** The `annox` binary: the `annox.path` setting, else `annox` on PATH, else
 * the install script's `~/.local/bin`, which a desktop-launched VS Code may
 * not have on its PATH. */
export function resolveBinary(): string {
  const configured = vscode.workspace.getConfiguration("annox").get<string>("path")?.trim();
  if (configured) return configured.replace(/^~(?=$|[\\/])/, os.homedir());
  const exe = process.platform === "win32" ? "annox.exe" : "annox";
  for (const dir of (process.env.PATH ?? "").split(path.delimiter)) {
    if (dir && isFile(path.join(dir, exe))) return path.join(dir, exe);
  }
  const local = path.join(os.homedir(), ".local", "bin", exe);
  return isFile(local) ? local : exe;
}

function author(): { id: string; name?: string } | undefined {
  const config = vscode.workspace.getConfiguration("annox");
  const id = config.get<string>("author.id")?.trim();
  const name = config.get<string>("author.name")?.trim();
  return id ? { id, ...(name ? { name } : {}) } : undefined;
}

export interface ClientHooks {
  /** Called around a `workspace/applyEdit` from the server, i.e. accepting
   * or reverting a suggestion. */
  beforeServerEdit(): void;
  afterServerEdit(): void;
}

export class Annox implements vscode.Disposable {
  client: LanguageClient | undefined;
  readonly output = vscode.window.createOutputChannel("annox", { log: true });

  constructor(
    private readonly store: Store,
    private readonly hooks: ClientHooks,
  ) {}

  async start(): Promise<void> {
    if (this.client) return;
    const command = resolveBinary();
    const serverOptions: ServerOptions = { command, args: ["lsp"] };
    const clientOptions: LanguageClientOptions = {
      documentSelector: [{ scheme: "file" }],
      outputChannel: this.output,
      initializationOptions: { annox: { diagnostics: false, author: author() } },
      middleware: {
        didOpen: async (doc, next) => {
          await next(doc);
          void this.fetch(doc.uri);
        },
        didClose: async (doc, next) => {
          await next(doc);
          this.store.delete(doc.uri);
        },
        // Threads are shown as comment threads, so hovers would repeat them.
        provideHover: () => undefined,
        workspace: {
          handleApplyEdit: async (params, next) => {
            this.hooks.beforeServerEdit();
            try {
              return await next(params, CancellationToken.None);
            } finally {
              this.hooks.afterServerEdit();
            }
          },
        },
      },
    };
    const client = new LanguageClient("annox", "annox", serverOptions, clientOptions);
    client.registerFeature(new AnnoxFeature());
    client.onNotification("annox/didChangeAnnotations", (p: DidChangeAnnotations) => {
      this.store.set(p.textDocument.uri, p);
    });
    client.onNotification("annox/didChangePresence", (p: { peers?: Peer[] }) => {
      this.store.setPeers(p.peers ?? []);
    });
    this.client = client;
    try {
      await client.start();
    } catch (e) {
      this.client = undefined;
      const message = e instanceof Error ? e.message : String(e);
      const choice = await vscode.window.showErrorMessage(
        `annox: could not start \`${command} lsp\`: ${message}. Install annox or set "annox.path".`,
        "Open Settings",
      );
      if (choice) void vscode.commands.executeCommand("workbench.action.openSettings", "annox.path");
      return;
    }
    // Documents opened before the server started.
    for (const doc of vscode.workspace.textDocuments) {
      if (doc.uri.scheme === "file") void this.fetch(doc.uri);
    }
  }

  async stop(): Promise<void> {
    const client = this.client;
    this.client = undefined;
    this.store.clear();
    if (client) await client.stop().catch(() => undefined);
  }

  async restart(): Promise<void> {
    await this.stop();
    await this.start();
  }

  /** Asks for the document's annotations, including closed threads, so that
   * resolved threads show as resolved and later pushes include them (§6.6.3). */
  async fetch(uri: vscode.Uri): Promise<void> {
    if (!this.client) return;
    try {
      const result = await this.client.sendRequest<AnnotationsResult>("annox/annotations", {
        textDocument: { uri: this.client.code2ProtocolConverter.asUri(uri) },
        includeClosed: true,
      });
      this.store.set(uri, result);
    } catch {
      // Not in an annox workspace (NO_WORKSPACE), or the document closed.
    }
  }

  /** The protocol URI of a document. */
  uri(uri: vscode.Uri): string {
    return this.client ? this.client.code2ProtocolConverter.asUri(uri) : uri.toString();
  }

  /** Sends an `annox/*` request. Shows the error and returns undefined if it
   * fails, unless `quiet`. */
  async request<T>(method: string, params: unknown, quiet = false): Promise<T | undefined> {
    if (!this.client) {
      if (!quiet) void vscode.window.showWarningMessage("annox: the annox server is not running");
      return undefined;
    }
    try {
      return await this.client.sendRequest<T>(method, params);
    } catch (e) {
      if (!quiet) {
        const message = e instanceof Error ? e.message : String(e);
        void vscode.window.showErrorMessage(`annox: ${method.replace(/^annox\//, "")} failed: ${message}`);
      }
      return undefined;
    }
  }

  notify(method: string, params: unknown): void {
    void this.client?.sendNotification(method, params);
  }

  dispose(): void {
    void this.stop();
    this.output.dispose();
  }
}
