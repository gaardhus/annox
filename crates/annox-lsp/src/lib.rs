//! The annox language server (spec §6): standard LSP features for plain
//! clients (§6.5), and `annox/*` extension methods for annox-aware ones
//! (§6.6).

pub mod cli;
pub mod mcp;
mod methods;
pub mod position;
mod views;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use annox_core::anchor::{self, Anchor, Resolution, State};
use annox_core::ops;
use annox_core::storage::{Area, Index, Loaded, Workspace};
use annox_core::suggestion;
use annox_core::text::Text;
use annox_sync::replica::{self, FromReplica, ReplicaConfig, ToReplica};
use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument, DidRenameFiles,
    DidSaveTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{ApplyWorkspaceEdit, CodeActionRequest, ExecuteCommand, HoverRequest, Request as _};
use lsp_types::{
    ApplyWorkspaceEditParams, ApplyWorkspaceEditResponse, CodeAction, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, Command, Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams, ExecuteCommandOptions,
    ExecuteCommandParams, Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, MarkupContent,
    MarkupKind, NumberOrString, PublishDiagnosticsParams, RenameFilesParams, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit,
    Url, WorkspaceEdit,
};
use serde_json::{json, Map, Value};

use crate::position::{Encoding, LineIndex};

/// Spec version implemented (§6.2).
pub const SPEC_VERSION: &str = "0.1";

const COMMANDS: [&str; 4] = ["annox.accept", "annox.reject", "annox.resolve", "annox.reopen"];

// Error codes (§6.6.4).
const UNKNOWN_ANNOTATION: i32 = 1001;
const STALE_SUGGESTION: i32 = 1002;
const INVALID_OPERATION: i32 = 1003;
const NOT_CONFLICTED: i32 = 1004;
const NO_WORKSPACE: i32 = 1005;
const EDIT_NOT_APPLIED: i32 = 1007;
const NEEDS_REVIEW: i32 = 1008;
const OVERLAP: i32 = 1009;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;

type Failure = (i32, String);

/// The identity to write events under when the session doesn't give one:
/// `ANNOX_AUTHOR` and `ANNOX_AUTHOR_NAME`, then the user config file, then git.
pub fn default_author(root: &Path) -> Result<Value, String> {
    let env = |key| std::env::var(key).ok().filter(|v| !v.is_empty());
    resolve_author(env("ANNOX_AUTHOR"), env("ANNOX_AUTHOR_NAME"), root)
}

/// The identity for an explicit `id` and `name`, either of which may be
/// missing. Without an `id`, the user config's author is used, then git's, with
/// `name` replacing its name. An explicit `id` never borrows another
/// identity's name.
pub fn resolve_author(id: Option<String>, name: Option<String>, root: &Path) -> Result<Value, String> {
    if let Some(id) = id {
        return Ok(match name {
            Some(name) => json!({ "id": id, "name": name }),
            None => json!({ "id": id }),
        });
    }
    let mut author = annox_core::config::load()?.author.unwrap_or_else(|| git_author(root));
    if let Some(name) = name {
        author["name"] = json!(name);
    }
    Ok(author)
}

/// Git's identity, if `root` is in a git repository (§2.7).
pub fn git_author(root: &Path) -> Value {
    let git = |key: &str| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
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

fn fail<T>(code: i32, message: impl Into<String>) -> Result<T, Failure> {
    Err((code, message.into()))
}

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
    let encoding =
        Encoding::negotiate(params.capabilities.general.as_ref().and_then(|g| g.position_encodings.as_deref()));
    let annox_client = params.capabilities.experimental.as_ref().is_some_and(|e| e.get("annox").is_some());
    let dynamic_watch = params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.did_change_watched_files.as_ref())
        .and_then(|d| d.dynamic_registration)
        .unwrap_or(false);
    let mut roots: Vec<PathBuf> =
        params.workspace_folders.iter().flatten().filter_map(|f| f.uri.to_file_path().ok()).collect();
    #[allow(deprecated)]
    if let Some(root) = params.root_uri.as_ref().and_then(|u| u.to_file_path().ok()) {
        roots.push(root);
    }
    let options = params.initialization_options.as_ref().and_then(|o| o.get("annox")).cloned().unwrap_or_default();
    connection.initialize_finish(
        id,
        json!({
            "capabilities": capabilities(encoding),
            "serverInfo": { "name": "annox", "version": env!("CARGO_PKG_VERSION") },
        }),
    )?;
    let (sync_tx, sync_rx) = crossbeam_channel::unbounded();
    let mut server = Server {
        connection,
        encoding,
        annox_client,
        author: options.get("author").cloned(),
        diagnostics: options.get("diagnostics").and_then(Value::as_bool).unwrap_or(true),
        open: HashMap::new(),
        next_id: 0,
        pending: HashMap::new(),
        pending_init: HashMap::new(),
        roots,
        poll_interval: if dynamic_watch { BACKSTOP_INTERVAL } else { POLL_INTERVAL },
        fingerprints: HashMap::new(),
        replicas: HashMap::new(),
        sync_tx,
        sync_rx,
        cache: RefCell::new(Cache::default()),
        dirty: HashMap::new(),
    };
    if dynamic_watch {
        server.register_watcher();
    }
    if let Err(e) = annox_core::config::load() {
        server.warn(format!("annox: ignoring the config file, {e}"));
    }
    server.main_loop()
}

