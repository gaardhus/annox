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
    let uri = lsp_types::Url::from_file_path(&file).unwrap().to_string();
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
        let r = self
            .client
            .call("annox/annotations", json!({ "textDocument": { "uri": self.uri }, "includeClosed": closed }));
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
    assert_eq!(comment["editedBy"], Value::Null);
    let id = comment["id"].as_str().unwrap().to_owned();

    let reply = c.call("annox/reply", json!({ "parent": id, "body": "Section 3." }));
    assert_eq!(reply["parent"], json!(id));
    assert_eq!(c.call_err("annox/reply", json!({ "parent": reply["id"], "body": "nested" })), 1003);

    let edited = c.call("annox/edit", json!({ "annotation": id, "body": "Which section exactly?" }));
    assert_eq!(edited["body"], "Which section exactly?");
    assert_eq!(edited["editedBy"]["author"], comment["author"]);
    assert_eq!(edited["replies"][0]["editedBy"], Value::Null);
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
    assert_eq!(kinds, ["create", "create", "edit"], "the root, its reply, then the edit");
    assert_eq!(history[1]["annotation"], reply["id"]);
    assert_eq!(
        f.client.call("annox/history", json!({ "annotation": reply["id"] })),
        history,
        "a reply shows its thread"
    );

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

    let resolved =
        f.client.call("annox/resolveConflict", json!({ "annotation": id, "field": "body", "value": "merged" }));
    assert_eq!(resolved["body"], "merged");
    assert_eq!(resolved["conflicts"], json!({}));
    let code = f.client.call_err("annox/resolveConflict", json!({ "annotation": id, "field": "body", "value": "x" }));
    assert_eq!(code, 1004);
    f.client.shutdown();
}

#[test]
fn accept_all_needs_confirmation_for_partial_context() {
    let mut f = setup();
    let uri = f.uri.clone();
    let sugg = f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "suggestion", "range": range(1, 14, 27), "replacement": "we show that" }),
    );
    let sid = sugg["id"].as_str().unwrap().to_owned();
    // The suffix changes, so the suggestion is found by quote search (step 3).
    let edited = DOC.replace("tight", "sharp");
    f.client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": edited }] }),
    );

    let results = f.client.call("annox/acceptAll", json!({ "annotations": [sid] }))["results"].clone();
    assert_eq!(results[0]["error"]["code"], 1008);

    let id = f.client.request("annox/acceptAll", json!({ "annotations": [sid], "confirmed": true }));
    let edit = f.client.answer_apply_edit(true);
    assert_eq!(edit["edit"]["changes"][&uri][0]["newText"], "we show that");
    let results = f.client.response(&id).unwrap()["results"].clone();
    assert_eq!(results[0], json!({ "annotation": sid, "accepted": true }));
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
    let comment = f
        .client
        .call("annox/create", json!({ "textDocument": { "uri": uri }, "kind": "comment", "range": range(1, 0, 12) }));
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
        let own: Vec<_> = index
            .events
            .values()
            .filter(|(e, _)| e.annotation.as_deref() == Some(&sid))
            .map(|(e, _)| e.clone())
            .collect();
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
fn revert_suggests_or_applies_the_original_text() {
    let mut f = setup();
    let uri = f.uri.clone();
    let sugg = f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "suggestion", "range": range(1, 14, 27), "replacement": "we show that" }),
    );
    let sid = sugg["id"].as_str().unwrap().to_owned();
    assert_eq!(f.client.call_err("annox/revert", json!({ "annotation": sid })), 1003, "only accepted ones");

    let id = f.client.request("annox/accept", json!({ "annotation": sid }));
    f.client.answer_apply_edit(true);
    f.client.response(&id).unwrap();
    let applied = DOC.replace("we prove that", "we show that");
    f.client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": applied }] }),
    );

    // Without `accept`, the revert is an open suggestion linked to the original.
    let revert = f.client.call("annox/revert", json!({ "annotation": sid, "body": "too strong" }));
    assert_eq!(
        (&revert["status"], &revert["reverts"], &revert["body"]),
        (&json!("open"), &json!(sid), &json!("too strong"))
    );
    assert_eq!(revert["edit"]["replacement"], "we prove that");
    assert_eq!(revert["resolution"]["range"], range(1, 14, 26));

    let again = f.client.call("annox/revert", json!({ "annotation": sid }));
    assert_eq!(again["id"], revert["id"], "the open revert is used again");

    // With `accept`, it's applied like annox/accept.
    let id = f.client.request("annox/revert", json!({ "annotation": sid, "accept": true }));
    let edit = f.client.answer_apply_edit(true);
    let change = &edit["edit"]["changes"][&uri][0];
    assert_eq!((&change["newText"], &change["range"]), (&json!("we prove that"), &range(1, 14, 26)));
    let view = f.client.response(&id).unwrap();
    assert_eq!((&view["id"], &view["status"], &view["reverts"]), (&revert["id"], &json!("accepted"), &json!(sid)));
    f.client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": DOC }] }),
    );
    assert_eq!(f.client.call_err("annox/revert", json!({ "annotation": sid })), 1003, "already reverted");

    // Once the applied text is gone, it can't be found to revert.
    let other = f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "suggestion", "range": range(1, 14, 27), "replacement": "we show that" }),
    );
    let oid = other["id"].as_str().unwrap().to_owned();
    let id = f.client.request("annox/accept", json!({ "annotation": oid }));
    f.client.answer_apply_edit(true);
    f.client.response(&id).unwrap();
    f.client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 4 }, "contentChanges": [{ "text": DOC }] }),
    );
    assert_eq!(f.client.call_err("annox/revert", json!({ "annotation": oid })), 1002);
    f.client.shutdown();
}

