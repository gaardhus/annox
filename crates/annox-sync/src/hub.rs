//! The sync hub (§7.7): stores every accepted event per workspace, numbers
//! them in arrival order, and relays new events and presence to connected
//! replicas. It never interprets events.
//!
//! Workspaces are addressed as `ws://host/w/<name>`. Each is persisted as
//! `<data>/<name>.jsonl`: a header line with the hub id, then one item per
//! line.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, Sender};
use serde_json::{json, Value};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::{self, HeaderValue};

use crate::rpc::{Error, Peer, RpcError, Timeout};
use crate::{is_well_formed, FORMAT, INVALID_ITEM, SUBPROTOCOL, UNKNOWN_HUB, UNSUPPORTED_FORMAT};

/// Most items returned by one `pull`.
const PULL_LIMIT: usize = 1000;

#[derive(Clone, Debug)]
pub struct HubConfig {
    /// Directory holding one log file per workspace.
    pub data: PathBuf,
    /// If set, connections must send `Authorization: Bearer <token>` (§7.2).
    pub token: Option<String>,
}

struct Log {
    hub_id: String,
    /// Items in `seq` order; `seq` is the index plus one.
    items: Vec<Value>,
    by_id: HashMap<String, u64>,
    file: File,
    subscribers: HashMap<u64, Sender<Value>>,
    presence: HashMap<u64, Value>,
}

impl Log {
    fn open(path: &Path) -> io::Result<Log> {
        let mut items = Vec::new();
        let mut hub_id = None;
        if path.exists() {
            for line in BufReader::new(File::open(path)?).lines() {
                let value: Value = serde_json::from_str(&line?).map_err(io::Error::other)?;
                if let Some(id) = value["hubId"].as_str() {
                    hub_id = Some(id.to_owned());
                } else {
                    items.push(value);
                }
            }
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let hub_id = match hub_id {
            Some(id) => id,
            None => {
                let id = uuid::Uuid::now_v7().to_string();
                writeln!(file, "{}", json!({ "hubId": id }))?;
                id
            }
        };
        let by_id = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((item["event"]["id"].as_str()?.to_owned(), i as u64 + 1)))
            .collect();
        Ok(Log { hub_id, items, by_id, file, subscribers: HashMap::new(), presence: HashMap::new() })
    }

    fn cursor(&self) -> u64 {
        self.items.len() as u64
    }

    /// Stores an item, or returns the `seq` of the identical stored event.
    fn accept(&mut self, item: &Value) -> Result<(u64, bool), RpcError> {
        if !is_well_formed(item) {
            return Err((INVALID_ITEM, "not a well-formed event".into()));
        }
        let id = item["event"]["id"].as_str().unwrap_or_default();
        if let Some(&seq) = self.by_id.get(id) {
            let stored = &self.items[seq as usize - 1];
            return if stored["event"] == item["event"] && stored["document"] == item["document"] {
                Ok((seq, false))
            } else {
                Err((INVALID_ITEM, "an event with this id but different content exists".into()))
            };
        }
        let seq = self.cursor() + 1;
        let stored = json!({ "seq": seq, "document": item["document"], "event": item["event"] });
        writeln!(self.file, "{stored}").map_err(|e| (-32603, e.to_string()))?;
        self.file.flush().map_err(|e| (-32603, e.to_string()))?;
        self.by_id.insert(id.to_owned(), seq);
        self.items.push(stored);
        Ok((seq, true))
    }

    fn broadcast_presence(&self) {
        for (&conn, tx) in &self.subscribers {
            let peers: Vec<&Value> = self.presence.iter().filter(|(c, _)| **c != conn).map(|(_, p)| p).collect();
            let _ = tx.send(notification("annoxSync/didChangePresence", json!({ "peers": peers })));
        }
    }
}

fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

struct Shared {
    config: HubConfig,
    logs: Mutex<HashMap<String, Arc<Mutex<Log>>>>,
    next_connection: AtomicU64,
}

impl Shared {
    fn log(&self, name: &str) -> io::Result<Arc<Mutex<Log>>> {
        let mut logs = self.logs.lock().unwrap();
        if let Some(log) = logs.get(name) {
            return Ok(log.clone());
        }
        fs::create_dir_all(&self.config.data)?;
        let log = Arc::new(Mutex::new(Log::open(&self.config.data.join(format!("{name}.jsonl")))?));
        logs.insert(name.to_owned(), log.clone());
        Ok(log)
    }
}

fn valid_workspace_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Serves the hub on `listener` until the process exits.
pub fn serve(listener: TcpListener, config: HubConfig) -> io::Result<()> {
    let shared = Arc::new(Shared { config, logs: Mutex::new(HashMap::new()), next_connection: AtomicU64::new(1) });
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let shared = shared.clone();
        std::thread::spawn(move || {
            let _ = handle(stream, &shared);
        });
    }
    Ok(())
}

fn reject(status: u16, message: &str) -> ErrorResponse {
    http::Response::builder().status(status).body(Some(message.to_owned())).unwrap()
}

