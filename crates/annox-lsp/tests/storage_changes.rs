//! Noticing outside changes to `.annox/` (§6.4) and creating a workspace
//! from the editor (§6.2).

mod common;

use annox_core::ops::{self, NewAnnotation};
use annox_core::storage::{Index, Workspace};
use annox_core::text::Text;
use common::Client;
use serde_json::json;

const DOC: &str = "Hello annotated world.\n";

fn open(client: &Client, uri: &str) {
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "text", "version": 1, "text": DOC } }),
    );
}

#[test]
fn outside_changes_are_pushed_without_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::init(dir.path()).unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, DOC).unwrap();
    let uri = format!("file://{}", file.display());
    // No didChangeWatchedFiles support: the server polls.
    let (client, _) = Client::start(json!({
        "capabilities": { "experimental": { "annox": { "version": "0.0" } } },
        "initializationOptions": { "annox": { "diagnostics": false } },
    }));
    open(&client, &uri);
    assert_eq!(client.notification("annox/didChangeAnnotations")["annotations"], json!([]));

    // Another tool (or `git pull`) adds a comment on disk.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let new = NewAnnotation { kind: "comment", start: 6, end: 15, body: Some("from git"), label: None, replacement: None, local: false };
    let author = json!({ "id": "mailto:bob@example.org" });
    ops::create_annotation(&ws, &Index::read(&ws), "notes.txt", &Text::from_raw(DOC), &new, &author).unwrap();

    let pushed = client.notification("annox/didChangeAnnotations");
    assert_eq!(pushed["annotations"][0]["body"], "from git");
    client.shutdown();
}

#[test]
fn registers_a_watcher_when_the_client_can_watch() {
    let (client, _) = Client::start(json!({
        "capabilities": { "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } },
    }));
    let params = client.answer("client/registerCapability", json!(null));
    let registration = &params["registrations"][0];
    assert_eq!(registration["method"], "workspace/didChangeWatchedFiles");
    assert_eq!(registration["registerOptions"]["watchers"][0]["globPattern"], "**/.annox/**");
    client.shutdown();
}

#[test]
fn creating_the_first_annotation_offers_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sub").join("notes.txt");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, DOC).unwrap();
    let uri = format!("file://{}", file.display());
    let root_uri = format!("file://{}", dir.path().display());
    let (mut client, _) = Client::start(json!({
        "capabilities": {},
        "workspaceFolders": [{ "uri": root_uri, "name": "root" }],
    }));
    open(&client, &uri);
    let create = json!({
        "textDocument": { "uri": uri },
        "kind": "comment",
        "range": { "start": { "line": 0, "character": 6 }, "end": { "line": 0, "character": 15 } },
        "body": "first!",
    });

    // Declined: nothing is created.
    let id = client.request("annox/create", create.clone());
    let prompt = client.answer("window/showMessageRequest", json!({ "title": "Cancel" }));
    assert!(prompt["message"].as_str().unwrap().contains(&dir.path().display().to_string()));
    assert_eq!(client.response(&id).unwrap_err().code, 1005);
    assert!(!dir.path().join(".annox").exists());

    // Accepted: the workspace goes in the workspace folder, not the file's directory.
    let id = client.request("annox/create", create);
    client.answer("window/showMessageRequest", json!({ "title": "Create workspace" }));
    let view = client.response(&id).unwrap();
    assert_eq!(view["body"], "first!");
    assert!(dir.path().join(".annox/annox.json").is_file());
    assert!(!dir.path().join("sub/.annox").exists());
    client.shutdown();
}
