//! Syncing through a hub (§7): starting replicas, and passing presence both ways (§7.8).

use annox_core::anchor::{self, Anchor};
use annox_core::storage::{Index, Workspace};
use annox_core::text::Text;
use annox_sync::replica::{self, FromReplica, ReplicaConfig, ToReplica};
use lsp_server::Notification;
use lsp_types::Url;
use serde_json::{json, Value};

use crate::position::LineIndex;
use crate::Server;

impl Server<'_> {
    /// Starts a replica for the workspace of `uri` if it names a hub and the
    /// user has credentials for it (§6.4, §7.2).
    pub(crate) fn ensure_replica(&mut self, uri: &Url) {
        let Some(ws) = uri.to_file_path().ok().and_then(|p| Workspace::find(&p)) else { return };
        if self.replicas.contains_key(&ws.root) {
            return;
        }
        let replica = match replica_config(&ws, || self.author(&ws)) {
            Some(Ok(config)) => Some(replica::spawn(config, self.sync_tx.clone())),
            Some(Err(message)) => {
                self.warn(message);
                None
            }
            None => None,
        };
        self.replicas.insert(ws.root, replica);
    }

    pub(crate) fn on_sync(&mut self, event: FromReplica) {
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
            None => Text::from_raw(&crate::pdf::read_document(&ws.root.join(&path)).unwrap_or_default()),
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
    pub(crate) fn set_presence(&self, params: &Value) {
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
}

/// How to sync workspace `ws` with the hub its `annox.json` names, if it
/// names one (§7.2), syncing as `author`; or why it can't.
pub(crate) fn replica_config(ws: &Workspace, author: impl FnOnce() -> Value) -> Option<Result<ReplicaConfig, String>> {
    let marker = std::fs::read_to_string(ws.root.join(".annox/annox.json")).unwrap_or_default();
    let url = serde_json::from_str::<Value>(&marker).ok()?["sync"]["url"].as_str()?.to_owned();
    let Some(credential) = annox_sync::credentials::lookup(&url) else {
        return Some(Err(format!("annox: {url} has no entry in the credentials file; not syncing")));
    };
    Some(Ok(ReplicaConfig { root: ws.root.clone(), url, token: credential.token, author: author() }))
}
