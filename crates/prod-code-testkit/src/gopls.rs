//! A real gopls behind the scripted gateway, for the Go refactorings' real-server tests.
//!
//! [`GoplsBridge`] is a [`ScriptedGateway`] whose script manufactures no language-server
//! answer: every language request goes to a real `gopls` started on the module, and
//! diagnostics are the real Go compiler's (`go test -run ^$` with an `-overlay` of the texts
//! the session opened). The only thing a test can add is a failure: [`GoplsBridge::fail`]
//! makes one method answer with a JSON-RPC error, for the failures a real server does not
//! produce on demand. It cannot put words in gopls's mouth.
//!
//! `go` and `gopls` are a prerequisite, not an option: [`require_go_toolchain`] fails the test
//! when either is missing. These tests run on provisioned build nodes, and a test that passes
//! because it did not run would be a false green.

use crate::{LSP_ERROR, ScriptedGateway};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn which(binary: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(binary))
            .find(|p| p.is_file())
    })
}

/// Fails the calling test unless `go` and `gopls` are on `PATH`.
pub fn require_go_toolchain() {
    let missing: Vec<&str> = ["go", "gopls"]
        .into_iter()
        .filter(|b| which(b).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "missing prerequisite: {} not on PATH. This real-server test is required and runs on \
         provisioned build nodes; a missing toolchain is a verification gap, not a pass",
        missing.join(" and ")
    );
}

/// A Go module in a directory Go tools do not skip (a `.tmp…` name is hidden from `./...`),
/// committed so the pre-flight sync has a base.
pub struct GoModule {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl GoModule {
    pub fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("gosig")
            .tempdir()
            .expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonical root");
        for (rel, text) in files {
            std::fs::write(root.join(rel), text).expect("write fixture");
        }
        for args in [
            &["init", "-q"][..],
            &["add", "-A"],
            &[
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=test",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            let ok = Command::new("git")
                .args(args)
                .current_dir(&root)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("git runs")
                .success();
            assert!(ok, "git {args:?}");
        }
        Self { _dir: dir, root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).expect("read")
    }

    /// Every path under the module, `.git` aside, and its bytes: a write, a new file or a
    /// deleted one all show.
    pub fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).expect("read_dir") {
                let path = entry.expect("entry").path();
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if rel == ".git" {
                    continue;
                }
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    out.insert(rel, std::fs::read(&path).expect("read"));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &self.root, &mut out);
        out
    }

    /// Runs `go` in the module; the combined output, and whether it succeeded.
    pub fn go(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new("go")
            .args(args)
            .current_dir(&self.root)
            .env("GOTOOLCHAIN", "local")
            .env("GOFLAGS", "-mod=mod")
            .output()
            .expect("go runs");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    }

    /// `go run .`, which must succeed; what the program printed.
    pub fn run(&self) -> String {
        let (ok, text) = self.go(&["run", "."]);
        assert!(ok, "go run fails: {text}");
        text
    }
}

/// A real gopls over stdio, read by a thread of its own so that a silent server fails the test
/// instead of hanging it.
struct Gopls {
    _child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next: i64,
    root: PathBuf,
    /// What gopls has been told each file on disk holds.
    seen: BTreeMap<PathBuf, Vec<u8>>,
}

impl Gopls {
    fn start(root: &Path) -> Self {
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

    fn send(&mut self, message: &Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).expect("write");
        self.stdin.flush().expect("flush");
    }

    /// One request; the server's own requests on the way are answered as an editor would.
    fn request(&mut self, method: &str, params: Value) -> Result<Value, Value> {
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

fn disk(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
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

/// The `file://` URI of a path.
pub fn uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

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
