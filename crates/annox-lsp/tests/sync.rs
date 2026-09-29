//! Two language servers syncing through a hub (§7): annotations and
//! presence made in one copy of a workspace appear live in the other.

mod common;

use std::net::TcpListener;

use annox_core::storage::Workspace;
use annox_sync::hub::{serve, HubConfig};
use common::Client;
use serde_json::{json, Value};

const DOC: &str = "Hello annotated world.\n";

fn workspace(dir: &std::path::Path, url: &str) -> String {
    Workspace::init(dir).unwrap();
    std::fs::write(dir.join(".annox/annox.json"), json!({ "format": 1, "sync": { "url": url } }).to_string()).unwrap();
    let file = dir.join("notes.txt");
    std::fs::write(&file, DOC).unwrap();
    format!("file://{}", file.display())
}

fn client(uri: &str, name: &str) -> Client {
    let (client, _) = Client::start(json!({
        "capabilities": { "experimental": { "annox": { "version": "0.0" } } },
        "initializationOptions": { "annox": {
            "diagnostics": false,
            "author": { "id": format!("mailto:{name}@example.org"), "name": name },
        } },
    }));
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "text", "version": 1, "text": DOC } }),
    );
    client
}

/// Waits for an `annox/didChangeAnnotations` push satisfying `pred`.
fn wait_annotations(client: &Client, pred: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    loop {
        let pushed = client.notification("annox/didChangeAnnotations");
        let annotations = pushed["annotations"].as_array().unwrap().clone();
        if pred(&annotations) {
            return annotations;
        }
    }
}

#[test]
fn annotations_and_presence_sync_between_servers() {
    let hub_data = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/w/notes", listener.local_addr().unwrap());
    std::thread::spawn(move || serve(listener, HubConfig { data: hub_data.path().to_path_buf(), token: Some("tok".into()) }));

    // Per-user credentials, outside both workspaces (§7.2).
    let config = tempfile::tempdir().unwrap();
    let credentials = config.path().join("credentials.json");
    std::fs::write(&credentials, json!({ "hubs": { url.clone(): { "token": "tok" } } }).to_string()).unwrap();
    // SAFETY: this test binary has a single test, so nothing reads the
    // environment concurrently.
    unsafe { std::env::set_var("ANNOX_CREDENTIALS_FILE", &credentials) };

    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (uri_a, uri_b) = (workspace(dir_a.path(), &url), workspace(dir_b.path(), &url));
    let mut ada = client(&uri_a, "ada");
    let mut bob = client(&uri_b, "bob");

    let comment = ada.call(
        "annox/create",
        json!({
            "textDocument": { "uri": uri_a },
            "kind": "comment",
            "range": { "start": { "line": 0, "character": 6 }, "end": { "line": 0, "character": 15 } },
            "body": "Synced?",
        }),
    );
    let id = comment["id"].clone();

    // Bob's server receives the document record and the comment from the hub.
    let seen = wait_annotations(&bob, |a| !a.is_empty());
    assert_eq!(seen[0]["id"], id);
    assert_eq!(seen[0]["body"], "Synced?");
    assert_eq!(seen[0]["resolution"]["state"], "exact");

    // And the reverse direction, through a reply.
    bob.call("annox/reply", json!({ "parent": id, "body": "Yes!" }));
    let seen = wait_annotations(&ada, |a| a.first().is_some_and(|c| !c["replies"].as_array().unwrap().is_empty()));
    assert_eq!(seen[0]["replies"][0]["author"]["name"], "bob");

    // Presence: Ada's cursor shows up in Bob's copy of the file.
    ada.notify(
        "annox/setPresence",
        json!({
            "textDocument": { "uri": uri_a },
            "selection": { "start": { "line": 0, "character": 6 }, "end": { "line": 0, "character": 15 } },
        }),
    );
    let peers = loop {
        let p = bob.notification("annox/didChangePresence")["peers"].clone();
        if p.as_array().is_some_and(|a| !a.is_empty()) {
            break p;
        }
    };
    assert_eq!(peers[0]["author"]["name"], "ada");
    assert_eq!(peers[0]["textDocument"]["uri"], uri_b);
    assert_eq!(peers[0]["range"]["start"]["character"], 6);

    ada.shutdown();
    bob.shutdown();
}