pub(crate) struct OpenDoc {
    text: Text,
    crlf: bool,
    /// What the client asked `annox/annotations` to include (§6.6.3).
    include_closed: bool,
    include_deleted: bool,
}

/// Work that waits for the client's `workspace/applyEdit` response.
pub(crate) enum Work {
    /// Accept one suggestion (§6.6.2).
    Accept { annotation: String },
    /// Bulk accept: `results` in request order, `chosen` are applied.
    AcceptAll { chosen: Vec<String>, results: Vec<Value> },
    /// Resolve a status conflict to `status`, reverting the edit (§4.3.3).
    Revert { annotation: String, status: String },
}

pub(crate) struct Pending {
    command: RequestId,
    uri: Url,
    applied_version: String,
    /// New anchors for the comments the edit changes, written once it is
    /// applied (§4.3.5).
    carried: Vec<(String, Anchor)>,
    work: Work,
    /// Reply with the AnnotationView (extension method) or null (command).
    respond_view: bool,
}

/// A create request waiting for the user to confirm creating a workspace
/// (§6.2).
pub(crate) struct PendingInit {
    command: RequestId,
    params: Value,
    root: PathBuf,
}

/// How often storage is checked when the client can't watch files (§6.4).
const POLL_INTERVAL: Duration = Duration::from_millis(1000);

/// How often storage is checked when the client watches files, for changes
/// its watcher misses. VS Code's misses everything in folders created
/// together with their parent, such as a document's first annotation folder.
const BACKSTOP_INTERVAL: Duration = Duration::from_secs(5);

const CREATE_WORKSPACE: &str = "Create workspace";

pub(crate) struct Server<'a> {
    connection: &'a Connection,
    encoding: Encoding,
    annox_client: bool,
    author: Option<Value>,
    diagnostics: bool,
    open: HashMap<Url, OpenDoc>,
    next_id: i32,
    pending: HashMap<RequestId, Pending>,
    pending_init: HashMap<RequestId, PendingInit>,
    /// Workspace folders from `initialize`, for placing new workspaces.
    roots: Vec<PathBuf>,
    /// How often to check `.annox/` for changes the client didn't report (§6.4).
    poll_interval: Duration,
    fingerprints: HashMap<PathBuf, Vec<(String, u64, std::time::SystemTime)>>,
    /// Sync replica per workspace root (§7), or `None` if it doesn't sync.
    replicas: HashMap<PathBuf, Option<Sender<ToReplica>>>,
    sync_tx: Sender<FromReplica>,
    sync_rx: Receiver<FromReplica>,
    cache: RefCell<Cache>,
    /// Documents edited since their last refresh, with when to refresh them.
    dirty: HashMap<Url, Instant>,
}

/// Workspace indexes and document analyses, reused until storage changes.
#[derive(Default)]
pub(crate) struct Cache {
    generation: u64,
    indexes: HashMap<PathBuf, Rc<Index>>,
    /// Per document: the generation and text version it was built from.
    analyses: HashMap<Url, (u64, String, Rc<Analysis>)>,
    /// Per document: loaded events and derived states, which depend only on
    /// storage, so text edits don't re-derive them.
    derived: HashMap<Url, Derived>,
}

/// Loaded events and derived annotation states of one document.
type Derived = Rc<(Loaded, BTreeMap<String, Value>)>;

/// How long typing must pause before annotations are re-resolved (§6.4).
const DEBOUNCE: Duration = Duration::from_millis(150);

/// A root annotation of a document, resolved against its current text.
pub(crate) struct Item {
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

