//! JSON-RPC 2.0 over a WebSocket: one message per text frame (§7.3).

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Streams whose reads can time out, so one thread can both read and send.
pub trait Timeout {
    fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl Timeout for TcpStream {
    fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.set_read_timeout(timeout)
    }
}

impl Timeout for MaybeTlsStream<TcpStream> {
    fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            MaybeTlsStream::Plain(s) => s.set_read_timeout(timeout),
            MaybeTlsStream::Rustls(s) => s.get_ref().set_read_timeout(timeout),
            _ => Ok(()),
        }
    }
}

pub type RpcError = (i32, String);

#[derive(Debug)]
pub enum Error {
    Closed,
    Io(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Closed => write!(f, "connection closed"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<tungstenite::Error> for Error {
    fn from(e: tungstenite::Error) -> Self {
        match e {
            tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => Error::Closed,
            e => Error::Io(e.to_string()),
        }
    }
}

pub struct Peer<S: Read + Write> {
    ws: WebSocket<S>,
    next_id: u64,
    /// Messages received while waiting for a response.
    inbox: VecDeque<Value>,
}

impl<S: Read + Write> Peer<S> {
    pub fn new(ws: WebSocket<S>) -> Peer<S> {
        Peer { ws, next_id: 0, inbox: VecDeque::new() }
    }

    pub fn send(&mut self, msg: &Value) -> Result<(), Error> {
        self.ws.send(Message::text(msg.to_string())).map_err(Error::from)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), Error> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    pub fn respond(&mut self, id: &Value, result: Result<Value, RpcError>) -> Result<(), Error> {
        let msg = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        };
        self.send(&msg)
    }

    /// The next message, or `None` if the read timed out.
    pub fn recv(&mut self) -> Result<Option<Value>, Error> {
        match self.inbox.pop_front() {
            Some(msg) => Ok(Some(msg)),
            None => self.recv_raw(),
        }
    }

    /// Sends a request and waits for its response. Other messages that arrive
    /// meanwhile are kept for `recv`.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Result<Value, RpcError>, Error> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let mut deferred = VecDeque::new();
        let result = loop {
            let Some(msg) = self.recv_raw()? else { continue };
            if msg["id"] == json!(id) && msg.get("method").is_none() {
                break match msg.get("error") {
                    Some(e) => Err((e["code"].as_i64().unwrap_or(0) as i32, e["message"].as_str().unwrap_or("").to_owned())),
                    None => Ok(msg["result"].clone()),
                };
            }
            deferred.push_back(msg);
        };
        self.inbox.extend(deferred);
        Ok(result)
    }

    fn recv_raw(&mut self) -> Result<Option<Value>, Error> {
        match self.ws.read() {
            Ok(Message::Text(text)) => Ok(serde_json::from_str(text.as_str()).ok()),
            Ok(Message::Close(_)) => Err(Error::Closed),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn close(&mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}
