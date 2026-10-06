/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::module::GoModule;
use super::process::Gopls;
use crate::{LSP_ERROR, ScriptedGateway};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// The texts one validation opened, sorted: the key of a compiler run.
type Overlay = Vec<(PathBuf, String)>;
/// A compiler error: file, line, column, message.
type Located = (PathBuf, u32, u32, String);

/// The gateway in front of the real gopls: language requests go to gopls, the texts a session
/// opens are kept, and diagnostics are the compiler's for the module with those texts in place.
pub struct GoplsBridge {
    gopls: Mutex<Gopls>,
    root: PathBuf,
    open: Mutex<HashMap<PathBuf, String>>,
    compiled: Mutex<HashMap<Overlay, Vec<Located>>>,
    failures: Mutex<HashMap<String, String>>,
    renames: Mutex<Vec<Value>>,
    addr: SocketAddr,
}

impl GoplsBridge {
    /// Starts gopls on `module` and a gateway in front of it; `addr()` is where to point a tool
    /// or the CLI's `--remote`.
    pub async fn start(module: &GoModule) -> Arc<Self> {
        let gopls = Gopls::start(module.root());
        let holder: Arc<Mutex<Option<Arc<GoplsBridge>>>> = Arc::new(Mutex::new(None));
        let answering = Arc::clone(&holder);
        let gateway = ScriptedGateway::start(move |method, params| {
            let bridge = answering.lock().unwrap().clone();
            match bridge {
                Some(bridge) => bridge.answer(method, params),
                None => Value::Null,
            }
        })
        .await;
        let bridge = Arc::new(GoplsBridge {
            gopls: Mutex::new(gopls),
            root: module.root().to_path_buf(),
            open: Mutex::new(HashMap::new()),
            compiled: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            renames: Mutex::new(Vec::new()),
            addr: gateway.addr(),
        });
        *holder.lock().unwrap() = Some(Arc::clone(&bridge));
        bridge
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Asks gopls directly, past the gateway: what the server itself says.
    pub fn native(&self, method: &str, params: Value) -> Result<Value, Value> {
        self.gopls.lock().unwrap().request(method, params)
    }

    /// Every `textDocument/rename` gopls was asked through the gateway, in order.
    pub fn renames(&self) -> Vec<Value> {
        self.renames.lock().unwrap().clone()
    }

    /// Makes `method` fail with a JSON-RPC internal error carrying `message`, until
    /// [`GoplsBridge::heal`].
    pub fn fail(&self, method: &str, message: &str) {
        self.failures
            .lock()
            .unwrap()
            .insert(method.to_string(), message.to_string());
    }

    /// Takes every injected failure away.
    pub fn heal(&self) {
        self.failures.lock().unwrap().clear();
    }

    fn answer(&self, method: &str, params: &Value) -> Value {
        if let Some(message) = self.failures.lock().unwrap().get(method) {
            return json!({ LSP_ERROR: { "code": -32603, "message": message } });
        }
        let path_of = |params: &Value| {
            params
                .pointer("/textDocument/uri")
                .and_then(|u| u.as_str())
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.to_file_path().ok())
        };
        match method {
            "textDocument/didOpen" => {
                if let (Some(p), Some(t)) = (
                    path_of(params),
                    params
                        .pointer("/textDocument/text")
                        .and_then(|t| t.as_str()),
                ) {
                    self.open.lock().unwrap().insert(p, t.to_string());
                }
                Value::Null
            }
            "textDocument/didChange" => {
                if let (Some(p), Some(t)) = (
                    path_of(params),
                    params
                        .pointer("/contentChanges/0/text")
                        .and_then(|t| t.as_str()),
                ) {
                    self.open.lock().unwrap().insert(p, t.to_string());
                }
                Value::Null
            }
            "textDocument/didClose" => {
                if let Some(p) = path_of(params) {
                    self.open.lock().unwrap().remove(&p);
                }
                Value::Null
            }
            "textDocument/diagnostic" => match path_of(params) {
                Some(file) => self.diagnostics(&file),
                None => json!({ "kind": "full", "items": [] }),
            },
            m if m.starts_with("prod-code/") || !m.contains('/') => Value::Null,
            m if m.starts_with("$/") || m.starts_with("workspace/didChange") => Value::Null,
            _ => {
                if method == "textDocument/rename" {
                    self.renames.lock().unwrap().push(params.clone());
                }
                match self.gopls.lock().unwrap().request(method, params.clone()) {
                    Ok(result) => result,
                    Err(error) => json!({ LSP_ERROR: error }),
                }
            }
        }
    }

