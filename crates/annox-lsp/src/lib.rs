//! The annox language server (spec §6), first milestone: diagnostics,
//! hover, and code actions for plain LSP clients (§6.5), with accept
//! applied through `workspace/applyEdit` (§6.6.2).

pub mod position;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use annox_core::anchor::{self, Anchor, Resolution, State};
use annox_core::ops;
use annox_core::storage::{Index, Workspace};
use annox_core::suggestion;
use annox_core::text::Text;
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{ApplyWorkspaceEdit, CodeActionRequest, ExecuteCommand, HoverRequest, Request as _};
use lsp_types::{
    ApplyWorkspaceEditParams, ApplyWorkspaceEditResponse, CodeAction, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, Command, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams, ExecuteCommandOptions,
    ExecuteCommandParams, Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams,
    MarkupContent, MarkupKind, NumberOrString, PublishDiagnosticsParams, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions,
    TextEdit, Url, WorkspaceEdit,
};
use serde_json::{json, Map, Value};

use crate::position::{Encoding, LineIndex};

/// Spec version implemented (§6.2).
pub const SPEC_VERSION: &str = "0.0";

const COMMANDS: [&str; 4] = ["annox.accept", "annox.reject", "annox.resolve", "annox.reopen"];

// Error codes (§6.6.4).
const UNKNOWN_ANNOTATION: i32 = 1001;
const STALE_SUGGESTION: i32 = 1002;
const INVALID_OPERATION: i32 = 1003;
const NO_WORKSPACE: i32 = 1005;
const EDIT_NOT_APPLIED: i32 = 1007;

/// Runs the server on `connection` until shutdown.
pub fn run(connection: &Connection) -> anyhow::Result<()> {
    let capabilities = |encoding: Encoding| ServerCapabilities {
        position_encoding: Some(encoding.kind()),
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::FULL),
            save: Some(TextDocumentSyncSaveOptions::Supported(true)),
            ..Default::default()
        })),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: COMMANDS.iter().map(|c| c.to_string()).collect(),
            ..Default::default()
        }),
        experimental: Some(json!({ "annox": { "version": SPEC_VERSION } })),
        ..Default::default()
    };
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let encoding = Encoding::negotiate(
        params.capabilities.general.as_ref().and_then(|g| g.position_encodings.as_deref()),
    );
    let options = params.initialization_options.as_ref().and_then(|o| o.get("annox")).cloned().unwrap_or_default();
    connection.initialize_finish(
        id,
        json!({
            "capabilities": capabilities(encoding),
            "serverInfo": { "name": "annox", "version": env!("CARGO_PKG_VERSION") },
        }),
    )?;
    let mut server = Server {
        connection,
        encoding,
        author: options.get("author").cloned(),
        diagnostics: options.get("diagnostics").and_then(Value::as_bool).unwrap_or(true),
        open: HashMap::new(),
        next_id: 0,
        pending: HashMap::new(),
    };
    server.main_loop()
}

struct OpenDoc {
    text: Text,
    crlf: bool,
}

/// An accept waiting for the client's `workspace/applyEdit` response.
struct PendingAccept {
    command: RequestId,
    annotation: String,
    uri: Url,
    applied_version: String,
}

struct Server<'a> {
    connection: &'a Connection,
    encoding: Encoding,
    author: Option<Value>,
    diagnostics: bool,
    open: HashMap<Url, OpenDoc>,
    next_id: i32,
    pending: HashMap<RequestId, PendingAccept>,
}

/// A root annotation of a document, resolved against its current text.
struct Item {
    id: String,
    state: Value,
    target: Anchor,
    resolution: Resolution,
}

impl Item {
    fn kind(&self) -> &str {
        self.state["kind"].as_str().unwrap_or_default()
    }

    fn status(&self) -> &str {
        self.state["status"].as_str().unwrap_or_default()
    }