#[test]
fn annotations_at_include_closed_ones() {
    let mut f = setup();
    let uri = f.uri.clone();
    let create = |c: &mut Client, fields: Value| {
        let mut params = json!({ "textDocument": { "uri": uri } });
        params.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        c.call("annox/create", params)["id"].as_str().unwrap().to_owned()
    };
    let resolved = create(&mut f.client, json!({ "kind": "comment", "range": range(1, 3, 12), "body": "Which?" }));
    f.client.call("annox/setStatus", json!({ "annotation": resolved, "status": "resolved" }));
    let accepted = create(
        &mut f.client,
        json!({ "kind": "suggestion", "range": range(1, 14, 27), "replacement": "we show that" }),
    );
    let id = f.client.request("annox/accept", json!({ "annotation": accepted }));
    f.client.answer_apply_edit(true);
    f.client.response(&id).unwrap();
    let applied = DOC.replace("we prove that", "we show that");
    f.client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": applied }] }),
    );
    let open = create(&mut f.client, json!({ "kind": "comment", "range": range(1, 31, 36), "body": "tight?" }));

    let mut at = |r: Value| -> Vec<Value> {
        let result = f.client.call("annox/annotationsAt", json!({ "textDocument": { "uri": uri }, "range": r }));
        result["annotations"].as_array().unwrap().clone()
    };
    let hits = at(range(1, 14, 20));
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (&hits[0]["id"], &hits[0]["status"], &hits[0]["quote"], &hits[0]["at"]),
        (&json!(accepted), &json!("accepted"), &json!("we prove that"), &range(1, 14, 26)),
        "an accepted suggestion is on the text it put in"
    );
    assert!(hits[0].get("resolution").is_none(), "a closed one isn't resolved");
    let mut ids: Vec<String> = at(range(1, 0, 40)).iter().map(|v| v["id"].as_str().unwrap().to_owned()).collect();
    ids.sort();
    let mut all = vec![resolved.clone(), accepted, open];
    all.sort();
    assert_eq!(ids, all);
    let point = at(range(1, 5, 5));
    assert_eq!((point.len(), &point[0]["id"]), (1, &json!(resolved)), "a point inside the text counts");
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
    let new_uri = lsp_types::Url::from_file_path(&new_path).unwrap().to_string();
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

#[test]
fn commit_previews_then_commits() {
    let mut f = setup();
    let uri = f.uri.clone();
    let root = f.ws.root.clone();
    let commit = json!({ "textDocument": { "uri": uri }, "dryRun": true });
    assert_eq!(f.client.call_err("annox/commit", commit.clone()), 1003, "not a git repository");

    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git").arg("-C").arg(&root).args(args).status().unwrap().success())
    };
    git(&["init", "--quiet"]);
    git(&["config", "user.email", "ada@example.org"]);
    git(&["config", "user.name", "Ada"]);
    git(&["config", "commit.gpgsign", "false"]);
    f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "comment", "range": range(1, 0, 12), "body": "Which section?" }),
    );
    let planned = f.client.call("annox/commit", commit);
    assert_eq!(planned["commit"], Value::Null);
    assert_eq!(planned["message"], "chore(annox): 1 comment on paper.tex");

    let committed = f.client.call("annox/commit", json!({ "textDocument": { "uri": uri }, "message": "Review" }));
    assert!(committed["commit"].is_string());
    assert_eq!((&committed["message"], &committed["files"]), (&"Review".into(), &planned["files"]));
    let nothing = f.client.call("annox/commit", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(nothing["files"], 0);
}

#[test]
fn accepting_carries_comments_on_the_replaced_text() {
    let mut f = setup();
    let uri = f.uri.clone();
    let sugg = f.client.call(
        "annox/create",
        json!({ "textDocument": { "uri": uri }, "kind": "suggestion", "range": range(1, 14, 27), "replacement": "we show that" }),
    );
    let comment = f
        .client
        .call("annox/create", json!({ "textDocument": { "uri": uri }, "kind": "comment", "range": range(1, 17, 37) }));
    let id = f.client.request("annox/accept", json!({ "annotation": sugg["id"] }));
    f.client.answer_apply_edit(true);
    f.client.response(&id).unwrap();
    f.client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": DOC.replace("we prove that", "we show that") }] }),
    );
    let view = f.annotations(false).into_iter().find(|v| v["id"] == comment["id"]).unwrap();
    assert_eq!(view["resolution"], json!({ "state": "exact", "step": 0, "range": range(1, 14, 36) }));
    f.client.shutdown();
}
