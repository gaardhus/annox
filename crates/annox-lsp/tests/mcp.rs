//! Tests of the MCP server (`annox mcp`), driven over in-memory stdio.

use annox_lsp::mcp::{serve, Config};
use serde_json::{json, Value};

/// Sends `messages`, one per line, and returns the responses.
fn exchange(config: &Config, messages: &[Value]) -> Vec<Value> {
    let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
    let mut output = Vec::new();
    serve(input.as_bytes(), &mut output, config).unwrap();
    String::from_utf8(output).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn call(id: i64, name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
}

/// The JSON a successful tool call printed.
fn output(response: &Value) -> Value {
    assert_eq!(response["result"]["isError"], false, "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn setup() -> (tempfile::TempDir, Config) {
    let dir = tempfile::tempdir().unwrap();
    annox_core::storage::Workspace::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("paper.md"), "We prove that teh bound is tight.\n").unwrap();
    let config = Config {
        root: dir.path().to_owned(),
        author: Some("urn:test:agent".into()),
        name: Some("Agent".into()),
        channel: None,
    };
    (dir, config)
}

#[test]
fn handshake_and_tool_list() {
    let (_dir, config) = setup();
    let responses = exchange(
        &config,
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "nope" }),
        ],
    );
    assert_eq!(responses.len(), 3, "notifications get no response");
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-03-26");
    let tools: Vec<&str> =
        responses[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(tools.contains(&"suggest") && tools.contains(&"retarget") && tools.contains(&"restore"), "{tools:?}");
    let suggest = responses[1]["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "suggest").unwrap();
    assert_eq!(suggest["inputSchema"]["required"], json!(["file", "quote", "replacement"]));
    assert_eq!(responses[2]["error"]["code"], -32601);
}

#[test]
fn suggest_reply_and_accept() {
    let (dir, config) = setup();
    let created = exchange(
        &config,
        &[call(1, "suggest", json!({ "file": "paper.md", "quote": "teh", "replacement": "the", "body": "typo" }))],
    );
    let id = output(&created[0])["id"].as_str().unwrap().to_owned();

    let responses = exchange(
        &config,
        &[
            call(2, "reply", json!({ "id": id, "body": "Thanks" })),
            call(3, "list_annotations", json!({ "file": "paper.md" })),
            call(4, "accept", json!({ "id": id })),
            call(5, "list_annotations", json!({ "all": true })),
        ],
    );
    let list = output(&responses[1]);
    assert_eq!(list[0]["author"], json!({ "id": "urn:test:agent", "name": "Agent" }));
    assert_eq!(list[0]["replies"][0]["body"], "Thanks");
    output(&responses[2]);
    assert_eq!(output(&responses[3])[0]["status"], "accepted");
    assert_eq!(std::fs::read_to_string(dir.path().join("paper.md")).unwrap(), "We prove that the bound is tight.\n");
}

#[test]
fn errors_are_tool_results() {
    let (_dir, config) = setup();
    let responses = exchange(
        &config,
        &[
            call(1, "comment", json!({ "file": "paper.md", "quote": "missing", "body": "x" })),
            call(2, "comment", json!({ "file": "paper.md", "quote": "teh" })),
            call(3, "comment", json!({ "file": "paper.md", "quote": "teh", "body": "x", "bogus": 1 })),
            call(4, "nope", json!({})),
        ],
    );
    for (r, needle) in responses.iter().zip(["not found", "missing argument body", "unknown argument bogus"]) {
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains(needle), "{r}");
    }
    assert_eq!(responses[3]["error"]["code"], -32602);
}

#[test]
fn list_filters_and_show() {
    let (dir, config) = setup();
    let created = exchange(
        &config,
        &[
            call(1, "suggest", json!({ "file": "paper.md", "quote": "teh", "replacement": "the" })),
            call(2, "comment", json!({ "file": "paper.md", "quote": "bound", "body": "Which?" })),
        ],
    );
    let s = output(&created[0])["id"].clone();
    let c = output(&created[1])["id"].clone();
    let other = Config { root: dir.path().to_owned(), author: Some("urn:test:user".into()), name: None, channel: None };
    let responses = exchange(
        &other,
        &[
            call(1, "list_annotations", json!({ "kind": "suggestion" })),
            call(2, "list_annotations", json!({ "mine": true })),
            call(3, "list_annotations", json!({ "others": true, "kind": "comment" })),
            call(4, "list_annotations", json!({ "not_author": "urn:test:agent", "status": "open,resolved" })),
            call(5, "show_annotation", json!({ "id": c })),
        ],
    );
    let ids = |r: &Value| output(r).as_array().unwrap().iter().map(|a| a["id"].clone()).collect::<Vec<_>>();
    assert_eq!(ids(&responses[0]), vec![s]);
    assert_eq!(ids(&responses[1]), Vec::<Value>::new(), "`mine` is the server's identity");
    assert_eq!(ids(&responses[2]), vec![c.clone()]);
    assert_eq!(ids(&responses[3]), Vec::<Value>::new());
    assert_eq!(output(&responses[4])["body"], "Which?");
}