    fn is_open(&self) -> bool {
        self.status() == "open" && self.state["deleted"] == json!(false)
    }

    fn applicable(&self) -> bool {
        self.kind() == "suggestion" && suggestion::is_applicable(&self.resolution)
    }

    fn conflicted(&self) -> bool {
        self.state["conflicts"].as_object().is_some_and(|c| !c.is_empty())
    }
}

/// Everything known about one document.
struct Analysis {
    ws: Workspace,
    index: Index,
    text: Text,
    items: Vec<Item>,
    replies: BTreeMap<String, Vec<Value>>,
}

type Failure = (i32, String);

impl Server<'_> {
    fn main_loop(&mut self) -> anyhow::Result<()> {
        for msg in &self.connection.receiver {
            match msg {
                Message::Request(req) => {
                    if self.connection.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    self.on_request(req);
                }
                Message::Notification(n) => self.on_notification(n),
                Message::Response(resp) => self.on_response(resp),
            }
        }
        Ok(())
    }

    fn send(&self, msg: impl Into<Message>) {
        let _ = self.connection.sender.send(msg.into());
    }

    fn reply(&self, id: RequestId, result: Result<Value, Failure>) {
        self.send(match result {
            Ok(v) => Response::new_ok(id, v),
            Err((code, message)) => Response::new_err(id, code, message),
        });
    }

    fn on_request(&mut self, req: Request) {
        let id = req.id.clone();
        match req.method.as_str() {
            HoverRequest::METHOD => {
                let result = serde_json::from_value::<HoverParams>(req.params)
                    .map(|p| json!(self.hover(p)))
                    .map_err(|e| (-32602, e.to_string()));
                self.reply(id, result);
            }
            CodeActionRequest::METHOD => {
                let result = serde_json::from_value::<CodeActionParams>(req.params)
                    .map(|p| json!(self.code_actions(p)))
                    .map_err(|e| (-32602, e.to_string()));
                self.reply(id, result);
            }
            ExecuteCommand::METHOD => match serde_json::from_value::<ExecuteCommandParams>(req.params) {
                Ok(p) => self.execute(id, p),
                Err(e) => self.reply(id, Err((-32602, e.to_string()))),
            },
            _ => self.reply(id, Err((-32601, format!("unhandled method {}", req.method)))),
        }
    }

    fn on_notification(&mut self, n: Notification) {
        match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(n.params) {
                    let uri = p.text_document.uri;
                    self.set_text(uri.clone(), &p.text_document.text);
                    self.publish(&uri);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) {
                    if let Some(change) = p.content_changes.into_iter().last() {
                        let uri = p.text_document.uri;
                        self.set_text(uri.clone(), &change.text);
                        self.publish(&uri);
                    }
                }
            }
            DidSaveTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidSaveTextDocumentParams>(n.params) {
                    self.publish(&p.text_document.uri);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) {
                    self.open.remove(&p.text_document.uri);
                    self.send_diagnostics(p.text_document.uri, vec![]);
                }
            }
            DidChangeWatchedFiles::METHOD => self.publish_all(),
            _ => {}
        }
    }

    fn on_response(&mut self, resp: Response) {
        let Some(pending) = self.pending.remove(&resp.id) else { return };
        let applied = resp
            .response_result
            .ok()
            .and_then(|v| serde_json::from_value::<ApplyWorkspaceEditResponse>(v).ok())
            .is_some_and(|r| r.applied);
        if !applied {
            self.reply(pending.command, Err((EDIT_NOT_APPLIED, "the client did not apply the edit".into())));
            return;
        }
        // Only now is the acceptance recorded (§6.6.2 step 3).
        let result = self.analyze(&pending.uri).ok_or((NO_WORKSPACE, "no annox workspace".into())).and_then(|a| {
            let fields = Map::from_iter([
                ("status".into(), json!("accepted")),
                ("appliedVersion".into(), json!(pending.applied_version)),
            ]);
            self.write_event(&a, &pending.annotation, "status", fields)
        });
        self.reply(pending.command, result.map(|_| Value::Null));
        self.publish_all();
    }

    fn set_text(&mut self, uri: Url, raw: &str) {
        self.open.insert(uri, OpenDoc { text: Text::from_raw(raw), crlf: raw.contains("\r\n") });
    }

    /// Loads, replays, and resolves the annotations of `uri` (§5.7.1, §3.7).
    fn analyze(&self, uri: &Url) -> Option<Analysis> {
        let path: PathBuf = uri.to_file_path().ok()?;
        let ws = Workspace::find(&path)?;
        let rel = ws.relative(&path)?;
        let text = match self.open.get(uri) {
            Some(doc) => doc.text.clone(),
            None => Text::from_raw(&std::fs::read_to_string(&path).ok()?),
        };
        let index = Index::read(&ws);
        let loaded = index.load(&rel);
        let mut items = Vec::new();
        let mut replies: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (id, state) in loaded.derive() {
            if state["kind"] == "reply" {
                if state["deleted"] == json!(false) {
                    let parent = state["parent"].as_str().unwrap_or_default().to_owned();
                    replies.entry(parent).or_default().push(state);
                }
                continue;
            }
            let Ok(target) = serde_json::from_value::<Anchor>(state["target"].clone()) else { continue };
            let resolution = anchor::resolve(&text, &target);
            items.push(Item { id, state, target, resolution });
        }
        Some(Analysis { ws, index, text, items, replies })
    }

    fn author(&self, ws: &Workspace) -> Value {
        if let Some(a) = &self.author {
            return a.clone();
        }
        // Default to the git identity (§2.7).
        let git = |key: &str| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&ws.root)
                .args(["config", key])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|s| !s.is_empty())
        };
        match (git("user.email"), git("user.name")) {
            (Some(email), name) => json!({ "id": format!("mailto:{email}"), "name": name }),
            (None, Some(name)) => json!({ "id": "urn:annox:anonymous", "name": name }),
            (None, None) => json!({ "id": "urn:annox:anonymous" }),
        }
    }

    fn write_event(&self, a: &Analysis, annotation: &str, kind: &str, fields: Map<String, Value>) -> Result<(), Failure> {
        ops::append_event(&a.ws, &a.index, annotation, kind, fields, &self.author(&a.ws))
            .map(|_| ())
            .map_err(|e| (-32603, e.to_string()))
    }

    // Diagnostics (§6.5.1).

    fn publish_all(&self) {
        for uri in self.open.keys() {
            self.publish(uri);
        }
    }

    fn publish(&self, uri: &Url) {
        if !self.diagnostics {
            return;
        }
        let diagnostics = self.analyze(uri).map(|a| self.diagnostics_for(&a)).unwrap_or_default();
        self.send_diagnostics(uri.clone(), diagnostics);
    }

    fn send_diagnostics(&self, uri: Url, diagnostics: Vec<Diagnostic>) {
        let params = PublishDiagnosticsParams { uri, diagnostics, version: None };
        self.send(Notification::new(PublishDiagnostics::METHOD.into(), params));
    }

    fn diagnostics_for(&self, a: &Analysis) -> Vec<Diagnostic> {
        let lines = LineIndex::new(&a.text, self.encoding);
        let mut out = Vec::new();
        let mut orphaned = 0;
        for item in a.items.iter().filter(|i| i.is_open()) {
            let Some((start, end)) = item.resolution.range else {
                orphaned += 1;
                continue;
            };
            let (mut severity, mut message) = if item.kind() == "suggestion" {
                let q = &item.target.selectors.quote.exact;
                let r = item.state["edit"]["replacement"].as_str().unwrap_or_default();
                let text = format!("Suggestion: \"{q}\" → \"{r}\"");
                let text = if item.applicable() { text } else { format!("Stale {text}") };
                (DiagnosticSeverity::INFORMATION, text)
            } else {
                let body = item.state["body"].as_str().and_then(|b| b.lines().next()).filter(|l| !l.is_empty());
                let label = item.state["label"].as_str();
                (DiagnosticSeverity::HINT, body.or(label).unwrap_or("Highlight").to_owned())
            };
            if item.conflicted() {
                severity = DiagnosticSeverity::WARNING;
                message = format!("Conflict: {message}");
            }
            out.push(Diagnostic {
                range: lines.range(start, end),
                severity: Some(severity),
                code: Some(NumberOrString::String(item.kind().to_owned())),
                source: Some("annox".into()),
                message,
                data: Some(json!({ "annotation": item.id })),
                ..Default::default()
            });
        }
        if orphaned > 0 {
            out.push(Diagnostic {
                range: lines.range(0, 0),
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("annox".into()),
                message: format!("{orphaned} annotation{} could not be located", if orphaned == 1 { "" } else { "s" }),
                ..Default::default()
            });
        }
        out
    }

    // Hover (§6.5.2).

    fn hover(&self, p: HoverParams) -> Option<Hover> {
        let doc = &p.text_document_position_params;
        let a = self.analyze(&doc.text_document.uri)?;
        let lines = LineIndex::new(&a.text, self.encoding);
        let offset = lines.offset(doc.position);
        let hits: Vec<&Item> = a
            .items
            .iter()
            .filter(|i| i.state["deleted"] == json!(false))
            .filter(|i| i.resolution.range.is_some_and(|(s, e)| s <= offset && offset <= e))
            .collect();
        let first = hits.first()?.resolution.range?;
        let sections: Vec<String> = hits.iter().map(|i| thread_markdown(i, a.replies.get(&i.id))).collect();
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: sections.join("\n\n---\n\n") }),
            range: Some(lines.range(first.0, first.1)),
        })
    }

    // Code actions and commands (§6.5.3).

    fn code_actions(&self, p: CodeActionParams) -> Vec<CodeActionOrCommand> {
        let Some(a) = self.analyze(&p.text_document.uri) else { return vec![] };
        let lines = LineIndex::new(&a.text, self.encoding);
        let (from, to) = (lines.offset(p.range.start), lines.offset(p.range.end));
        let mut actions = Vec::new();
        for item in &a.items {
            let Some((s, e)) = item.resolution.range else { continue };
            if s > to || e < from || item.state["deleted"] != json!(false) {
                continue;
            }
            for (command, title) in offered_commands(item) {
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: title.into(),
                    command: Some(Command {
                        title: title.into(),
                        command: command.into(),
                        arguments: Some(vec![json!({ "annotation": item.id })]),
                    }),
                    ..Default::default()
                }));
            }
        }
        actions
    }

    fn execute(&mut self, id: RequestId, p: ExecuteCommandParams) {
        let annotation = p.arguments.first().and_then(|a| a.get("annotation")).and_then(Value::as_str);
        let Some(annotation) = annotation.map(str::to_owned) else {
            return self.reply(id, Err((-32602, "missing annotation argument".into())));
        };
        let found = self.open.keys().find_map(|uri| {
            let a = self.analyze(uri)?;
            let pos = a.items.iter().position(|i| i.id == annotation)?;
            Some((uri.clone(), a, pos))
        });
        let Some((uri, a, pos)) = found else {
            return self.reply(id, Err((UNKNOWN_ANNOTATION, format!("unknown annotation {annotation}"))));
        };
        let item = &a.items[pos];
        if !offered_commands(item).iter().any(|(c, _)| *c == p.command) {
            return self.reply(id, Err((INVALID_OPERATION, format!("{} does not apply to this annotation", p.command))));
        }
        let status = match p.command.as_str() {
            "annox.accept" => return self.start_accept(id, uri, &a, pos),
            "annox.reject" => "rejected",
            "annox.resolve" => "resolved",
            _ => "open",
        };
        let result = self.write_event(&a, &annotation, "status", Map::from_iter([("status".into(), json!(status))]));
        self.reply(id, result.map(|_| Value::Null));
        self.publish_all();
    }

    /// Sends the suggestion's edit with `workspace/applyEdit`; the status
    /// event is written when the client confirms (§6.6.2).
    fn start_accept(&mut self, command: RequestId, uri: Url, a: &Analysis, pos: usize) {
        let item = &a.items[pos];
        let replacement = item.state["edit"]["replacement"].as_str().unwrap_or_default();
        let applied = match suggestion::apply(&a.text, &item.target, replacement) {
            Ok(applied) => applied,
            Err(_) => return self.reply(command, Err((STALE_SUGGESTION, "the suggestion is stale".into()))),
        };
        let lines = LineIndex::new(&a.text, self.encoding);
        let crlf = self.open.get(&uri).is_some_and(|d| d.crlf);
        let new_text = if crlf { replacement.replace('\n', "\r\n") } else { replacement.to_owned() };
        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([(
                uri.clone(),
                vec![TextEdit { range: lines.range(applied.start, applied.end), new_text }],
            )])),
            ..Default::default()
        };
        self.next_id += 1;
        let request_id = RequestId::from(format!("annox-apply-{}", self.next_id));
        self.pending.insert(
            request_id.clone(),
            PendingAccept { command, annotation: item.id.clone(), uri, applied_version: applied.text.version },
        );
        let params = ApplyWorkspaceEditParams { label: Some("Accept suggestion".into()), edit };
        self.send(Request::new(request_id, ApplyWorkspaceEdit::METHOD.into(), params));
    }
}

