//! Unread annotations (§5.13): the `unread` flag, `annox/markRead`, and
//! `annox/unread`.

mod common;

use annox_core::ops::{self, NewAnnotation};
use annox_core::storage::{Index, Workspace};
use annox_core::text::Text;
use common::Client;
use serde_json::{json, Map, Value};

const DOC: &str = "Hello annotated world.\n";

struct Fixture {
    _dir: tempfile::TempDir,
    ws: Workspace,
    uri: String,
    client: Client,
}

const BOB: &str = "mailto:bob@example.org";

/// Bob comments on `path` on disk, as if it arrived by `git pull`.
fn bob_comments(ws: &Workspace, path: &str, body: &str) -> String {
    let new = NewAnnotation {
        kind: "comment",
        start: 6,
        end: 15,
        body: Some(body),
        label: None,
        replacement: None,
        local: false,
        reverts: None,
    };
    let author = json!({ "id": BOB });
    ops::create_annotation(ws, &Index::read(ws), path, &Text::from_raw(DOC), &new, &author).unwrap().id
}

/// A workspace where Bob commented on `notes.txt` before Ada started, and Ada
/// has `notes.txt` open.
fn setup() -> Fixture {
    setup_with(json!({}))
}

/// `setup`, with extra `initializationOptions.annox`.
fn setup_with(options: Value) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::init(dir.path()).unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, DOC).unwrap();
    std::fs::write(dir.path().join("other.txt"), DOC).unwrap();
    bob_comments(&ws, "notes.txt", "before Ada");
    let mut annox = json!({
        "author": { "id": "mailto:ada@example.org", "name": "Ada" },
        "diagnostics": false,
    });
    annox.as_object_mut().unwrap().extend(options.as_object().unwrap().clone());
    let (client, _) = Client::start(json!({
        "capabilities": { "experimental": { "annox": { "version": "0.1" } } },
        "initializationOptions": { "annox": annox },
    }));
    let uri = lsp_types::Url::from_file_path(&file).unwrap().to_string();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "text", "version": 1, "text": DOC } }),
    );
    client.notification("annox/didChangeAnnotations");
    Fixture { _dir: dir, ws, uri, client }
}

impl Fixture {
    fn annotations(&mut self) -> Vec<Value> {
        let r = self.client.call("annox/annotations", json!({ "textDocument": { "uri": self.uri } }));
        r["annotations"].as_array().unwrap().clone()
    }

    fn unread(&mut self, body: &str) -> bool {
        let a = self.annotations();
        let view = a.iter().find(|v| v["body"] == body).unwrap_or_else(|| panic!("no {body} in {a:#?}"));
        view["unread"].as_bool().unwrap()
    }
}

#[test]
fn a_new_reader_starts_with_nothing_unread() {
    let mut f = setup();
    assert!(!f.unread("before Ada"));
    assert_eq!(f.client.call("annox/unread", json!({})), json!({ "documents": [] }));
    f.client.shutdown();
}

#[test]
fn others_changes_are_unread_until_marked_read() {
    let mut f = setup();
    let id = bob_comments(&f.ws, "notes.txt", "after Ada");
    assert!(f.unread("after Ada"));
    assert!(!f.unread("before Ada"));

    let marked = f.client.call("annox/markRead", json!({ "annotations": [id] }));
    assert_eq!(marked["annotations"], json!([id]));
    assert!(!f.unread("after Ada"));

    // An edit by Bob makes it unread again.
    let index = Index::read(&f.ws);
    let fields = Map::from_iter([("body".into(), json!("after Ada, edited"))]);
    ops::append_event(&f.ws, &index, &id, "edit", fields, &json!({ "id": BOB })).unwrap();
    assert!(f.unread("after Ada, edited"));

    assert_eq!(f.client.call_err("annox/markRead", json!({ "annotations": ["nope"] })), 1001);
    f.client.call("annox/markRead", json!({ "all": true }));
    assert!(!f.unread("after Ada, edited"));
    f.client.shutdown();
}

#[test]
fn replying_marks_the_thread_read() {
    let mut f = setup();
    let id = bob_comments(&f.ws, "notes.txt", "after Ada");
    let bob_reply =
        ops::create_reply(&f.ws, &Index::read(&f.ws), &id, "and more", false, &json!({ "id": BOB })).unwrap().id;
    let thread = f.annotations().into_iter().find(|v| v["id"] == id).unwrap();
    assert_eq!(thread["unread"], true);
    assert_eq!(thread["replies"][0]["id"], bob_reply);
    assert_eq!(thread["replies"][0]["unread"], true);

    f.client.call("annox/reply", json!({ "parent": id, "body": "Thanks" }));
    let thread = f.annotations().into_iter().find(|v| v["id"] == id).unwrap();
    assert_eq!(thread["unread"], false);
    assert!(thread["replies"].as_array().unwrap().iter().all(|r| r["unread"] == false));
    f.client.shutdown();
}

#[test]
fn unread_lists_threads_in_closed_documents() {
    let mut f = setup();
    let id = bob_comments(&f.ws, "other.txt", "elsewhere");
    let unread = f.client.call("annox/unread", json!({}));
    let docs = unread["documents"].as_array().unwrap();
    assert_eq!(docs.len(), 1);
    assert!(docs[0]["textDocument"]["uri"].as_str().unwrap().ends_with("/other.txt"));
    assert_eq!(docs[0]["annotations"][0]["id"], id);
    assert_eq!(docs[0]["annotations"][0]["resolution"]["state"], "exact");

    // A reply to a read thread lists the thread.
    f.client.call("annox/markRead", json!({ "all": true }));
    ops::create_reply(&f.ws, &Index::read(&f.ws), &id, "ping", false, &json!({ "id": BOB })).unwrap();
    let unread = f.client.call("annox/unread", json!({}));
    assert_eq!(unread["documents"][0]["annotations"][0]["id"], id);
    f.client.shutdown();
}

#[test]
fn with_unread_off_nothing_is_unread_and_the_readers_state_is_dropped() {
    let mut f = setup_with(json!({ "unread": false }));
    let knows = |ws: &Workspace| annox_core::read::ReadState::load(ws).knows("mailto:ada@example.org");
    assert!(!knows(&f.ws));
    let id = bob_comments(&f.ws, "notes.txt", "while off");
    assert!(!f.unread("while off"));
    assert_eq!(f.client.call("annox/unread", json!({})), json!({ "documents": [] }));
    assert_eq!(f.client.call("annox/markRead", json!({ "annotations": [id] })), json!({ "annotations": [] }));

    // Writing still records it, as the CLI would; opening a document drops it.
    f.client.call("annox/reply", json!({ "parent": id, "body": "Thanks" }));
    assert!(knows(&f.ws));
    let other = lsp_types::Url::from_file_path(f.ws.root.join("other.txt")).unwrap().to_string();
    f.client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": other, "languageId": "text", "version": 1, "text": DOC } }),
    );
    f.client.call("annox/unread", json!({}));
    assert!(!knows(&f.ws));
    f.client.shutdown();
}