#[test]
fn unread_uses_the_servers_identity() {
    let (dir, config) = setup();
    let unread = |config: &Config| {
        let r = exchange(config, &[call(1, "list_annotations", json!({ "unread": true }))]);
        output(&r[0])
    };
    assert_eq!(unread(&config), json!([]));
    // Someone else comments.
    let ada = Config { author: Some("mailto:ada@example.org".into()), name: None, ..config };
    exchange(&ada, &[call(1, "comment", json!({ "file": "paper.md", "quote": "teh", "body": "Typo?" }))]);
    let config =
        Config { author: Some("urn:test:agent".into()), name: None, root: dir.path().to_owned(), channel: None };
    let listed = unread(&config);
    assert_eq!(listed[0]["body"], "Typo?");
    assert_eq!(unread(&ada), json!([]));

    let id = listed[0]["id"].as_str().unwrap();
    output(&exchange(&config, &[call(2, "mark_read", json!({ "id": id }))])[0]);
    assert_eq!(unread(&config), json!([]));
}

/// Input that yields what is sent on a channel, and ends when it's dropped.
struct Pipe(std::sync::mpsc::Receiver<String>, Vec<u8>);

impl std::io::Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.1.is_empty() {
            match self.0.recv() {
                Ok(line) => self.1 = line.into_bytes(),
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.1.len());
        buf[..n].copy_from_slice(&self.1[..n]);
        self.1.drain(..n);
        Ok(n)
    }
}

/// Output that can be read while the server is still writing it.
#[derive(Clone, Default)]
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Shared {
    fn messages(&self) -> Vec<Value> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

#[test]
fn channel_pushes_threads_addressed_to_the_agent() {
    let (dir, mut config) = setup();
    config.channel = Some(annox_lsp::cli::Watch {
        reader: config.author.clone(),
        mention: vec!["@agent".into()],
        settle: 0,
        ..Default::default()
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let output = Shared::default();
    let server = {
        let (output, config) = (output.clone(), Config { channel: config.channel.clone(), ..config });
        std::thread::spawn(move || serve(std::io::BufReader::new(Pipe(rx, Vec::new())), output, &config).unwrap())
    };
    let init =
        json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } });
    tx.send(format!("{init}\n")).unwrap();
    tx.send(format!("{}\n", json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(700));

    let comment = |body: &str| {
        let args = ["comment", "paper.md", "--quote", "teh", "--body", body, "--author", "urn:test:user"];
        annox_lsp::cli::run(&args.map(str::to_owned), dir.path()).unwrap()
    };
    comment("Not for the agent.");
    let asked = comment("@Agent, is this a typo?");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let events = loop {
        let events: Vec<Value> =
            output.messages().into_iter().filter(|m| m["method"] == "notifications/claude/channel").collect();
        if !events.is_empty() || std::time::Instant::now() > deadline {
            break events;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    drop(tx);
    server.join().unwrap();

    let messages = output.messages();
    assert_eq!(messages[0]["result"]["capabilities"]["experimental"]["claude/channel"], json!({}));
    assert!(messages[0]["result"]["instructions"].as_str().unwrap().contains("<channel source=\"annox\""));
    assert_eq!(events.len(), 1, "{events:?}");
    let params = &events[0]["params"];
    assert_eq!(params["meta"]["annotation_id"], asked["id"]);
    assert_eq!(params["meta"]["path"], "paper.md");
    assert_eq!(params["meta"]["line"], "1");
    let thread: Value = serde_json::from_str(params["content"].as_str().unwrap()).unwrap();
    assert_eq!(thread["body"], "@Agent, is this a typo?");
}