    fn deleted(&self) -> bool {
        self.state["deleted"] != json!(false)
    }

    fn is_open(&self) -> bool {
        self.status() == "open" && !self.deleted()
    }

    fn applicable(&self) -> bool {
        self.kind() == "suggestion" && suggestion::is_applicable(&self.resolution)
    }

    fn conflicted(&self) -> bool {
        self.state["conflicts"].as_object().is_some_and(|c| !c.is_empty())
    }
}

/// Everything known about one document.
pub(crate) struct Analysis {
    ws: Workspace,
    rel: String,
    index: Rc<Index>,
    loaded: Loaded,
    text: Text,
    items: Vec<Item>,
    /// Reply states by parent id, in display order (§2.5.5).
    replies: BTreeMap<String, Vec<Value>>,
}

impl Analysis {
    fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    fn reply(&self, id: &str) -> Option<&Value> {
        self.replies.values().flatten().find(|r| r["id"] == id)
    }

    fn area(&self, id: &str) -> Area {
        self.index.events.get(id).map_or(Area::Shared, |(_, loc)| loc.area)
    }
}

impl Server<'_> {
    fn main_loop(&mut self) -> anyhow::Result<()> {
        let ticker = crossbeam_channel::tick(self.poll_interval);
        let sync_rx = self.sync_rx.clone();
        loop {
            let debounce = match self.dirty.values().min() {
                Some(due) => crossbeam_channel::after(due.saturating_duration_since(Instant::now())),
                None => crossbeam_channel::never(),
            };
            crossbeam_channel::select! {
                recv(debounce) -> _ => self.flush_dirty(),
                recv(self.connection.receiver) -> msg => match msg {
                    Ok(Message::Request(req)) => {
                        if self.connection.handle_shutdown(&req)? {
                            return Ok(());
                        }
                        self.on_request(req);
                    }
                    Ok(Message::Notification(n)) => self.on_notification(n),
                    Ok(Message::Response(resp)) => self.on_response(resp),
                    Err(_) => return Ok(()),
                },
                recv(ticker) -> _ => self.poll_storage(),
                recv(sync_rx) -> event => if let Ok(event) = event { self.on_sync(event) },
            }
        }
    }

    /// Asks the client to report changes under `.annox/` (§6.4).
    fn register_watcher(&mut self) {
        let params = json!({ "registrations": [{
            "id": "annox-storage",
            "method": DidChangeWatchedFiles::METHOD,
            "registerOptions": { "watchers": [{ "globPattern": "**/.annox/**" }] },
        }] });
        self.next_id += 1;
        let id = RequestId::from(format!("annox-register-{}", self.next_id));
        self.send(Request::new(id, "client/registerCapability".into(), params));
    }

    /// Checks `.annox/` of every open document's workspace for changes made by
    /// others that the client didn't report (§6.4).
    fn poll_storage(&mut self) {
        if self.scan_storage() {
            self.invalidate();
            self.refresh_all();
        }
    }

    /// Records the state of `.annox/` in every open document's workspace, and
    /// whether it changed since the last scan.
    fn scan_storage(&mut self) -> bool {
        let roots: std::collections::BTreeSet<PathBuf> = self
            .open
            .keys()
            .filter_map(|uri| uri.to_file_path().ok())
            .filter_map(|p| Workspace::find(&p))
            .map(|ws| ws.root)
            .collect();
        let mut changed = false;
        for root in roots {
            let print = fingerprint(&root);
            if self.fingerprints.get(&root) != Some(&print) {
                changed |= self.fingerprints.contains_key(&root);
                self.fingerprints.insert(root, print);
            }
        }
        changed
    }

    /// Refreshes documents whose typing pause has elapsed.
    fn flush_dirty(&mut self) {
        let now = Instant::now();
        let due: Vec<Url> = self.dirty.iter().filter(|(_, t)| **t <= now).map(|(u, _)| u.clone()).collect();
        for uri in due {
            self.dirty.remove(&uri);
            self.refresh(&uri);
        }
    }

    /// Forgets cached indexes and analyses after storage changed.
    pub(crate) fn invalidate(&self) {
        let mut cache = self.cache.borrow_mut();
        cache.generation += 1;
        cache.indexes.clear();
        cache.analyses.clear();
        cache.derived.clear();
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
        let params = req.params;
        match req.method.as_str() {
            HoverRequest::METHOD => {
                let result = parse::<HoverParams>(params).map(|p| json!(self.hover(p)));
                self.reply(id, result);
            }
            CodeActionRequest::METHOD => {
                let result = parse::<CodeActionParams>(params).map(|p| json!(self.code_actions(p)));
                self.reply(id, result);
            }
            ExecuteCommand::METHOD => match parse::<ExecuteCommandParams>(params) {
                Ok(p) => self.execute(id, p),
                Err(e) => self.reply(id, Err(e)),
            },
            method if method.starts_with("annox/") => self.extension(id, method.to_owned(), params),
            _ => self.reply(id, fail(-32601, format!("unhandled method {}", req.method))),
        }
    }

    fn on_notification(&mut self, n: Notification) {
        match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(n.params) {
                    let uri = p.text_document.uri;
                    self.set_text(uri.clone(), &p.text_document.text);
                    self.ensure_replica(&uri);
                    // Polls compare against storage as it was read here.
                    if self.scan_storage() {
                        self.invalidate();
                        self.refresh_all();
                    } else {
                        self.refresh(&uri);
                    }
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(n.params) {
                    if let Some(change) = p.content_changes.into_iter().last() {
                        let uri = p.text_document.uri;
                        self.set_text(uri.clone(), &change.text);
                        // Re-resolve once typing pauses (§6.4).
                        self.dirty.insert(uri, Instant::now() + DEBOUNCE);
                    }
                }
            }
            DidSaveTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidSaveTextDocumentParams>(n.params) {
                    // Saving is a natural point to pick up outside changes too.
                    self.invalidate();
                    self.refresh(&p.text_document.uri);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(n.params) {
                    self.open.remove(&p.text_document.uri);
                    if self.diagnostics {
                        self.send_diagnostics(p.text_document.uri, vec![]);
                    }
                }
            }
            DidChangeWatchedFiles::METHOD => {
                // So that the next poll doesn't reload for the same change.
                self.scan_storage();
                self.invalidate();
                self.refresh_all();
            }
            DidRenameFiles::METHOD => {
                if let Ok(p) = serde_json::from_value::<RenameFilesParams>(n.params) {
                    self.on_rename(p);
                }
            }
            "annox/setPresence" => self.set_presence(&n.params),
            _ => {}
        }
    }

    fn on_response(&mut self, resp: Response) {
        if let Some(init) = self.pending_init.remove(&resp.id) {
            return self.finish_init(init, resp);
        }
        let Some(pending) = self.pending.remove(&resp.id) else { return };
        let applied = resp
            .response_result
            .ok()
            .and_then(|v| serde_json::from_value::<ApplyWorkspaceEditResponse>(v).ok())
            .is_some_and(|r| r.applied);
        let result = self.finish(&pending, applied);
        self.reply(pending.command, result);
        self.refresh_all();
    }

    /// Records the outcome of an applied (or refused) edit (§6.6.2 step 3).
    fn finish(&self, pending: &Pending, applied: bool) -> Result<Value, Failure> {
        let status_fields = |status: &str| {
            let mut fields = Map::from_iter([("status".into(), json!(status))]);
            if status == "accepted" {
                fields.insert("appliedVersion".into(), json!(pending.applied_version));
            }
            fields
        };
        let a = self.analyze(&pending.uri).ok_or((NO_WORKSPACE, "no annox workspace".to_owned()))?;
        if applied {
            for (id, target) in &pending.carried {
                let target = serde_json::to_value(target).map_err(|e| (INTERNAL_ERROR, e.to_string()))?;
                self.write_event(&a, id, "reanchor", Map::from_iter([("target".into(), target)]))?;
            }
        }
        match &pending.work {
            Work::AcceptAll { chosen, results } => {
                let mut results = results.clone();
                for r in results.iter_mut() {
                    let id = r["annotation"].as_str().unwrap_or_default().to_owned();
                    if !chosen.contains(&id) {
                        continue;
                    }
                    *r = if !applied {
                        views::result_error(&id, EDIT_NOT_APPLIED, "the client did not apply the edit")
                    } else {
                        match self.write_event(&a, &id, "status", status_fields("accepted")) {
                            Ok(()) => json!({ "annotation": id, "accepted": true }),
                            Err((code, msg)) => views::result_error(&id, code, &msg),
                        }
                    };
                }
                Ok(json!({ "results": results }))
            }
            Work::Accept { annotation } | Work::Revert { annotation, .. } => {
                if !applied {
                    return fail(EDIT_NOT_APPLIED, "the client did not apply the edit");
                }
                let status = match &pending.work {
                    Work::Revert { status, .. } => status.as_str(),
                    _ => "accepted",
                };
                self.write_event(&a, annotation, "status", status_fields(status))?;
                if pending.respond_view {
                    self.view_of(&pending.uri, annotation)
                } else {
                    Ok(Value::Null)
                }
            }
        }
    }

    fn set_text(&mut self, uri: Url, raw: &str) {
        let (include_closed, include_deleted) =
            self.open.get(&uri).map_or((false, false), |d| (d.include_closed, d.include_deleted));
        let doc = OpenDoc { text: Text::from_raw(raw), crlf: raw.contains("\r\n"), include_closed, include_deleted };
        self.open.insert(uri, doc);
    }

    /// Loads, replays, and resolves the annotations of `uri` (§5.7.1, §3.7).
    /// Results are cached until storage or the document's text changes.
    fn analyze(&self, uri: &Url) -> Option<Rc<Analysis>> {
        let path: PathBuf = uri.to_file_path().ok()?;
        let ws = Workspace::find(&path)?;
        let rel = ws.relative(&path)?;
        let text = match self.open.get(uri) {
            Some(doc) => doc.text.clone(),
            None => Text::from_raw(&std::fs::read_to_string(&path).unwrap_or_default()),
        };
        let generation = self.cache.borrow().generation;
        if let Some((g, version, a)) = self.cache.borrow().analyses.get(uri) {
            if *g == generation && *version == text.version {
                return Some(a.clone());
            }
        }
        let cached = self.cache.borrow().indexes.get(&ws.root).cloned();
        let index = cached.unwrap_or_else(|| {
            let index = Rc::new(Index::read(&ws));
            self.cache.borrow_mut().indexes.insert(ws.root.clone(), index.clone());
            index
        });
        let cached = self.cache.borrow().derived.get(uri).cloned();
        let derived = cached.unwrap_or_else(|| {
            let loaded = index.load(&rel);
            let states = loaded.derive();
            let derived = Rc::new((loaded, states));
            self.cache.borrow_mut().derived.insert(uri.clone(), derived.clone());
            derived
        });
        let (loaded, states) = &*derived;
        let loaded = loaded.clone();
        let mut items = Vec::new();
        let mut replies: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for (id, state) in states.clone() {
            if state["kind"] == "reply" {
                let parent = state["parent"].as_str().unwrap_or_default().to_owned();
                replies.entry(parent).or_default().push(state);
                continue;
            }
            let Ok(target) = serde_json::from_value::<Anchor>(state["target"].clone()) else { continue };
            let resolution = anchor::resolve(&text, &target);
            items.push(Item { id, state, target, resolution });
        }
        let version = text.version.clone();
        let analysis = Rc::new(Analysis { ws, rel, index, loaded, text, items, replies });
        self.cache.borrow_mut().analyses.insert(uri.clone(), (generation, version, analysis.clone()));
        Some(analysis)
    }

    /// Finds the open document that holds annotation `id` (a root or reply).
    fn find(&self, id: &str) -> Option<(Url, Rc<Analysis>)> {
        self.open.keys().find_map(|uri| {
            let a = self.analyze(uri)?;
            (a.item(id).is_some() || a.reply(id).is_some()).then(|| (uri.clone(), a))
        })
    }

    fn author(&self, ws: &Workspace) -> Value {
        // A broken config file was reported at startup; write as git's identity meanwhile.
        self.author.clone().unwrap_or_else(|| default_author(&ws.root).unwrap_or_else(|_| git_author(&ws.root)))
    }

    fn write_event(
        &self,
        a: &Analysis,
        annotation: &str,
        kind: &str,
        fields: Map<String, Value>,
    ) -> Result<(), Failure> {
        let result = ops::append_event(&a.ws, &a.index, annotation, kind, fields, &self.author(&a.ws));
        self.invalidate();
        result.map(|_| ()).map_err(|e| (INTERNAL_ERROR, e.to_string()))
    }

    /// The AnnotationView of `id` after reloading `uri` (§6.6.1).
    fn view_of(&self, uri: &Url, id: &str) -> Result<Value, Failure> {
        let a = self.analyze(uri).ok_or((NO_WORKSPACE, "no annox workspace".to_owned()))?;
        views::view_by_id(&a, id, self.encoding).ok_or((UNKNOWN_ANNOTATION, format!("unknown annotation {id}")))
    }

    /// Sends a `WorkspaceEdit` replacing `[start, end)` of `uri` with `new_text`,
    /// and parks `work` until the client answers.
    fn apply_edit(
        &mut self,
        command: RequestId,
        uri: Url,
        edits: Vec<(usize, usize, String)>,
        work: Work,
        respond_view: bool,
    ) {
        let Some(a) = self.analyze(&uri) else {
            return self.reply(command, fail(NO_WORKSPACE, "no annox workspace"));
        };
        let crlf = self.open.get(&uri).is_some_and(|d| d.crlf);
        let lines = LineIndex::new(&a.text, self.encoding);
        // Apply back to front to compute the resulting version.
        let mut sorted = edits.clone();
        sorted.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
        let mut result = a.text.clone();
        for (s, e, t) in &sorted {
            result = result.splice(*s, *e, t);
        }
        let text_edits = edits
            .into_iter()
            .map(|(s, e, t)| TextEdit {
                range: lines.range(s, e),
                new_text: if crlf { t.replace('\n', "\r\n") } else { t },
            })
            .collect();
        let edit = WorkspaceEdit { changes: Some(HashMap::from([(uri.clone(), text_edits)])), ..Default::default() };
        self.next_id += 1;
        let request_id = RequestId::from(format!("annox-apply-{}", self.next_id));
        let mut spans: Vec<(usize, usize, usize)> =
            sorted.iter().map(|(s, e, t)| (*s, *e, t.chars().count())).collect();
        spans.reverse();
        let carried =
            suggestion::carry_comments(&a.text, &result, &a.rel, &spans, a.items.iter().map(|i| (&i.id, &i.state)));
        let pending = Pending { command, uri, applied_version: result.version, carried, work, respond_view };
        self.pending.insert(request_id.clone(), pending);
        let label = match &self.pending[&request_id].work {
            Work::Accept { .. } => "annox: accept suggestion",
            Work::AcceptAll { .. } => "annox: accept suggestions",
            Work::Revert { .. } => "annox: revert suggestion",
        };
        let params = ApplyWorkspaceEditParams { label: Some(label.into()), edit };
        self.send(Request::new(request_id, ApplyWorkspaceEdit::METHOD.into(), params));
    }

    // Pushing state: diagnostics (§6.5.1) and annox/didChangeAnnotations (§6.6.3).

    fn refresh_all(&self) {
        for uri in self.open.keys() {
            self.refresh(uri);
        }
        // Anything new on disk may need pushing to a hub (§7.6).
        for tx in self.replicas.values().flatten() {
            let _ = tx.send(ToReplica::Scan);
        }
    }

    // Sync (§7).

    /// Starts a replica for the workspace of `uri` if it names a hub and the
    /// user has credentials for it (§6.4, §7.2).
    pub(crate) fn ensure_replica(&mut self, uri: &Url) {
        let Some(ws) = uri.to_file_path().ok().and_then(|p| Workspace::find(&p)) else { return };
        if self.replicas.contains_key(&ws.root) {
            return;
        }
        let marker = std::fs::read_to_string(ws.root.join(".annox/annox.json")).unwrap_or_default();
        let url =
            serde_json::from_str::<Value>(&marker).ok().and_then(|m| m["sync"]["url"].as_str().map(str::to_owned));
        let replica = url.and_then(|url| {
            let Some(credential) = annox_sync::credentials::lookup(&url) else {
                self.warn(format!("annox: {url} has no entry in the credentials file; not syncing"));
                return None;
            };
            let config =
                ReplicaConfig { root: ws.root.clone(), url, token: credential.token, author: self.author(&ws) };
            Some(replica::spawn(config, self.sync_tx.clone()))
        });
        self.replicas.insert(ws.root, replica);
    }

    fn log(&self, message: String) {
        self.send(Notification::new("window/logMessage".into(), json!({ "type": 3, "message": message })));
    }

    fn warn(&self, message: String) {
        self.send(Notification::new("window/logMessage".into(), json!({ "type": 2, "message": message })));
    }

    fn on_sync(&mut self, event: FromReplica) {
        match event {
            FromReplica::Changed { .. } => {
                self.invalidate();
                self.refresh_all();
            }
            FromReplica::Status { message, .. } => self.log(format!("annox sync: {message}")),
            FromReplica::Presence { root, peers } => {
                if !self.annox_client {
                    return;
                }
                let ws = Workspace { root };
                let index = Index::read(&ws);
                let peers: Vec<Value> = peers.iter().map(|p| self.presence_view(&ws, &index, p)).collect();
                self.send(Notification::new("annox/didChangePresence".into(), json!({ "peers": peers })));
            }
        }
    }

    /// A hub peer (§7.8.1) with its document as a URI and its range as an LSP
    /// `Range`, mapped onto this copy of the document (§7.8.2).
    fn presence_view(&self, ws: &Workspace, index: &Index, peer: &Value) -> Value {
        let mut view = json!({ "author": peer["author"] });
        let Some(doc) = peer["document"].as_str() else { return view };
        let Some(path) = index.documents.get(index.canonical(doc)).and_then(|d| d.path.clone()) else { return view };
        let Ok(uri) = Url::from_file_path(ws.root.join(&path)) else { return view };
        view["textDocument"] = json!({ "uri": uri });
        let text = match self.open.get(&uri) {
            Some(d) => d.text.clone(),
            None => Text::from_raw(&std::fs::read_to_string(ws.root.join(&path)).unwrap_or_default()),
        };
        let (Some(start), Some(end)) = (peer["range"]["start"].as_u64(), peer["range"]["end"].as_u64()) else {
            return view;
        };
        let (start, end) = (start as usize, end as usize);
        let range = match serde_json::from_value(peer["quote"].clone()) {
            // Resolve the cursor as an anchor, so it stays on the same text
            // when the peer's copy differs from this one.
            Ok(quote) => {
                let target = Anchor {
                    path: path.clone(),
                    version: peer["version"].as_str().unwrap_or_default().to_owned(),
                    selectors: anchor::Selectors { position: anchor::PositionSelector { start, end }, quote },
                };
                anchor::resolve(&text, &target).range
            }
            // A peer without a quote: clamp its offsets to this copy.
            Err(_) => Some((start.min(end).min(text.len()), end.min(text.len()))),
        };
        if let Some((start, end)) = range {
            view["range"] = json!(LineIndex::new(&text, self.encoding).range(start, end));
        }
        view
    }

    /// Forwards the user's document and cursor to the hub (§7.8).
    fn set_presence(&self, params: &Value) {
        let uri = params["textDocument"]["uri"].as_str().and_then(|u| Url::parse(u).ok());
        let Some(a) = uri.as_ref().and_then(|u| self.analyze(u)) else {
            for tx in self.replicas.values().flatten() {
                let _ = tx.send(ToReplica::Presence(json!({})));
            }
            return;
        };
        let mut presence = json!({ "version": a.text.version });
        if let Some(doc) = a.loaded.at.first() {
            presence["document"] = json!(doc);
        }
        if let Ok(range) = serde_json::from_value::<lsp_types::Range>(params["selection"].clone()) {
            let lines = LineIndex::new(&a.text, self.encoding);
            let (from, to) = (lines.offset(range.start), lines.offset(range.end));
            let (start, end) = (from.min(to), from.max(to));
            presence["range"] = json!({ "start": start, "end": end });
            presence["quote"] = json!(anchor::create(&a.text, start, end, "").selectors.quote);
        }
        if let Some(Some(tx)) = self.replicas.get(&a.ws.root) {
            let _ = tx.send(ToReplica::Presence(presence));
        }
    }

    fn refresh(&self, uri: &Url) {
        if !self.diagnostics && !self.annox_client {
            return;
        }
        let a = self.analyze(uri);
        if self.diagnostics {
            let diagnostics = a.as_ref().map(|a| self.diagnostics_for(a)).unwrap_or_default();
            self.send_diagnostics(uri.clone(), diagnostics);
        }
        if self.annox_client {
            if let (Some(a), Some(doc)) = (a, self.open.get(uri)) {
                let params = json!({
                    "textDocument": { "uri": uri },
                    "annotations": views::views(&a, self.encoding, doc.include_closed, doc.include_deleted),
                    "document": views::document_info(&a),
                });
                self.send(Notification::new("annox/didChangeAnnotations".into(), params));
            }
        }
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
            .filter(|i| i.is_open())
            .filter(|i| i.resolution.range.is_some_and(|(s, e)| s <= offset && offset <= e))
            .collect();
        let first = hits.first()?.resolution.range?;
        let sections: Vec<String> = hits.iter().map(|i| thread_markdown(i, a.replies.get(&i.id))).collect();
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: sections.join("\n\n---\n\n"),
            }),
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
            if s > to || e < from || item.deleted() {
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
            return self.reply(id, fail(INVALID_PARAMS, "missing annotation argument"));
        };
        let Some((uri, a)) = self.find(&annotation) else {
            return self.reply(id, fail(UNKNOWN_ANNOTATION, format!("unknown annotation {annotation}")));
        };
        let Some(item) = a.item(&annotation) else {
            return self.reply(id, fail(INVALID_OPERATION, "commands apply to comments and suggestions"));
        };
        if !offered_commands(item).iter().any(|(c, _)| *c == p.command) {
            return self.reply(id, fail(INVALID_OPERATION, format!("{} does not apply to this annotation", p.command)));
        }
        let status = match p.command.as_str() {
            "annox.accept" => return self.accept(id, uri, &a, &annotation, false),
            "annox.reject" => "rejected",
            "annox.resolve" => "resolved",
            _ => "open",
        };
        let result = self.write_event(&a, &annotation, "status", Map::from_iter([("status".into(), json!(status))]));
        self.reply(id, result.map(|_| Value::Null));
        self.refresh_all();
    }

    /// Applies a suggestion through `workspace/applyEdit`; the status event
    /// is written when the client confirms (§6.6.2).
    fn accept(&mut self, command: RequestId, uri: Url, a: &Analysis, annotation: &str, respond_view: bool) {
        let Some(item) = a.item(annotation).filter(|i| i.kind() == "suggestion" && i.is_open()) else {
            return self.reply(command, fail(INVALID_OPERATION, "only open suggestions can be accepted"));
        };
        let replacement = item.state["edit"]["replacement"].as_str().unwrap_or_default().to_owned();
        match suggestion::apply(&a.text, &item.target, &replacement) {
            Ok(applied) => {
                let work = Work::Accept { annotation: annotation.to_owned() };
                self.apply_edit(command, uri, vec![(applied.start, applied.end, replacement)], work, respond_view);
            }
            Err(_) => self.reply(command, fail(STALE_SUGGESTION, "the suggestion is stale")),
        }
    }

    fn on_rename(&self, p: RenameFilesParams) {
        for file in p.files {
            let (Ok(old), Ok(new)) = (Url::parse(&file.old_uri), Url::parse(&file.new_uri)) else { continue };
            let (Ok(old_path), Ok(new_path)) = (old.to_file_path(), new.to_file_path()) else { continue };
            let Some(ws) = Workspace::find(&new_path) else { continue };
            let (Some(from), Some(to)) = (ws.relative(&old_path), ws.relative(&new_path)) else { continue };
            let _ = ops::move_document(&ws, &Index::read(&ws), &from, &to, &self.author(&ws));
            self.invalidate();
        }
        self.refresh_all();
    }
}

