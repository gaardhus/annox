//! End-to-end test of the language server over an in-memory connection.

use std::time::Duration;

use annox_core::ops::{self, NewAnnotation};
use annox_core::storage::{Index, Workspace};
use annox_core::text::Text;
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use serde_json::{json, Value};

const DOC: &str = "\\section{Results}\nIn Section 3, we prove that the bound is tight.\n";

struct Client {
    conn: Connection,
    next: i32,
}

impl Client {
    fn request(&mut self, method: &str, params: Value) -> RequestId {
        self.next += 1;
        let id = RequestId::from(self.next);
        self.conn.sender.send(Request::new(id.clone(), method.into(), params).into()).unwrap();
        id
    }

    fn notify(&self, method: &str, params: Value) {
        self.conn.sender.send(Notification::new(method.into(), params).into()).unwrap();
    }

    /// Receives messages until `pick` returns something.
    fn recv_until<T>(&self, mut pick: impl FnMut(Message) -> Option<T>) -> T {
        loop {
            let msg = self.conn.receiver.recv_timeout(Duration::from_secs(10)).expect("server went quiet");
            if let Some(t) = pick(msg) {
                return t;
            }
        }
    }

    fn response(&self, id: &RequestId) -> Result<Value, lsp_server::ResponseError> {
        self.recv_until(|m| match m {
            Message::Response(r) if &r.id == id => Some(r.response_result),
            _ => None,
        })
    }

    fn diagnostics(&self) -> Vec<Value> {
        self.recv_until(|m| match m {
            Message::Notification(n) if n.method == "textDocument/publishDiagnostics" => {
                Some(n.params["diagnostics"].as_array().unwrap().clone())
            }
            _ => None,
        })
    }
}

#[test]
fn diagnostics_hover_and_accept() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::init(dir.path()).unwrap();
    let file = dir.path().join("paper.tex");
    std::fs::write(&file, DOC).unwrap();
    let text = Text::from_raw(DOC);
    let author = json!({ "id": "mailto:ada@example.org", "name": "Ada" });
    let comment = NewAnnotation {
        kind: "comment",
        start: 18,
        end: 30,
        body: Some("Which section?"),
        label: None,
        replacement: None,
        local: false,
        reverts: None,
    };
    let comment = ops::create_annotation(&ws, &Index::read(&ws), "paper.tex", &text, &comment, &author).unwrap();
    let sugg = NewAnnotation {
        kind: "suggestion",
        start: 32,
        end: 45,
        body: None,
        label: None,
        replacement: Some("we show that"),
        local: false,
        reverts: None,
    };
    let sugg = ops::create_annotation(&ws, &Index::read(&ws), "paper.tex", &text, &sugg, &author).unwrap();

    let (server_conn, client_conn) = Connection::memory();
    let server = std::thread::spawn(move || annox_lsp::run(&server_conn).unwrap());
    let mut client = Client { conn: client_conn, next: 0 };
    let uri = lsp_types::Url::from_file_path(&file).unwrap().to_string();

    let init = client.request(
        "initialize",
        json!({
            "capabilities": { "general": { "positionEncodings": ["utf-8"] } },
            "initializationOptions": { "annox": { "author": { "id": "mailto:bob@example.org", "name": "Bob" } } },
        }),
    );
    let result = client.response(&init).unwrap();
    assert_eq!(result["capabilities"]["positionEncoding"], "utf-8");
    assert_eq!(result["capabilities"]["experimental"]["annox"]["version"], "0.1");
    client.notify("initialized", json!({}));

    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "latex", "version": 1, "text": DOC } }),
    );
    let diags = client.diagnostics();
    assert_eq!(diags.len(), 2, "{diags:#?}");
    let messages: Vec<&str> = diags.iter().map(|d| d["message"].as_str().unwrap()).collect();
    assert!(messages.contains(&"Which section?"));
    assert!(messages.contains(&"Suggestion: \"we prove that\" → \"we show that\""));

    let hover = client.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 16 } }),
    );
    let hover = client.response(&hover).unwrap();
    let markdown = hover["contents"]["value"].as_str().unwrap();
    assert!(markdown.contains("```diff\n- we prove that\n+ we show that\n```"), "{markdown}");

    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": { "line": 1, "character": 16 }, "end": { "line": 1, "character": 16 } },
            "context": { "diagnostics": [] },
        }),
    );
    let actions = client.response(&actions).unwrap();
    let titles: Vec<&str> = actions.as_array().unwrap().iter().map(|a| a["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Accept suggestion", "Reject suggestion"]);

    // Accept: the server asks the client to apply the edit first (§6.6.2).
    let accept = client.request(
        "workspace/executeCommand",
        json!({ "command": "annox.accept", "arguments": [{ "annotation": sugg.id }] }),
    );
    let apply = client.recv_until(|m| match m {
        Message::Request(r) if r.method == "workspace/applyEdit" => Some(r),
        _ => None,
    });
    let edits = &apply.params["edit"]["changes"][&uri];
    assert_eq!(edits[0]["newText"], "we show that");
    assert_eq!(
        edits[0]["range"],
        json!({ "start": { "line": 1, "character": 14 }, "end": { "line": 1, "character": 27 } })
    );
    client.conn.sender.send(Response::new_ok(apply.id, json!({ "applied": true })).into()).unwrap();
    assert_eq!(client.response(&accept).unwrap(), Value::Null);

    let expected = Text::from_raw(&DOC.replace("we prove that", "we show that"));
    let loaded = Index::read(&ws).load("paper.tex");
    let state = &loaded.derive()[&sugg.id];
    assert_eq!(state["status"], "accepted");
    assert_eq!(state["appliedVersion"], json!(expected.version));

    // Resolve the comment thread, written as Bob.
    let resolve = client.request(
        "workspace/executeCommand",
        json!({ "command": "annox.resolve", "arguments": [{ "annotation": comment.id }] }),
    );
    assert_eq!(client.response(&resolve).unwrap(), Value::Null);
    let loaded = Index::read(&ws).load("paper.tex");
    assert_eq!(loaded.derive()[&comment.id]["status"], "resolved");
    let status_event =
        loaded.events.iter().find(|e| e.kind == "status" && e.annotation.as_deref() == Some(&comment.id));
    assert_eq!(status_event.unwrap().author["name"], "Bob");

    // Hover shows only open threads (§6.5.2).
    let hover_comment = |client: &mut Client| {
        let id = client.request(
            "textDocument/hover",
            json!({ "textDocument": { "uri": uri }, "position": { "line": 1, "character": 5 } }),
        );
        client.response(&id).unwrap()
    };
    assert_eq!(hover_comment(&mut client), Value::Null);

    // Reopen the thread, then replace the buffer: the comment is orphaned.
    let reopen = client.request(
        "workspace/executeCommand",
        json!({ "command": "annox.reopen", "arguments": [{ "annotation": comment.id }] }),
    );
    assert_eq!(client.response(&reopen).unwrap(), Value::Null);
    let hover = hover_comment(&mut client);
    assert!(hover["contents"]["value"].as_str().unwrap().contains("Which section?"), "{hover}");
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": "unrelated\n" }] }),
    );
    let orphan_warning = |diags: &[Value]| diags.iter().any(|d| d["message"] == "1 annotation could not be located");
    let mut diags = client.diagnostics();
    while !orphan_warning(&diags) {
        diags = client.diagnostics();
    }
    assert_eq!(diags.len(), 1, "{diags:#?}");

    let shutdown = client.request("shutdown", Value::Null);
    client.response(&shutdown).unwrap();
    client.notify("exit", Value::Null);
    server.join().unwrap();
}

