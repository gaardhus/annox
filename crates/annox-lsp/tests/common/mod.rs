//! A minimal LSP client for driving the server over an in-memory connection.

#![allow(dead_code)]

use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use serde_json::Value;

pub struct Client {
    pub conn: Connection,
    pub server: Option<std::thread::JoinHandle<()>>,
    next: i32,
}

impl Client {
    /// Starts a server thread and runs `initialize` with `params`.
    pub fn start(params: Value) -> (Client, Value) {
        let (server_conn, client_conn) = Connection::memory();
        let server = std::thread::spawn(move || annox_lsp::run(&server_conn).unwrap());
        let mut client = Client { conn: client_conn, server: Some(server), next: 0 };
        let init = client.request("initialize", params);
        let result = client.response(&init).unwrap();
        client.notify("initialized", serde_json::json!({}));
        (client, result)
    }

    pub fn request(&mut self, method: &str, params: Value) -> RequestId {
        self.next += 1;
        let id = RequestId::from(self.next);
        self.conn.sender.send(Request::new(id.clone(), method.into(), params).into()).unwrap();
        id
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.conn.sender.send(Notification::new(method.into(), params).into()).unwrap();
    }

    /// Receives messages until `pick` returns something.
    pub fn recv_until<T>(&self, mut pick: impl FnMut(Message) -> Option<T>) -> T {
        loop {
            let msg = self.conn.receiver.recv_timeout(Duration::from_secs(10)).expect("server went quiet");
            if let Some(t) = pick(msg) {
                return t;
            }
        }
    }

    pub fn response(&self, id: &RequestId) -> Result<Value, lsp_server::ResponseError> {
        self.recv_until(|m| match m {
            Message::Response(r) if &r.id == id => Some(r.response_result),
            _ => None,
        })
    }

    /// Sends a request and waits for its successful result.
    pub fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.request(method, params);
        self.response(&id).unwrap_or_else(|e| panic!("{method} failed: {e:?}"))
    }

    /// Sends a request and waits for its error code.
    pub fn call_err(&mut self, method: &str, params: Value) -> i32 {
        let id = self.request(method, params);
        self.response(&id).expect_err("expected an error").code
    }

    pub fn notification(&self, method: &str) -> Value {
        self.recv_until(|m| match m {
            Message::Notification(n) if n.method == method => Some(n.params),
            _ => None,
        })
    }

    /// Waits for a `workspace/applyEdit` request and answers it.
    pub fn answer_apply_edit(&self, applied: bool) -> Value {
        let req = self.recv_until(|m| match m {
            Message::Request(r) if r.method == "workspace/applyEdit" => Some(r),
            _ => None,
        });
        let answer = serde_json::json!({ "applied": applied });
        self.conn.sender.send(Response::new_ok(req.id, answer).into()).unwrap();
        req.params
    }

    pub fn shutdown(mut self) {
        let id = self.request("shutdown", Value::Null);
        self.response(&id).unwrap();
        self.notify("exit", Value::Null);
        self.server.take().unwrap().join().unwrap();
    }
}
