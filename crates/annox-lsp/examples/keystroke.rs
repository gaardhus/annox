//! What editing costs in the server: a 100k-character document with 1,000
//! annotations. Run with:
//!
//!     cargo run --release -p annox-lsp --example keystroke

use std::time::{Duration, Instant};

use annox_core::anchor;
use annox_core::event::Event;
use annox_core::ops::{self, NewAnnotation};
use annox_core::storage::{Index, Workspace};
use annox_core::text::Text;
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{json, Map, Value};

fn main() {
    let mut raw = String::new();
    let mut i = 0;
    while raw.len() < 100_000 {
        raw.push_str(&format!("Sentence {i} states that the bound holds for every graph.\n"));
        i += 1;
    }
    let text = Text::from_raw(&raw);
    let dir = std::env::temp_dir().join(format!("annox-keystroke-{}", annox_core::new_id()));
    let ws = Workspace::init(&dir).unwrap();
    std::fs::write(dir.join("doc.txt"), &raw).unwrap();
    let author = json!({ "id": "mailto:bench@example.org" });
    let first = NewAnnotation {
        kind: "comment",
        start: 0,
        end: 8,
        body: Some("hmm"),
        label: None,
        replacement: None,
        local: false,
    };
    ops::create_annotation(&ws, &Index::read(&ws), "doc.txt", &text, &first, &author).unwrap();
    let folder = Index::read(&ws).events.values().next().unwrap().1.folder.clone();
    let stride = raw.len() / 1000;
    for k in 1..1000 {
        let at = k * stride + raw[k * stride..].find("holds for every").unwrap();
        let id = annox_core::new_id();
        let fields = Map::from_iter([
            ("kind".into(), json!("comment")),
            ("target".into(), json!(anchor::create(&text, at, at + 15, "doc.txt"))),
            ("body".into(), json!("hmm")),
        ]);
        let e = Event {
            id: id.clone(),
            annotation: Some(id),
            document: None,
            after: vec![],
            kind: "create".into(),
            author: author.clone(),
            time: Event::now(),
            fields,
        };
        ws.write_event(&folder, &e).unwrap();
    }

    let (server, client) = Connection::memory();
    let handle = std::thread::spawn(move || annox_lsp::run(&server).unwrap());
    let send = |m: Message| client.sender.send(m).unwrap();
    let wait = |pred: &dyn Fn(&Message) -> bool| loop {
        let m = client.receiver.recv_timeout(Duration::from_secs(60)).unwrap();
        if pred(&m) {
            return m;
        }
    };
    let is_diagnostics =
        |m: &Message| matches!(m, Message::Notification(n) if n.method == "textDocument/publishDiagnostics");
    send(Request::new(RequestId::from(1), "initialize".into(), json!({ "capabilities": {} })).into());
    wait(&|m| matches!(m, Message::Response(_)));
    send(Notification::new("initialized".into(), json!({})).into());
    let uri = format!("file://{}", dir.join("doc.txt").display());

    let start = Instant::now();
    send(
        Notification::new(
            "textDocument/didOpen".into(),
            json!({ "textDocument": { "uri": uri, "languageId": "text", "version": 1, "text": raw } }),
        )
        .into(),
    );
    wait(&is_diagnostics);
    println!("open (cold: read storage, derive, resolve): {:.1} ms", start.elapsed().as_secs_f64() * 1e3);

    // 20 keystrokes at the top of the file, faster than the debounce.
    let start = Instant::now();
    let mut edited = raw.clone();
    for k in 0..20 {
        edited.insert(0, 'x');
        send(
            Notification::new(
                "textDocument/didChange".into(),
                json!({ "textDocument": { "uri": uri, "version": k + 2 }, "contentChanges": [{ "text": edited }] }),
            )
            .into(),
        );
    }
    let sent = start.elapsed();
    let diags = wait(&is_diagnostics);
    let total = start.elapsed();
    let count = match diags {
        Message::Notification(n) => n.params["diagnostics"].as_array().map_or(0, Vec::len),
        _ => 0,
    };
    println!("20 keystrokes sent in {:.1} ms; one refresh after the pause, {:.1} ms after the first keystroke (debounce 150 ms), {count} diagnostics",
        sent.as_secs_f64() * 1e3, total.as_secs_f64() * 1e3);

    let hover = |id: i32| {
        let start = Instant::now();
        send(
            Request::new(
                RequestId::from(id),
                "textDocument/hover".into(),
                json!({ "textDocument": { "uri": uri }, "position": { "line": 500, "character": 30 } }),
            )
            .into(),
        );
        wait(&|m| matches!(m, Message::Response(r) if r.id == RequestId::from(id)));
        start.elapsed()
    };
    println!("hover (warm cache): {:.2} ms", hover(10).as_secs_f64() * 1e3);

    send(Request::new(RequestId::from(99), "shutdown".into(), Value::Null).into());
    wait(&|m| matches!(m, Message::Response(r) if r.id == RequestId::from(99)));
    send(Notification::new("exit".into(), Value::Null).into());
    handle.join().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
