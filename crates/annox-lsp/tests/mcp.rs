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
    let config =
        Config { root: dir.path().to_owned(), author: Some("urn:test:agent".into()), name: Some("Agent".into()) };
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
