//! End-to-end tests of the `annox/*` extension methods (§6.6).

mod common;

use annox_core::ops;
use annox_core::storage::{Index, Workspace};
use common::Client;
use serde_json::{json, Map, Value};

const DOC: &str = "\\section{Results}\nIn Section 3, we prove that the bound is tight.\n";

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({ "start": { "line": line, "character": start }, "end": { "line": line, "character": end } })
}

struct Fixture {
    _dir: tempfile::TempDir,
    ws: Workspace,
    uri: String,
    client: Client,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::init(dir.path()).unwrap();
    let file = dir.path().join("paper.tex");
    std::fs::write(&file, DOC).unwrap();
    let (client, _) = Client::start(json!({
        "capabilities": { "experimental": { "annox": { "version": "0.1" } } },
        "initializationOptions": { "annox": {
            "author": { "id": "mailto:ada@example.org", "name": "Ada" },
            "diagnostics": false,
        } },
    }));
    let uri = format!("file://{}", file.display());
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "latex", "version": 1, "text": DOC } }),
    );
    let pushed = client.notification("annox/didChangeAnnotations");
    assert_eq!(pushed["annotations"], json!([]));
    Fixture { _dir: dir, ws, uri, client }
}

impl Fixture {
    /// Writes two events to `id` against the same snapshot, so they are
    /// concurrent (as if made on two branches).
    fn concurrent(&self, id: &str, a: (&str, Value), b: (&str, Value)) {
        let index = Index::read(&self.ws);
        let author = json!({ "id": "mailto:bob@example.org", "name": "Bob" });
        for (kind, fields) in [a, b] {
            let fields: Map<String, Value> = fields.as_object().unwrap().clone();
            ops::append_event(&self.ws, &index, id, kind, fields, &author).unwrap();
        }
    }

    fn annotations(&mut self, closed: bool) -> Vec<Value> {
        let r = self.client.call(
            "annox/annotations",
            json!({ "textDocument": { "uri": self.uri }, "includeClosed": closed }),
        );
        r["annotations"].as_array().unwrap().clone()
    }
}

#[test]
fn create_reply_edit_publish() {
    let mut f = setup();
    let uri = f.uri.clone();
    let c = &mut f.client;

    let comment = c.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "comment", "range": range(1, 0, 12), "body": "Which section?" }),
    );
    assert_eq!(comment["kind"], "comment");
    assert_eq!(comment["resolution"]["state"], "exact");
    assert_eq!(comment["resolution"]["range"], range(1, 0, 12));
    assert_eq!(comment["local"], false);
    assert!(comment.get("target").is_none(), "views omit target");
    let id = comment["id"].as_str().unwrap().to_owned();

    let reply = c.call("annox/reply", json!({ "parent": id, "body": "Section 3." }));
    assert_eq!(reply["parent"], json!(id));
    assert_eq!(c.call_err("annox/reply", json!({ "parent": reply["id"], "body": "nested" })), 1003);

    let edited = c.call("annox/edit", json!({ "annotation": id, "body": "Which section exactly?" }));
    assert_eq!(edited["body"], "Which section exactly?");
    assert_eq!(edited["replies"][0]["body"], "Section 3.");

    // A local draft, then published (§5.11).
    let draft = c.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "comment", "range": range(1, 14, 22), "body": "draft", "local": true }),
    );
    assert_eq!(draft["local"], true);
    let draft_id = draft["id"].as_str().unwrap().to_owned();
    let local_files = walk(&f.ws.root.join(".annox/local"));
    assert!(local_files.iter().any(|p| p.ends_with(&format!("{draft_id}.json"))));
    let c = &mut f.client;
    let published = c.call("annox/publish", json!({ "annotations": [draft_id] }));
    assert_eq!(published[0]["local"], false);
    assert!(walk(&f.ws.root.join(".annox/local")).iter().all(|p| !p.ends_with(&format!("{draft_id}.json"))));

    let all = f.annotations(false);
    assert_eq!(all.len(), 2);

    let history = f.client.call("annox/history", json!({ "annotation": id }));
    let kinds: Vec<&str> = history.as_array().unwrap().iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["create", "edit"]);

    assert_eq!(f.client.call_err("annox/setStatus", json!({ "annotation": id, "status": "accepted" })), 1003);
    let resolved = f.client.call("annox/setStatus", json!({ "annotation": id, "status": "resolved" }));
    assert_eq!(resolved["status"], "resolved");
    assert_eq!(f.annotations(false).len(), 1, "resolved threads are hidden by default");
    assert_eq!(f.annotations(true).len(), 2);
    f.client.shutdown();
}

