//! The hub over real WebSockets (§7.4–§7.9).

use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use annox_sync::hub::{serve, HubConfig};
use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

fn start(token: Option<&str>) -> (String, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let config = HubConfig { data: data.path().to_path_buf(), token: token.map(str::to_owned) };
    std::thread::spawn(move || serve(listener, config));
    (format!("ws://{addr}"), data)
}

struct Client {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next: u64,
}

impl Client {
    fn connect(url: &str, token: Option<&str>) -> Result<Client, tungstenite::Error> {
        let mut req = url.into_client_request()?;
        if let Some(t) = token {
            req.headers_mut().insert("authorization", format!("Bearer {t}").parse().unwrap());
        }
        let (ws, _) = tungstenite::connect(req)?;
        if let MaybeTlsStream::Plain(s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        }
        Ok(Client { ws, next: 0 })
    }

    fn recv(&mut self) -> Value {
        loop {
            if let Message::Text(t) = self.ws.read().expect("hub went quiet") {
                return serde_json::from_str(t.as_str()).unwrap();
            }
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params });
        self.ws.send(Message::text(msg.to_string())).unwrap();
        loop {
            let m = self.recv();
            if m["id"] == json!(self.next) {
                return m;
            }
        }
    }

    fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.ws.send(Message::text(msg.to_string())).unwrap();
    }

    /// Waits for a notification `method`.
    fn notification(&mut self, method: &str) -> Value {
        loop {
            let m = self.recv();
            if m["method"] == method {
                return m["params"].clone();
            }
        }
    }
}

fn item(doc: &str, id: &str, after: &[&str]) -> Value {
    json!({ "document": doc, "event": {
        "id": id, "document": doc, "after": after, "type": if after.is_empty() { "document" } else { "move" },
        "author": { "id": "mailto:ada@example.org" }, "time": "2026-09-29T10:00:00Z", "path": "a.md",
    } })
}

const DOC: &str = "01926d3a-0100-7000-8000-000000000000";
const EV2: &str = "01926d3a-0101-7000-8000-000000000000";

#[test]
fn push_pull_relay_and_dedup() {
    let (base, _data) = start(None);
    let url = format!("{base}/w/paper");
    let mut a = Client::connect(&url, None).unwrap();
    let mut b = Client::connect(&url, None).unwrap();

    assert_eq!(a.call("annoxSync/pull", json!({ "hubId": "x", "after": 0 }))["error"]["code"], -32002, "hello first");
    assert_eq!(a.call("annoxSync/hello", json!({ "format": 99 }))["error"]["code"], 2003);
    let hello = a.call("annoxSync/hello", json!({ "format": 1, "subscribe": true }))["result"].clone();
    assert_eq!(hello["cursor"], 0);
    assert_eq!(hello["writable"], true);
    let hub_id = hello["hubId"].as_str().unwrap().to_owned();
    b.call("annoxSync/hello", json!({ "format": 1, "subscribe": true }));

    let pushed = a.call("annoxSync/push", json!({ "items": [item(DOC, DOC, &[]), item(DOC, EV2, &[DOC])] }));
    assert_eq!(pushed["result"]["results"], json!([{ "seq": 1 }, { "seq": 2 }]));

    // B is told live; A (the pusher) is not.
    let received = b.notification("annoxSync/didReceive");
    assert_eq!(received["cursor"], 2);
    assert_eq!(received["items"][1]["seq"], 2);

    // Same event again: existing seq. Same id, different content: InvalidItem.
    let mut changed = item(DOC, EV2, &[DOC]);
    changed["event"]["path"] = json!("b.md");
    let again = b.call("annoxSync/push", json!({ "items": [item(DOC, EV2, &[DOC]), changed, { "document": "nope" }] }));
    let results = &again["result"]["results"];
    assert_eq!(results[0], json!({ "seq": 2 }));
    assert_eq!(results[1]["error"]["code"], 2005);
    assert_eq!(results[2]["error"]["code"], 2005);

    let page = b.call("annoxSync/pull", json!({ "hubId": hub_id, "after": 1, "limit": 10 }))["result"].clone();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["event"]["id"], EV2);
    assert_eq!(page["cursor"], 2);
    assert_eq!(page["more"], false);
    assert_eq!(b.call("annoxSync/pull", json!({ "hubId": "other", "after": 0 }))["error"]["code"], 2004);
}

#[test]
fn log_survives_restart() {
    let (base, data) = start(None);
    let url = format!("{base}/w/paper");
    let mut a = Client::connect(&url, None).unwrap();
    let hub_id = a.call("annoxSync/hello", json!({ "format": 1 }))["result"]["hubId"].clone();
    a.call("annoxSync/push", json!({ "items": [item(DOC, DOC, &[])] }));

    // A second hub process on the same data directory.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url2 = format!("ws://{}/w/paper", listener.local_addr().unwrap());
    let config = HubConfig { data: data.path().to_path_buf(), token: None };
    std::thread::spawn(move || serve(listener, config));
    let mut c = Client::connect(&url2, None).unwrap();
    let hello = c.call("annoxSync/hello", json!({ "format": 1 }))["result"].clone();
    assert_eq!(hello["hubId"], hub_id);
    assert_eq!(hello["cursor"], 1);
}

#[test]
fn token_and_workspace_path_are_checked() {
    let (base, _data) = start(Some("s3cret"));
    assert!(Client::connect(&format!("{base}/w/paper"), None).is_err());
    assert!(Client::connect(&format!("{base}/w/paper"), Some("wrong")).is_err());
    assert!(Client::connect(&format!("{base}/w/../etc"), Some("s3cret")).is_err());
    assert!(Client::connect(&format!("{base}/other"), Some("s3cret")).is_err());
    assert!(Client::connect(&format!("{base}/w/paper"), Some("s3cret")).is_ok());
}

#[test]
fn presence_is_relayed_and_dropped_on_disconnect() {
    let (base, _data) = start(None);
    let url = format!("{base}/w/paper");
    let mut a = Client::connect(&url, None).unwrap();
    let mut b = Client::connect(&url, None).unwrap();
    a.call("annoxSync/hello", json!({ "format": 1, "subscribe": true }));
    b.call("annoxSync/hello", json!({ "format": 1, "subscribe": true }));
    assert_eq!(b.notification("annoxSync/didChangePresence")["peers"], json!([]));

    a.notify("annoxSync/presence", json!({ "author": { "id": "mailto:ada@example.org" }, "document": DOC, "range": { "start": 3, "end": 3 } }));
    let peers = b.notification("annoxSync/didChangePresence")["peers"].clone();
    assert_eq!(peers[0]["author"]["id"], "mailto:ada@example.org");
    assert_eq!(peers[0]["range"]["start"], 3);
    assert!(peers[0]["connection"].is_string());
    // A doesn't see itself.
    assert_eq!(a.notification("annoxSync/didChangePresence")["peers"], json!([]));

    drop(a);
    assert_eq!(b.notification("annoxSync/didChangePresence")["peers"], json!([]));
}
