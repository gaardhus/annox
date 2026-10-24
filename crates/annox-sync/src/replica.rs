//! The replica side of sync (§7.6): keeps a workspace's shared `.annox/`
//! in step with a hub, on a background thread.
//!
//! Received events are written as ordinary files, so storage stays the single
//! source of truth. Local events the hub lacks are pushed in causal order.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use annox_core::event::Event;
use annox_core::storage::{Area, Index, Workspace};
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;

use crate::rpc::{Error, Peer, Timeout};
use crate::{is_document_path, is_well_formed, FORMAT, SUBPROTOCOL, UNKNOWN_HUB};

const PUSH_BATCH: usize = 200;

#[derive(Clone, Debug)]
pub struct ReplicaConfig {
    /// Workspace root.
    pub root: PathBuf,
    /// Hub URL from `annox.json` (§7.2).
    pub url: String,
    pub token: Option<String>,
    /// Author sent with presence.
    pub author: Value,
}

/// Commands from the owner of the replica.
#[derive(Clone, Debug)]
pub enum ToReplica {
    /// Push any shared events the hub doesn't have yet.
    Scan,
    /// Send presence: `{ document?, range?, version? }` (§7.8).
    Presence(Value),
    Stop,
}

/// Messages to the owner of the replica.
#[derive(Clone, Debug)]
pub enum FromReplica {
    /// Events from the hub were written to storage.
    Changed { root: PathBuf },
    /// Others' presence (§7.8.1).
    Presence { root: PathBuf, peers: Vec<Value> },
    /// Something worth logging, such as a connection failure.
    Status { root: PathBuf, message: String },
}

/// Starts a replica thread. Drop or send `Stop` to end it.
pub fn spawn(config: ReplicaConfig, events: Sender<FromReplica>) -> Sender<ToReplica> {
    let (tx, rx) = unbounded();
    std::thread::spawn(move || run(config, rx, events));
    tx
}

/// Why `wss:` is required (§7.2): plain `ws:` only to this machine.
pub fn check_url(url: &str) -> Result<(), String> {
    if url.starts_with("wss://") {
        return Ok(());
    }
    let rest = url.strip_prefix("ws://").ok_or("the hub URL must start with wss://")?;
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    let host =
        host.rsplit_once(':').map_or(host, |(h, port)| if port.chars().all(|c| c.is_ascii_digit()) { h } else { host });
    if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        Ok(())
    } else {
        Err(format!("refusing unencrypted ws:// to {host}; use wss://"))
    }
}