    /// The compiler's errors for `file` with every opened text that differs from the disk in
    /// place, test files included.
    fn diagnostics(&self, file: &Path) -> Value {
        let mut overlay: Overlay = self
            .open
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, t)| std::fs::read_to_string(p).ok().as_deref() != Some(t.as_str()))
            .map(|(p, t)| (p.clone(), t.clone()))
            .collect();
        overlay.sort();
        let errors = {
            let mut cache = self.compiled.lock().unwrap();
            cache
                .entry(overlay.clone())
                .or_insert_with(|| compile(&self.root, &overlay))
                .clone()
        };
        let items: Vec<Value> = errors
            .iter()
            .filter(|(p, ..)| p == file)
            .map(|(_, l, c, m)| {
                json!({
                    "range": {
                        "start": { "line": l - 1, "character": c - 1 },
                        "end": { "line": l - 1, "character": c }
                    },
                    "severity": 1,
                    "source": "compiler",
                    "message": m
                })
            })
            .collect();
        json!({ "kind": "full", "items": items })
    }
}

/// `go test -run ^$` over the module with `overlay` in place: every package and test file
/// type-checked, nothing run. The errors, as (file, line, column, message).
fn compile(root: &Path, overlay: &[(PathBuf, String)]) -> Vec<Located> {
    let dir = tempfile::Builder::new()
        .prefix("gosigoverlay")
        .tempdir()
        .expect("overlay dir");
    let mut replace = serde_json::Map::new();
    for (i, (path, text)) in overlay.iter().enumerate() {
        let copy = dir.path().join(format!("f{i}.go"));
        std::fs::write(&copy, text).expect("overlay file");
        replace.insert(
            path.display().to_string(),
            Value::String(copy.display().to_string()),
        );
    }
    let spec = dir.path().join("overlay.json");
    std::fs::write(&spec, json!({ "Replace": replace }).to_string()).expect("overlay spec");
    let out = Command::new("go")
        .args([
            "test",
            &format!("-overlay={}", spec.display()),
            "-count=1",
            "-run",
            "^$",
            "./...",
        ])
        .current_dir(root)
        .env("GOTOOLCHAIN", "local")
        .env("GOFLAGS", "-mod=mod")
        .output()
        .expect("go test runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut errors = Vec::new();
    for line in text.lines() {
        let mut parts = line.trim().splitn(4, ':');
        let (Some(f), Some(l), Some(c), Some(m)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(l), Ok(c)) = (l.parse::<u32>(), c.parse::<u32>()) else {
            continue;
        };
        if !f.ends_with(".go") {
            continue;
        }
        let path = if Path::new(f).is_absolute() {
            PathBuf::from(f)
        } else {
            root.join(f.trim_start_matches("./"))
        };
        // An error in a replaced file is reported under the replacement's own path.
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let path = overlay
            .iter()
            .enumerate()
            .find(|(i, _)| {
                std::fs::canonicalize(dir.path().join(format!("f{i}.go"))).ok()
                    == Some(real.clone())
            })
            .map_or(path, |(_, (original, _))| original.clone());
        errors.push((path, l, c, m.trim().to_string()));
    }
    eprintln!(
        "compiled with {} replaced file(s): {} error(s){}",
        overlay.len(),
        errors.len(),
        if out.status.success() {
            String::new()
        } else {
            format!("\n{text}")
        }
    );
    if !out.status.success() {
        assert!(
            !errors.is_empty(),
            "go test failed without a located error: {text}"
        );
    }
    errors
}