#[test]
fn reattach_an_orphan_to_its_suggested_location() {
    let before = "# Plan\n\nExisting projects are backfilled to NULL, which means the legacy path.\n\nNext: workers.\n";
    let after = "# Plan\n\nAll existing projects are backfilled to 0.5, which means no legacy path.\n\nWorkers next.\n";
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::init(dir.path()).unwrap();
    let file = dir.path().join("plan.md");
    std::fs::write(&file, before).unwrap();
    let quote = "Existing projects are backfilled to NULL, which means the legacy path.";
    let start = before.find(quote).unwrap();
    let new = NewAnnotation {
        kind: "comment",
        start,
        end: start + quote.len(),
        body: Some("Why NULL?"),
        label: None,
        replacement: None,
        local: false,
        reverts: None,
    };
    let author = json!({ "id": "mailto:ada@example.org", "name": "Ada" });
    let comment =
        ops::create_annotation(&ws, &Index::read(&ws), "plan.md", &Text::from_raw(before), &new, &author).unwrap();
    std::fs::write(&file, after).unwrap();

    let (server_conn, client_conn) = Connection::memory();
    let server = std::thread::spawn(move || annox_lsp::run(&server_conn).unwrap());
    let mut client = Client { conn: client_conn, next: 0 };
    let uri = lsp_types::Url::from_file_path(&file).unwrap().to_string();
    let init = client.request("initialize", json!({ "capabilities": {} }));
    client.response(&init).unwrap();
    client.notify("initialized", json!({}));
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uri, "languageId": "markdown", "version": 1, "text": after } }),
    );

    // The orphan warning, plus a hint where the comment probably belongs.
    let diags = client.diagnostics();
    let hint = diags.iter().find(|d| d["data"]["suggested"] == true).unwrap_or_else(|| panic!("{diags:#?}"));
    assert_eq!(hint["message"], "Orphaned comment may belong here: Why NULL?");
    let line = after.lines().nth(2).unwrap();
    let from = line.find("existing").unwrap();
    assert_eq!(
        hint["range"],
        json!({ "start": { "line": 2, "character": from }, "end": { "line": 2, "character": line.len() } })
    );

    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": hint["range"], "context": { "diagnostics": [] } }),
    );
    let actions = client.response(&actions).unwrap();
    let titles: Vec<&str> = actions.as_array().unwrap().iter().map(|a| a["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Re-attach orphaned comment here", "Resolve thread"]);

    let reattach = client.request(
        "workspace/executeCommand",
        json!({ "command": "annox.reattachSuggested", "arguments": [{ "annotation": comment.id }] }),
    );
    assert_eq!(client.response(&reattach).unwrap(), Value::Null);
    let state = &Index::read(&ws).load("plan.md").derive()[&comment.id];
    assert_eq!(state["target"]["selectors"]["quote"]["exact"], &line[from..]);

    let shutdown = client.request("shutdown", Value::Null);
    client.response(&shutdown).unwrap();
    client.notify("exit", Value::Null);
    server.join().unwrap();
}
