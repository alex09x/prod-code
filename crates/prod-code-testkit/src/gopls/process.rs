/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

/// The `file://` URI of a path.
pub fn uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

pub(crate) fn disk(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    std::fs::read_dir(root)
        .expect("read_dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "go" || x == "mod"))
        .map(|p| {
            let bytes = std::fs::read(&p).expect("read");
            (p, bytes)
        })
        .collect()
}

/// A real gopls over stdio, read by a thread of its own so that a silent server fails the test
/// instead of hanging it.
pub(crate) struct Gopls {
    _child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next: i64,
    root: PathBuf,
    /// What gopls has been told each file on disk holds.
    seen: BTreeMap<PathBuf, Vec<u8>>,
}

impl Gopls {
    pub(crate) fn start(root: &Path) -> Self {
        let mut child = Command::new("gopls")
            .arg("serve")
            .current_dir(root)
            .env("GOTOOLCHAIN", "local")
            .env("GOFLAGS", "-mod=mod")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("gopls starts");
        let stdin = child.stdin.take().expect("stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let (tx, incoming) = channel();
        std::thread::spawn(move || {
            loop {
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if stdout.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.strip_prefix("Content-Length:") {
                        length = n.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                if stdout.read_exact(&mut body).is_err() {
                    return;
                }
                let Ok(value) = serde_json::from_slice::<Value>(&body) else {
                    continue;
                };
                if tx.send(value).is_err() {
                    return;
                }
            }
        });
        let mut gopls = Self {
            _child: child,
            stdin,
            incoming,
            next: 1,
            root: root.to_path_buf(),
            seen: BTreeMap::new(),
        };
        let uri = url::Url::from_directory_path(root).unwrap().to_string();
        gopls
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": uri,
                    "workspaceFolders": [ { "uri": uri, "name": "fixture" } ],
                    "capabilities": {
                        "workspace": {
                            "workspaceEdit": { "documentChanges": true },
                            "configuration": true,
                            "didChangeWatchedFiles": { "dynamicRegistration": true }
                        },
                        "textDocument": { "rename": { "prepareSupport": true } }
                    }
                }),
            )
            .expect("gopls initializes");
        gopls.send(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
        gopls.seen = disk(root);
        gopls
    }

    pub(crate) fn send(&mut self, message: &Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).expect("write");
        self.stdin.flush().expect("flush");
    }

    /// One request; the server's own requests on the way are answered as an editor would.
    pub(crate) fn request(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        self.tell_about_disk();
        let id = self.next;
        self.next += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self
                .incoming
                .recv_timeout(Duration::from_secs(180))
                .unwrap_or_else(|_| panic!("gopls did not answer {method}"));
            if let (Some(asked), Some(their_id)) = (
                message.get("method").and_then(|m| m.as_str()),
                message.get("id"),
            ) {
                let result = if asked == "workspace/configuration" {
                    let n = message
                        .pointer("/params/items")
                        .and_then(|i| i.as_array())
                        .map_or(0, |i| i.len());
                    Value::Array(vec![json!({}); n])
                } else {
                    Value::Null
                };
                let reply = json!({ "jsonrpc": "2.0", "id": their_id.clone(), "result": result });
                self.send(&reply);
                continue;
            }
            if message.get("id") == Some(&json!(id)) {
                if let Some(error) = message.get("error") {
                    return Err(error.clone());
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    /// gopls does not watch the tree: files an applied change rewrote are announced to it.
    fn tell_about_disk(&mut self) {
        if self.seen.is_empty() {
            return;
        }
        let now = disk(&self.root);
        let mut changes = Vec::new();
        for (path, bytes) in &now {
            match self.seen.get(path) {
                Some(old) if old == bytes => {}
                Some(_) => changes.push((path.clone(), 2)),
                None => changes.push((path.clone(), 1)),
            }
        }
        for path in self.seen.keys().filter(|p| !now.contains_key(*p)) {
            changes.push((path.clone(), 3));
        }
        if changes.is_empty() {
            return;
        }
        let events: Vec<Value> = changes
            .iter()
            .map(|(p, kind)| json!({ "uri": uri(p), "type": kind }))
            .collect();
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": events }
        }));
        self.seen = now;
    }
}