/// The commands offered for an annotation (§6.5.3).
fn offered_commands(item: &Item) -> Vec<(&'static str, &'static str)> {
    match (item.kind(), item.status()) {
        ("suggestion", "open") if item.applicable() => {
            vec![("annox.accept", "Accept suggestion"), ("annox.reject", "Reject suggestion")]
        }
        ("suggestion", "open") => vec![("annox.reject", "Reject suggestion")],
        ("suggestion", "rejected" | "withdrawn") => vec![("annox.reopen", "Reopen suggestion")],
        ("comment", "open") => vec![("annox.resolve", "Resolve thread")],
        ("comment", "resolved") => vec![("annox.reopen", "Reopen thread")],
        _ => vec![],
    }
}

fn author_line(state: &Value) -> String {
    let author = &state["author"];
    let name = author["name"].as_str().or(author["id"].as_str()).unwrap_or("unknown");
    format!("**{name}** · {}", state["created"].as_str().unwrap_or_default())
}

/// A thread as Markdown (§6.5.2): the proposed change first for suggestions,
/// then the root and its replies.
fn thread_markdown(item: &Item, replies: Option<&Vec<Value>>) -> String {
    let mut parts = Vec::new();
    if item.kind() == "suggestion" {
        let r = item.state["edit"]["replacement"].as_str().unwrap_or_default();
        let stale = if item.applicable() { "" } else { " (stale)" };
        parts.push(format!("**Suggestion{stale}:** `{}` → `{r}`", item.target.selectors.quote.exact));
    }
    if item.conflicted() {
        parts.push("⚠ **This annotation has conflicting changes.**".into());
    }
    let mut header = author_line(&item.state);
    if item.resolution.state == State::Relocated {
        header.push_str(" · relocated");
    }
    match item.state["body"].as_str() {
        Some(body) => parts.push(format!("{header}\n\n{body}")),
        None => parts.push(header),
    }
    for reply in replies.into_iter().flatten() {
        parts.push(format!("{}\n\n{}", author_line(reply), reply["body"].as_str().unwrap_or_default()));
    }
    parts.join("\n\n")
}