fn run(config: ReplicaConfig, commands: Receiver<ToReplica>, events: Sender<FromReplica>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let status = |message: String| {
        let _ = events.send(FromReplica::Status { root: config.root.clone(), message });
    };
    if let Err(e) = check_url(&config.url) {
        return status(e);
    }
    let mut replica = Replica { config: config.clone(), known: HashSet::new(), pending: Vec::new() };
    let mut backoff = Duration::from_secs(1);
    loop {
        match replica.session(&commands, &events) {
            Ok(()) => return,
            Err(e) => status(format!("sync with {} interrupted: {e}", config.url)),
        }
        // Wait before reconnecting, but still honour Stop.
        match commands.recv_timeout(backoff) {
            Ok(ToReplica::Stop) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            _ => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

struct Replica {
    config: ReplicaConfig,
    /// Event ids the hub is known to have.
    known: HashSet<String>,
    /// Received items whose document isn't known yet (§7.5.1).
    pending: Vec<Value>,
}

type Conn = Peer<MaybeTlsStream<TcpStream>>;

impl Replica {
    fn ws(&self) -> Workspace {
        Workspace { root: self.config.root.clone() }
    }

    fn cursor_file(&self) -> PathBuf {
        self.config.root.join(".annox/cache/sync.json")
    }

    /// The saved cursor for `hub_id`, restoring the items that were pending
    /// with it: the cursor is already past them, so the hub won't send them
    /// again.
    fn load_cursor(&mut self, hub_id: &str) -> u64 {
        let saved: Option<Value> =
            std::fs::read_to_string(self.cursor_file()).ok().and_then(|t| serde_json::from_str(&t).ok());
        match saved {
            Some(s) if s["hubId"] == hub_id && s["url"] == self.config.url.as_str() => {
                if self.pending.is_empty() {
                    self.pending =
                        s["pending"].as_array().into_iter().flatten().filter(|i| is_well_formed(i)).cloned().collect();
                }
                s["cursor"].as_u64().unwrap_or(0)
            }
            _ => 0,
        }
    }

    fn save_cursor(&self, hub_id: &str, cursor: u64) {
        let file = self.cursor_file();
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let saved = json!({ "url": self.config.url, "hubId": hub_id, "cursor": cursor, "pending": self.pending });
        let _ = std::fs::write(file, saved.to_string());
    }

    fn connect(&self) -> Result<Conn, Error> {
        let mut request = self.config.url.as_str().into_client_request().map_err(Error::from)?;
        request.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_static(SUBPROTOCOL));
        if let Some(token) = &self.config.token {
            let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| Error::Io(e.to_string()))?;
            request.headers_mut().insert("authorization", value);
        }
        let (ws, _) = tungstenite::connect(request).map_err(Error::from)?;
        ws.get_ref().set_timeout(Some(Duration::from_millis(50))).map_err(|e| Error::Io(e.to_string()))?;
        Ok(Peer::new(ws))
    }

    fn call(conn: &mut Conn, method: &str, params: Value) -> Result<Value, Error> {
        conn.call(method, params)?.map_err(|(code, msg)| Error::Io(format!("{method}: {msg} ({code})")))
    }

    fn session(&mut self, commands: &Receiver<ToReplica>, events: &Sender<FromReplica>) -> Result<(), Error> {
        let root = self.config.root.clone();
        // Syncing writes to the workspace (§5.3).
        self.ws().check_format().map_err(|e| Error::Io(e.to_string()))?;
        let mut conn = self.connect()?;
        let hello = Self::call(&mut conn, "annoxSync/hello", json!({ "format": FORMAT, "subscribe": true }))?;
        let hub_id = hello["hubId"].as_str().unwrap_or_default().to_owned();
        let mut cursor = self.load_cursor(&hub_id);
        // Pending items may have found their document since, for example
        // through git, and the hub won't send them again.
        if !self.pending.is_empty() {
            self.apply(Vec::new(), events)?;
        }
        self.pull(&mut conn, &hub_id, &mut cursor, events)?;
        self.push_missing(&mut conn)?;
        loop {
            loop {
                match commands.try_recv() {
                    Ok(ToReplica::Scan) => self.push_missing(&mut conn)?,
                    Ok(ToReplica::Presence(mut p)) => {
                        p["author"] = self.config.author.clone();
                        conn.notify("annoxSync/presence", p)?;
                    }
                    Ok(ToReplica::Stop) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        conn.close();
                        return Ok(());
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                }
            }
            let Some(msg) = conn.recv()? else { continue };
            match msg["method"].as_str() {
                Some("annoxSync/didReceive") => {
                    let params = &msg["params"];
                    let first = params["items"][0]["seq"].as_u64().unwrap_or(cursor + 1);
                    if params["hubId"] != hub_id.as_str() || first > cursor + 1 {
                        // A gap: pull to fill it (§7.5.2).
                        self.pull(&mut conn, &hub_id, &mut cursor, events)?;
                    } else {
                        let items: Vec<Value> = params["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|i| i["seq"].as_u64().is_some_and(|s| s > cursor))
                            .cloned()
                            .collect();
                        if let Some(last) = items.last().and_then(|i| i["seq"].as_u64()) {
                            self.apply(items, events)?;
                            cursor = last;
                            self.save_cursor(&hub_id, cursor);
                            let _ = events.send(FromReplica::Changed { root: root.clone() });
                        }
                    }
                }
                Some("annoxSync/didChangePresence") => {
                    let peers = msg["params"]["peers"].as_array().cloned().unwrap_or_default();
                    let _ = events.send(FromReplica::Presence { root: root.clone(), peers });
                }
                _ => {}
            }
        }
    }

    fn pull(
        &mut self,
        conn: &mut Conn,
        hub_id: &str,
        cursor: &mut u64,
        events: &Sender<FromReplica>,
    ) -> Result<(), Error> {
        loop {
            let result = conn.call("annoxSync/pull", json!({ "hubId": hub_id, "after": *cursor }))?;
            let page = match result {
                Ok(page) => page,
                Err((UNKNOWN_HUB, _)) => {
                    *cursor = 0;
                    continue;
                }
                Err((code, msg)) => return Err(Error::Io(format!("annoxSync/pull: {msg} ({code})"))),
            };
            let items = page["items"].as_array().cloned().unwrap_or_default();
            if !items.is_empty() {
                self.apply(items, events)?;
                let _ = events.send(FromReplica::Changed { root: self.config.root.clone() });
            }
            *cursor = page["cursor"].as_u64().unwrap_or(*cursor);
            self.save_cursor(hub_id, *cursor);
            if !page["more"].as_bool().unwrap_or(false) {
                return Ok(());
            }
        }
    }

    /// Writes received items as event files in the sync mirror (§5.12,
    /// §7.5.1), even when git already delivered a copy, so the mirror keeps
    /// every event the hub sent. Items are untrusted: ids and paths are
    /// validated before anything touches the disk.
    ///
    /// Fails if an event can't be written, before the caller advances the
    /// cursor past it, so that it is received again once the session
    /// reconnects; events already written are skipped then.
    fn apply(&mut self, items: Vec<Value>, events: &Sender<FromReplica>) -> Result<(), Error> {
        let ws = self.ws();
        let (well_formed, ill_formed): (Vec<Value>, Vec<Value>) = items.into_iter().partition(is_well_formed);
        let mut malformed = ill_formed.len();
        let mut queue: Vec<Value> = std::mem::take(&mut self.pending);
        queue.extend(well_formed);
        let mut seen = HashSet::new();
        queue.retain(|i| seen.insert(i["event"]["id"].as_str().unwrap_or_default().to_owned()));
        for item in &queue {
            if let Some(id) = item["event"]["id"].as_str() {
                self.known.insert(id.to_owned());
            }
        }
        // Document events first, so annotation events find their folders.
        queue.sort_by_key(|i| i["event"]["annotation"].is_string());
        // Older workspaces don't ignore the mirror yet (§5.9).
        let _ = ws.ensure_ignored();
        let index = Index::read(&ws);
        // Each document's path, kept up to date as document events are
        // written rather than by reading the workspace again. Any of a
        // document's paths names a folder of it (§5.6).
        let mut paths: BTreeMap<String, String> =
            index.documents.iter().filter_map(|(id, d)| Some((id.clone(), d.path.clone()?))).collect();
        let mut still_pending = Vec::new();
        let mut queue = queue.into_iter();
        while let Some(item) = queue.next() {
            let Ok(event) = serde_json::from_value::<Event>(item["event"].clone()) else {
                malformed += 1;
                continue;
            };
            let doc = item["document"].as_str().unwrap_or_default();
            if index.synced.contains(&event.id) {
                continue;
            }
            let is_document_event = event.document.as_deref() == Some(doc);
            let own_path = || event.field("path")?.as_str().filter(|p| is_document_path(p)).map(str::to_owned);
            let path = paths.get(doc).cloned().or_else(|| (event.kind == "document").then(own_path).flatten());
            let Some(path) = path.filter(|p| is_document_path(p)) else {
                still_pending.push(item);
                continue;
            };
            let folder = if is_document_event {
                format!("synced/docs/{path}~{doc}/document")
            } else {
                format!("synced/docs/{path}~{doc}")
            };
            if let Err(e) = ws.write_event(&folder, &event) {
                // Keep what was pending, as the hub won't send it again.
                still_pending.push(item);
                still_pending.extend(queue);
                self.pending = still_pending;
                return Err(Error::Io(format!("can't write event {}: {e}", event.id)));
            }
            if is_document_event {
                if let Some(p) = own_path() {
                    paths.insert(doc.to_owned(), p);
                }
            }
        }
        self.pending = still_pending;
        if malformed > 0 {
            let message = format!("ignored {malformed} event(s) from {} that aren't valid", self.config.url);
            let _ = events.send(FromReplica::Status { root: self.config.root.clone(), message });
        }
        Ok(())
    }

    /// Pushes every shared event the hub isn't known to have, each after its
    /// document's events and after its `after` events (§7.6).
    fn push_missing(&mut self, conn: &mut Conn) -> Result<(), Error> {
        let index = Index::read(&self.ws());
        let mut items = Vec::new();
        for (doc, events) in &index.document_events {
            if index.areas.get(doc) != Some(&Area::Shared) {
                continue;
            }
            let evs: Vec<Event> = events.iter().map(|(e, _)| e.clone()).collect();
            for e in causal_order(evs) {
                items.push(json!({ "document": doc, "event": e }));
            }
        }
        let mut by_annotation: BTreeMap<&str, Vec<(Event, &str)>> = BTreeMap::new();
        for (e, loc) in index.events.values() {
            if loc.area == Area::Shared {
                if let Some(a) = e.annotation.as_deref() {
                    by_annotation.entry(a).or_default().push((e.clone(), &loc.document));
                }
            }
        }
        for evs in by_annotation.values() {
            let docs: BTreeMap<String, &str> = evs.iter().map(|(e, d)| (e.id.clone(), *d)).collect();
            for e in causal_order(evs.iter().map(|(e, _)| e.clone()).collect()) {
                items.push(json!({ "document": docs[&e.id], "event": e }));
            }
        }
        items.retain(|i| !self.known.contains(i["event"]["id"].as_str().unwrap_or_default()));
        for batch in items.chunks(PUSH_BATCH) {
            let result = Self::call(conn, "annoxSync/push", json!({ "items": batch }))?;
            for (item, r) in batch.iter().zip(result["results"].as_array().into_iter().flatten()) {
                if r.get("seq").is_some() {
                    self.known.insert(item["event"]["id"].as_str().unwrap_or_default().to_owned());
                }
            }
        }
        Ok(())
    }
}

/// Events ordered so each comes after the events in its `after` list; ties
/// by id. Events whose parents are missing come last.
fn causal_order(events: Vec<Event>) -> Vec<Event> {
    let ids: BTreeSet<String> = events.iter().map(|e| e.id.clone()).collect();
    let mut remaining: BTreeMap<String, Event> = events.into_iter().map(|e| (e.id.clone(), e)).collect();
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    loop {
        let ready: Vec<String> = remaining
            .values()
            .filter(|e| e.after.iter().all(|a| done.contains(a) || !ids.contains(a)))
            .map(|e| e.id.clone())
            .collect();
        if ready.is_empty() {
            out.extend(remaining.into_values());
            return out;
        }
        for id in ready {
            done.insert(id.clone());
            out.push(remaining.remove(&id).expect("ready event is remaining"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(doc: &str, id: &str, annotation: bool) -> Value {
        let mut event = json!({
            "id": id, "after": [], "author": { "id": "mailto:ada@example.org" }, "time": "2026-09-29T10:00:00Z",
        });
        if annotation {
            event["annotation"] = json!(id);
            event["type"] = json!("create");
        } else {
            event["document"] = json!(doc);
            event["type"] = json!("document");
            event["path"] = json!("a.md");
        }
        json!({ "document": doc, "event": event })
    }

    #[test]
    fn events_that_cant_be_written_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::init(dir.path()).unwrap();
        let config =
            ReplicaConfig { root: ws.root.clone(), url: "ws://localhost".into(), token: None, author: json!({}) };
        let mut replica = Replica { config, known: HashSet::new(), pending: Vec::new() };
        let (tx, rx) = unbounded();
        let doc = "01926d3a-0000-7000-8000-000000000000";
        let ann = "01926d3a-0001-7000-8000-000000000000";
        let mut broken = item(doc, "01926d3a-0002-7000-8000-000000000000", true);
        broken["event"]["author"] = json!(1);

        let marker = dir.path().join(".annox/annox.json");
        std::fs::write(&marker, "{ \"format\": 2 }").unwrap();
        assert!(replica.apply(vec![item(doc, doc, false), item(doc, ann, true)], &tx).is_err());
        assert_eq!(replica.pending.len(), 2, "nothing is lost when writing fails");
        assert!(Index::read(&ws).synced.is_empty());

        std::fs::write(&marker, "{ \"format\": 1 }").unwrap();
        replica.apply(vec![broken], &tx).unwrap();
        assert!(replica.pending.is_empty());
        let synced = Index::read(&ws).synced;
        assert!(synced.contains(doc) && synced.contains(ann), "{synced:?}");
        let reported =
            rx.try_iter().any(|m| matches!(m, FromReplica::Status { message, .. } if message.contains("ignored 1")));
        assert!(reported, "the broken item is reported");
    }

    #[test]
    fn requires_tls_except_locally() {
        assert!(check_url("wss://hub.example.org/w/x").is_ok());
        assert!(check_url("ws://localhost:7878/w/x").is_ok());
        assert!(check_url("ws://127.0.0.1:7878/w/x").is_ok());
        assert!(check_url("ws://hub.example.org/w/x").is_err());
        assert!(check_url("http://hub.example.org/w/x").is_err());
    }
}