// The handshake callback's error type is dictated by tungstenite.
#[allow(clippy::result_large_err)]
fn handle(stream: TcpStream, shared: &Shared) -> Result<(), Error> {
    let mut workspace = None;
    let token = shared.config.token.clone();
    let callback = |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
        if let Some(expected) = &token {
            let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok());
            if auth != Some(format!("Bearer {expected}").as_str()) {
                return Err(reject(401, "unauthorized"));
            }
        }
        match req.uri().path().strip_prefix("/w/") {
            Some(name) if valid_workspace_name(name) => workspace = Some(name.to_owned()),
            _ => return Err(reject(404, "no such workspace; use /w/<name>")),
        }
        let wants_protocol = req
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|p| p.trim() == SUBPROTOCOL));
        if wants_protocol {
            resp.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_static(SUBPROTOCOL));
        }
        Ok(resp)
    };
    let ws = tungstenite::accept_hdr(stream, callback).map_err(|e| Error::Io(e.to_string()))?;
    ws.get_ref().set_timeout(Some(Duration::from_millis(25))).map_err(|e| Error::Io(e.to_string()))?;
    let log = shared.log(&workspace.unwrap_or_default()).map_err(|e| Error::Io(e.to_string()))?;
    let connection = shared.next_connection.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = unbounded();
    let result = session(Peer::new(ws), &log, connection, tx, rx);
    let mut log = log.lock().unwrap();
    log.subscribers.remove(&connection);
    if log.presence.remove(&connection).is_some() {
        log.broadcast_presence();
    }
    result
}

fn session(
    mut peer: Peer<TcpStream>,
    log: &Mutex<Log>,
    connection: u64,
    tx: Sender<Value>,
    rx: Receiver<Value>,
) -> Result<(), Error> {
    let mut greeted = false;
    loop {
        while let Ok(msg) = rx.try_recv() {
            peer.send(&msg)?;
        }
        let Some(msg) = peer.recv()? else { continue };
        let method = msg["method"].as_str().unwrap_or_default();
        let params = &msg["params"];
        let Some(id) = msg.get("id").cloned() else {
            // Notifications: presence (§7.8).
            if method == "annoxSync/presence" && greeted {
                let mut log = log.lock().unwrap();
                let mut peer_state = params.clone();
                peer_state["connection"] = json!(connection.to_string());
                log.presence.insert(connection, peer_state);
                log.broadcast_presence();
            }
            continue;
        };
        let result = match method {
            "annoxSync/hello" => {
                if params["format"].as_u64() != Some(FORMAT) {
                    Err((UNSUPPORTED_FORMAT, format!("storage format {} is not supported", params["format"])))
                } else {
                    greeted = true;
                    let mut log = log.lock().unwrap();
                    if params["subscribe"].as_bool().unwrap_or(false) {
                        log.subscribers.insert(connection, tx.clone());
                        let peers: Vec<&Value> = log.presence.values().collect();
                        let _ = tx.send(notification("annoxSync/didChangePresence", json!({ "peers": peers })));
                    }
                    Ok(json!({ "format": FORMAT, "hubId": log.hub_id, "cursor": log.cursor(), "writable": true }))
                }
            }
            _ if !greeted => Err((-32002, "annoxSync/hello must come first".into())),
            "annoxSync/pull" => pull(&log.lock().unwrap(), params),
            "annoxSync/push" => push(&mut log.lock().unwrap(), connection, params),
            _ => Err((-32601, format!("unknown method {method}"))),
        };
        peer.respond(&id, result)?;
    }
}

fn pull(log: &Log, params: &Value) -> Result<Value, RpcError> {
    if params["hubId"].as_str() != Some(log.hub_id.as_str()) {
        return Err((UNKNOWN_HUB, "the hub id doesn't match this hub's log".into()));
    }
    let after = params["after"].as_u64().unwrap_or(0) as usize;
    let limit = params["limit"].as_u64().map_or(PULL_LIMIT, |l| (l as usize).clamp(1, PULL_LIMIT));
    let items: Vec<Value> = log.items.iter().skip(after).take(limit).cloned().collect();
    let cursor = after + items.len();
    Ok(json!({ "items": items, "cursor": cursor, "more": cursor < log.items.len() }))
}

fn push(log: &mut Log, connection: u64, params: &Value) -> Result<Value, RpcError> {
    let items = params["items"].as_array().ok_or((-32602, "items must be an array".to_owned()))?;
    let mut results = Vec::with_capacity(items.len());
    let mut fresh = Vec::new();
    for item in items {
        results.push(match log.accept(item) {
            Ok((seq, is_new)) => {
                if is_new {
                    fresh.push(log.items[seq as usize - 1].clone());
                }
                json!({ "seq": seq })
            }
            Err((code, message)) => json!({ "error": { "code": code, "message": message } }),
        });
    }
    if !fresh.is_empty() {
        let params = json!({ "hubId": log.hub_id, "items": fresh, "cursor": log.cursor() });
        for (&conn, tx) in &log.subscribers {
            if conn != connection {
                let _ = tx.send(notification("annoxSync/didReceive", params.clone()));
            }
        }
    }
    Ok(json!({ "results": results }))
}
