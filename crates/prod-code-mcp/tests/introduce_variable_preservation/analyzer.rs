/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A real analyzer child, joined on drop. Diagnostics are deliberately not the correctness
//! oracle: the programs are separately compiled and executed by the preservation harness.

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

pub struct Analyzer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    id: u32,
}

impl Analyzer {
    pub fn start(file: &Path, source: &str) -> Self {
        let mut child = Command::new("rust-analyzer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("installed rust-analyzer");
        let mut analyzer = Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            id: 0,
        };
        let root = file.parent().unwrap().parent().unwrap();
        let root_uri = url::Url::from_file_path(root).unwrap().to_string();
        analyzer.request("initialize", json!({
            "processId": std::process::id(), "rootUri": root_uri,
            "workspaceFolders": [{"uri":root_uri,"name":"preservation_fixture"}],
            "capabilities": {"experimental":{"serverStatusNotification":true},
                "textDocument":{"hover":{"contentFormat":["markdown"]}}}, "initializationOptions": {
                "checkOnSave": false,
                "cargo": {"buildScripts": {"enable": false}},
                "procMacro": {"enable": false}
            }
        }));
        analyzer.send(json!({"jsonrpc":"2.0", "method":"initialized", "params":{}}));
        analyzer.send(
            json!({"jsonrpc":"2.0", "method":"textDocument/didOpen", "params":{
                "textDocument": {"uri": url::Url::from_file_path(file).unwrap().as_str(),
                    "languageId":"rust", "version":1, "text":source}
            }}),
        );
        loop {
            let value = analyzer.receive();
            if value["method"] == "experimental/serverStatus"
                && value["params"]["quiescent"] == true
            {
                break;
            }
            analyzer.answer_server_request(&value);
        }
        analyzer
    }

    fn send(&mut self, value: Value) {
        let text = value.to_string();
        write!(self.input, "Content-Length: {}\r\n\r\n{}", text.len(), text).unwrap();
        self.input.flush().unwrap();
    }

    fn receive(&mut self) -> Value {
        let mut len = None;
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).unwrap() > 0,
                "analyzer exited"
            );
            if line == "\r\n" {
                break;
            }
            if let Some(n) = line.strip_prefix("Content-Length:") {
                len = Some(n.trim().parse::<usize>().unwrap());
            }
        }
        let mut bytes = vec![0; len.expect("LSP content length")];
        self.output.read_exact(&mut bytes).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        self.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}));
        loop {
            let value = self.receive();
            if value.get("method").is_some() {
                self.answer_server_request(&value);
            } else if value.get("id").and_then(Value::as_u64) == Some(u64::from(id)) {
                assert!(value.get("error").is_none(), "{method}: {value}");
                return value["result"].clone();
            }
        }
    }

    fn answer_server_request(&mut self, value: &Value) {
        if let Some(id) = value.get("id") {
            let result = if value["method"] == "workspace/configuration" {
                json!(vec![
                    Value::Null;
                    value["params"]["items"].as_array().map_or(0, Vec::len)
                ])
            } else {
                Value::Null
            };
            self.send(json!({"jsonrpc":"2.0", "id":id, "result":result}));
        }
    }
}

impl Drop for Analyzer {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().expect("join analyzer child");
    }
}