#[test]
fn conflicts_are_surfaced_and_resolved() {
    let mut f = setup();
    let uri = f.uri.clone();
    let comment = f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "comment", "range": range(1, 0, 12), "body": "v1" }),
    );
    let id = comment["id"].as_str().unwrap().to_owned();
    f.concurrent(&id, ("edit", json!({ "body": "mine" })), ("edit", json!({ "body": "theirs" })));

    let view = &f.annotations(false)[0];
    let entries = view["conflicts"]["body"].as_array().unwrap();
    let mut values: Vec<&str> = entries.iter().map(|e| e["value"].as_str().unwrap()).collect();
    values.sort_unstable();
    assert_eq!(values, ["mine", "theirs"]);
    assert_eq!(entries[0]["author"]["name"], "Bob");

    let resolved = f.client.call(
        "annox/resolveConflict",
        json!({ "annotation": id, "field": "body", "value": "merged" }),
    );
    assert_eq!(resolved["body"], "merged");
    assert_eq!(resolved["conflicts"], json!({}));
    let code = f.client.call_err("annox/resolveConflict", json!({ "annotation": id, "field": "body", "value": "x" }));
    assert_eq!(code, 1004);
    f.client.shutdown();
}

#[test]
fn accept_all_and_revert() {
    let mut f = setup();
    let uri = f.uri.clone();
    let sugg = f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "suggestion", "range": range(1, 14, 27), "replacement": "we show that" }),
    );
    let comment = f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "comment", "range": range(1, 0, 12) }),
    );
    assert_eq!(sugg["applicable"], true);
    let sid = sugg["id"].as_str().unwrap().to_owned();

    let id = f.client.request("annox/acceptAll", json!({ "annotations": [sid, comment["id"]] }));
    let edit = f.client.answer_apply_edit(true);
    assert_eq!(edit["edit"]["changes"][&uri].as_array().unwrap().len(), 1);
    let results = f.client.response(&id).unwrap()["results"].clone();
    assert_eq!(results[0], json!({ "annotation": sid, "accepted": true }));
    assert_eq!(results[1]["error"]["code"], 1003);

    // The editor applied the edit; a concurrent "reject" arrives from a branch.
    let applied = DOC.replace("we prove that", "we show that");
    f.client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": applied }] }),
    );
    let index = Index::read(&f.ws);
    let heads: Vec<String> = {
        let own: Vec<_> = index.events.values().filter(|(e, _)| e.annotation.as_deref() == Some(&sid)).map(|(e, _)| e.clone()).collect();
        let accepted = own.iter().find(|e| e.kind == "status").unwrap();
        accepted.after.clone()
    };
    let reject = annox_core::event::Event {
        id: annox_core::new_id(),
        annotation: Some(sid.clone()),
        document: None,
        after: heads,
        kind: "status".into(),
        author: json!({ "id": "mailto:bob@example.org" }),
        time: annox_core::event::Event::now(),
        fields: Map::from_iter([("status".into(), json!("rejected"))]),
    };
    let folder = &index.events[&sid].1.folder;
    f.ws.write_event(folder, &reject).unwrap();

    let view = f.annotations(true).into_iter().find(|v| v["id"] == json!(sid)).unwrap();
    let mut statuses: Vec<&str> =
        view["conflicts"]["status"].as_array().unwrap().iter().map(|e| e["value"].as_str().unwrap()).collect();
    statuses.sort_unstable();
    assert_eq!(statuses, ["accepted", "rejected"]);

    // Resolving to "rejected" with revert: the server first reverts the edit.
    let id = f.client.request(
        "annox/resolveConflict",
        json!({ "annotation": sid, "field": "status", "value": "rejected", "revert": true }),
    );
    let edit = f.client.answer_apply_edit(true);
    let change = &edit["edit"]["changes"][&uri][0];
    assert_eq!(change["newText"], "we prove that");
    assert_eq!(change["range"], range(1, 14, 26));
    let view = f.client.response(&id).unwrap();
    assert_eq!(view["status"], "rejected");
    assert_eq!(view["conflicts"], json!({}));
    f.client.shutdown();
}

#[test]
fn move_document_keeps_annotations() {
    let mut f = setup();
    let uri = f.uri.clone();
    f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "comment", "range": range(1, 0, 12), "body": "hi" }),
    );
    let new_path = f.ws.root.join("final.tex");
    std::fs::rename(f.ws.root.join("paper.tex"), &new_path).unwrap();
    let new_uri = format!("file://{}", new_path.display());
    let moved = f.client.call("annox/moveDocument", json!({ "from": uri, "to": new_uri }));
    assert_eq!(moved["moved"], 1);
    f.client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": new_uri, "languageId": "latex", "version": 1, "text": DOC } }),
    );
    let r = f.client.call("annox/annotations", json!({ "textDocument": { "uri": new_uri } }));
    assert_eq!(r["annotations"][0]["body"], "hi");
    let folders = walk(&f.ws.root.join(".annox/docs"));
    assert!(folders.iter().all(|p| p.contains("final.tex~")), "folders renamed: {folders:?}");
    f.client.shutdown();
}

fn walk(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p.display().to_string());
            }
        }
    }
    out
}