/// Names, sizes, and modification times of the event files under `root`.
fn fingerprint(root: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    let mut stack = vec![root.join(".annox/docs"), root.join(".annox/local/docs"), root.join(".annox/synced/docs")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                out.push((entry.path().display().to_string(), meta.len(), modified));
            }
        }
    }
    out.sort();
    out
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, Failure> {
    serde_json::from_value(params).map_err(|e| (INVALID_PARAMS, e.to_string()))
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

/// A change as a fenced `diff` block, so editors color the old text red and
/// the new text green.
fn diff_block(old: &str, new: &str) -> String {
    let longest = [old, new].iter().flat_map(|t| t.split(|c| c != '`')).map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    let mut out = format!("{fence}diff\n");
    for (sign, text) in [('-', old), ('+', new)] {
        if !text.is_empty() {
            for line in text.split('\n') {
                out.push_str(&format!("{sign} {line}\n"));
            }
        }
    }
    out + &fence
}

/// A thread as Markdown (§6.5.2): the proposed change first for suggestions,
/// then the root and its replies.
fn thread_markdown(item: &Item, replies: Option<&Vec<Value>>) -> String {
    let mut parts = Vec::new();
    if item.kind() == "suggestion" {
        let r = item.state["edit"]["replacement"].as_str().unwrap_or_default();
        let stale = if item.applicable() { "" } else { " (stale)" };
        parts.push(format!("**Suggestion{stale}:**\n{}", diff_block(&item.target.selectors.quote.exact, r)));
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
    for reply in replies.into_iter().flatten().filter(|r| r["deleted"] == json!(false)) {
        // A thematic break renders as a full-width rule between messages.
        parts.push(format!("---\n\n{}\n\n{}", author_line(reply), reply["body"].as_str().unwrap_or_default()));
    }
    parts.join("\n\n")
}
