//! Reordering Go parameters through a real gopls (#448).
//!
//! The gateway here is the scripted one, but nothing it answers is scripted: every language
//! server request goes to a real `gopls` started on the fixture, and diagnostics come from the
//! real Go compiler (`go test -run ^$` with an `-overlay` of the texts the validation opened).
//! The programs are run before and after each change, and what they print — including the
//! order in which their arguments' effects happened — must not change. Build nodes have `go`
//! and `gopls`; without them the tests say so and pass by skipping.

use prod_code_mcp::signature::{Modifiers, Param, SignatureChange};
use prod_code_testkit::{LSP_ERROR, ScriptedGateway};
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

fn toolchain() -> bool {
    if which("gopls").is_none() || which("go").is_none() {
        eprintln!("skipping: gopls or go is not on PATH (build nodes have both)");
        return false;
    }
    true
}

/// A Go module in a directory Go tools do not skip (a `.tmp…` name is hidden from `./...`),
/// committed so the pre-flight sync has a base.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
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

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).expect("read")
    }

    /// Every file of the module and its bytes.
    fn snapshot(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        for entry in std::fs::read_dir(&self.root).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_file() {
                out.insert(
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read(&path).expect("read"),
                );
            }
        }
        out
    }

    /// Runs `go` in the module; the combined output, and whether it succeeded.
    fn go(&self, args: &[&str]) -> (bool, String) {
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

    fn run(&self) -> String {
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

fn uri(path: &Path) -> String {
    url::Url::from_file_path(path).unwrap().to_string()
}

/// The gateway in front of the real gopls: language requests go to gopls, the texts a session
/// opens are kept, and diagnostics are the compiler's for the module with those texts in place.
/// `faults` replaces the answer to a method, for the failures a real server does not produce
/// on demand.
type Overlay = Vec<(PathBuf, String)>;
/// A compiler error: file, line, column, message.
type Located = (PathBuf, u32, u32, String);

struct Bridge {
    gopls: Mutex<Gopls>,
    root: PathBuf,
    open: Mutex<HashMap<PathBuf, String>>,
    compiled: Mutex<HashMap<Overlay, Vec<Located>>>,
    faults: Mutex<HashMap<String, Value>>,
    renames: Mutex<Vec<Value>>,
}

impl Bridge {
    fn answer(&self, method: &str, params: &Value) -> Value {
        if let Some(fault) = self.faults.lock().unwrap().get(method) {
            return fault.clone();
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
                    params.pointer("/textDocument/text").and_then(|t| t.as_str()),
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
        let mut overlay: Vec<(PathBuf, String)> = self
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
                std::fs::canonicalize(dir.path().join(format!("f{i}.go"))).ok() == Some(real.clone())
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

async fn bridge(fixture: &Fixture) -> (Arc<Bridge>, SocketAddr) {
    let bridge = Arc::new(Bridge {
        gopls: Mutex::new(Gopls::start(&fixture.root)),
        root: fixture.root.clone(),
        open: Mutex::new(HashMap::new()),
        compiled: Mutex::new(HashMap::new()),
        faults: Mutex::new(HashMap::new()),
        renames: Mutex::new(Vec::new()),
    });
    let answering = Arc::clone(&bridge);
    let gateway = ScriptedGateway::start(move |method, params| answering.answer(method, params)).await;
    (bridge, gateway.addr())
}

fn keep(names: &[&str]) -> Vec<Param> {
    names.iter().map(|n| Param::Keep(n.to_string())).collect()
}

/// The 1-based position of the first `needle` in the file.
fn at(fixture: &Fixture, rel: &str, needle: &str) -> (u32, u32) {
    let text = fixture.read(rel);
    let offset = text.find(needle).unwrap_or_else(|| panic!("{needle} in {rel}"));
    let before = &text[..offset];
    (
        before.matches('\n').count() as u32 + 1,
        before.rsplit('\n').next().unwrap().chars().count() as u32 + 1,
    )
}

async fn change(
    remote: SocketAddr,
    fixture: &Fixture,
    rel: &str,
    needle: &str,
    order: &[&str],
    apply: bool,
) -> anyhow::Result<SignatureChange> {
    change_with(
        remote,
        fixture,
        rel,
        needle,
        &keep(order),
        &Modifiers::default(),
        apply,
    )
    .await
}

async fn change_with(
    remote: SocketAddr,
    fixture: &Fixture,
    rel: &str,
    needle: &str,
    request: &[Param],
    modifiers: &Modifiers,
    apply: bool,
) -> anyhow::Result<SignatureChange> {
    let (line, col) = at(fixture, rel, needle);
    prod_code_mcp::signature_go::change_with(
        remote,
        &fixture.root,
        &fixture.path(rel),
        line,
        col,
        request,
        modifiers,
        apply,
        false,
    )
    .await
}

fn rewritten<'a>(change: &'a SignatureChange, rel: &str) -> &'a str {
    change
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with(&format!("/{rel}")))
        .map(|(_, t)| t.as_str())
        .unwrap_or_else(|| panic!("{rel} was not rewritten: {:?}", change.rewritten))
}

const SHOP_LIB: &str = r#"package main

import "fmt"

var trace []string

// note records an effect and passes its value on.
func note(tag string, v int) int {
	trace = append(trace, tag)
	return v
}

// Price charges qty items of unit cents less a discount, and records what it was given.
func Price(qty, unit int, discount float64, label string) (total int, err error) {
	trace = append(trace, fmt.Sprintf("Price(%d,%d,%.2f,%s)", qty, unit, discount, label))
	total = int(float64(qty*unit) * (1 - discount))
	return total, nil
}

type Cart struct{ items []string }

// Add puts n copies of name in the cart.
func (c *Cart) Add(name string, n int) {
	for i := 0; i < n; i++ {
		c.items = append(c.items, name)
	}
	trace = append(trace, fmt.Sprintf("Add(%s,%d)", name, n))
}

// Sum adds the values to base.
func Sum(label string, base int, xs ...int) int {
	for _, x := range xs {
		base += x
	}
	trace = append(trace, fmt.Sprintf("Sum(%s,%d)", label, base))
	return base
}
"#;

const SHOP_MAIN: &str = r#"package main

import "fmt"

func main() {
	q, u := 3, 250
	t, _ := Price(q, u, 0.1, "first")
	var c Cart
	c.Add("pear", note("n", 2))
	fmt.Println(t, report(), Sum("s", 1, 2, 3), Sum("t", 0, []int{4, 5}...), len(c.items))
	fmt.Println(trace)
}
"#;

const SHOP_OTHER: &str = r#"package main

func report() int {
	t, _ := Price(2, 100, 0, "second")
	return t
}
"#;

const SHOP_TEST: &str = r#"package main

import "testing"

func TestPrice(t *testing.T) {
	if got, _ := Price(1, 100, 0.5, "test"); got != 50 {
		t.Fatalf("got %d", got)
	}
}
"#;

/// Grouped parameters with named results across four files, a method whose argument calls a
/// function, and a variadic function called with a spread: previewed without a write, then
/// applied, and the program prints exactly what it printed before, effects in the same order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn functions_and_methods_are_reordered_by_gopls_and_run_the_same() {
    if !toolchain() {
        return;
    }
    let fixture = Fixture::new(&[
        ("go.mod", "module example.com/shop\n\ngo 1.22\n"),
        ("lib.go", SHOP_LIB),
        ("main.go", SHOP_MAIN),
        ("other.go", SHOP_OTHER),
        ("main_test.go", SHOP_TEST),
    ]);
    let before = fixture.run();
    eprintln!("original program:\n{before}");
    assert!(
        before.contains("[Price(3,250,0.10,first) n Add(pear,2) Price(2,100,0.00,second)"),
        "{before}"
    );
    let (bridge, remote) = bridge(&fixture).await;

    // A preview writes nothing.
    let untouched = fixture.snapshot();
    let preview = change(
        remote,
        &fixture,
        "lib.go",
        "Price(qty",
        &["label", "unit", "qty", "discount"],
        false,
    )
    .await
    .unwrap_or_else(|e| panic!("the preview runs: {e:#}"));
    assert!(!preview.applied);
    assert_eq!(fixture.snapshot(), untouched, "a preview wrote to the checkout");
    assert!(preview.unmatched.is_empty(), "{:?}", preview.unmatched);
    assert!(preview.unexpected.is_empty(), "{:?}", preview.unexpected);
    assert!(preview.diagnostics.is_empty(), "{:?}", preview.diagnostics);
    assert_eq!(
        preview.old_signature,
        "qty, unit int, discount float64, label string"
    );
    assert_eq!(
        preview.new_signature,
        "label string, unit, qty int, discount float64"
    );
    assert_eq!(preview.rewritten.len(), 4, "{:?}", preview.rewritten);
    assert!(rewritten(&preview, "main.go").contains("Price(\"first\", u, q, 0.1)"));
    assert!(rewritten(&preview, "other.go").contains("Price(\"second\", 100, 2, 0)"));
    assert!(rewritten(&preview, "main_test.go").contains("Price(\"test\", 100, 1, 0.5)"));
    let rename = bridge.renames.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        rename["newName"],
        "func(label string, unit int, qty int, discount float64) (total int,err error)"
    );
    assert_eq!(rename["position"], json!({ "line": 13, "character": 0 }));
    eprintln!("{}", preview.render(4000));

    let done = change(
        remote,
        &fixture,
        "lib.go",
        "Price(qty",
        &["label", "unit", "qty", "discount"],
        true,
    )
    .await
    .unwrap_or_else(|e| panic!("the change applies: {e:#}"));
    assert!(done.applied);
    for (path, text) in &done.rewritten {
        assert_eq!(&std::fs::read_to_string(path).unwrap(), text, "{path}");
    }
    assert!(fixture.read("lib.go").contains(
        "func Price(label string, unit, qty int, discount float64) (total int, err error) {"
    ));

    // A method, whose argument calls a function: the literal beside it has nothing to reorder.
    let method = change(remote, &fixture, "lib.go", "Add(name", &["n", "name"], true)
        .await
        .unwrap_or_else(|e| panic!("the method is reordered: {e:#}"));
    assert!(method.applied);
    assert!(fixture.read("main.go").contains("c.Add(note(\"n\", 2), \"pear\")"));
    assert!(fixture.read("lib.go").contains("func (c *Cart) Add(n int, name string) {"));

    // A variadic function: the fixed parameters move, the variadic one stays last.
    let variadic = change(
        remote,
        &fixture,
        "lib.go",
        "Sum(label",
        &["base", "label", "xs"],
        true,
    )
    .await
    .unwrap_or_else(|e| panic!("the variadic function is reordered: {e:#}"));
    assert!(variadic.applied);
    let main = fixture.read("main.go");
    assert!(main.contains("Sum(1, \"s\", 2, 3)"), "{main}");
    assert!(main.contains("Sum(0, \"t\", []int{4, 5}...)"), "{main}");

    let after = fixture.run();
    eprintln!("transformed program:\n{after}");
    assert_eq!(after, before, "the reordered program prints something else");
    let (tested, output) = fixture.go(&["test", "-count=1", "./..."]);
    assert!(tested, "go test after the change: {output}");
    let (vetted, output) = fixture.go(&["vet", "./..."]);
    assert!(vetted, "go vet after the change: {output}");
}

const REFUSE_LIB: &str = r#"package main

import "fmt"

var trace []string

func mark(tag string, v int) int {
	trace = append(trace, tag)
	return v
}

func Diff(a, b int) int { return a - b }

func Scale(x, y int) int { return x * y }

func Pair[T any, U any](t T, u U) string { return fmt.Sprint(t, u) }

type Adder interface{ Add(x int, y string) }

type Acc struct{ total int }

func (a *Acc) Add(x int, y string) { a.total += x + len(y) }

func Blank(int, string) {}

func Keep(a, b int) int { return a + b }
"#;

const REFUSE_MAIN: &str = r#"package main

import "fmt"

func main() {
	d := Diff(mark("a", 5), mark("b", 2))
	f := Scale
	var acc Acc
	var ad Adder = &acc
	acc.Add(1, "x")
	ad.Add(2, "y")
	Blank(1, "z")
	fmt.Println(d, f(2, 3), Pair(1, "s"), Keep(1, 2), acc.total, trace)
}
"#;

/// Everything that is not a safe reorder of named parameters is refused, says why, keeps the
/// open requirement in view, and leaves every file as it was — the program still runs the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_or_unsafe_changes_are_refused_and_write_nothing() {
    if !toolchain() {
        return;
    }
    let fixture = Fixture::new(&[
        ("go.mod", "module example.com/refuse\n\ngo 1.22\n"),
        ("lib.go", REFUSE_LIB),
        ("main.go", REFUSE_MAIN),
    ]);
    let before = fixture.run();
    eprintln!("original program:\n{before}");
    let untouched = fixture.snapshot();
    let (bridge, remote) = bridge(&fixture).await;
    let open = "remain open requirements";
    let err = |r: anyhow::Result<SignatureChange>| match r {
        Ok(c) => panic!("expected a refusal, got {}", c.render(2000)),
        Err(e) => format!("{e:#}"),
    };

    // gopls itself would swap the two calls: its inliner ignores effects.
    let (line, _) = at(&fixture, "lib.go", "func Diff");
    let native = bridge.gopls.lock().unwrap().request(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri(&fixture.path("lib.go")) },
            "position": { "line": line - 1, "character": 0 },
            "newName": "func(b int, a int) int"
        }),
    );
    eprintln!("gopls's own answer to reordering Diff: {native:?}");

    // Two arguments whose effects would run the other way round.
    let effects = err(change(remote, &fixture, "lib.go", "Diff(a", &["b", "a"], true).await);
    assert!(
        effects.contains("`mark(\"a\", 5)` and `mark(\"b\", 2)` would be evaluated in the opposite order"),
        "{effects}"
    );
    // A function value keeps the old order.
    let value = err(change(remote, &fixture, "lib.go", "Scale(x", &["y", "x"], true).await);
    assert!(value.contains("used as a value") && value.contains("main.go:7:7"), "{value}");
    assert!(value.contains(open), "{value}");
    // gopls refuses a generic function with calls; its reason and the requirement both show.
    let generic = err(change(remote, &fixture, "lib.go", "Pair[T", &["u", "t"], true).await);
    assert!(generic.contains("gopls refused") && generic.contains("generic"), "{generic}");
    assert!(generic.contains("inline") && generic.contains(open), "{generic}");
    // Unnamed parameters cannot be named in a request.
    let unnamed = err(change(remote, &fixture, "lib.go", "Blank(int", &["b", "a"], true).await);
    assert!(unnamed.contains("unnamed") && unnamed.contains(open), "{unnamed}");
    // Removing, adding, changing results, and a position that is no declaration.
    let removed = err(change(remote, &fixture, "lib.go", "Keep(a", &["a"], true).await);
    assert!(removed.contains("removing `b`") && removed.contains(open), "{removed}");
    let mut added = keep(&["b", "a"]);
    added.push(Param::Add {
        name: "c".into(),
        ty: "int".into(),
        value: "0".into(),
    });
    let added = err(change_with(remote, &fixture, "lib.go", "Keep(a", &added, &Modifiers::default(), true).await);
    assert!(added.contains("adding the parameter `c`"), "{added}");
    let results = Modifiers {
        returns: Some("int64".into()),
        ..Default::default()
    };
    let results = err(change_with(remote, &fixture, "lib.go", "Keep(a", &keep(&["b", "a"]), &results, true).await);
    assert!(results.contains("results") && results.contains(open), "{results}");
    let nowhere = err(change(remote, &fixture, "main.go", "fmt.Println", &["b", "a"], true).await);
    assert!(nowhere.contains("not in the header"), "{nowhere}");

    // A method also called through an interface: gopls rewrites the direct call and not the
    // interface's, and the type no longer implements it. The preview says both; applying refuses.
    let preview = change(remote, &fixture, "lib.go", ") Add(x", &["y", "x"], false)
        .await
        .unwrap_or_else(|e| panic!("the preview runs: {e:#}"));
    assert!(
        preview.unmatched.iter().any(|u| u.contains("main.go:11:5")),
        "{:?}",
        preview.unmatched
    );
    assert!(!preview.diagnostics.is_empty(), "the broken interface is not reported");
    eprintln!("{}", preview.render(3000));
    let interface = err(change(remote, &fixture, "lib.go", ") Add(x", &["y", "x"], true).await);
    assert!(interface.contains("not the reorder that was asked for"), "{interface}");

    // A query that fails stops the change: references, gopls's rename, the validation.
    let faults: [(&str, Value, &str); 3] = [
        (
            "textDocument/references",
            json!({ LSP_ERROR: { "code": -32603, "message": "no package metadata" } }),
            "cannot list the references",
        ),
        (
            "textDocument/rename",
            json!({ LSP_ERROR: { "code": -32603, "message": "renaming is broken today" } }),
            "renaming is broken today",
        ),
        (
            "textDocument/diagnostic",
            json!({ LSP_ERROR: { "code": -32603, "message": "no diagnostics" } }),
            "could not be validated",
        ),
    ];
    for (method, answer, said) in faults {
        bridge
            .faults
            .lock()
            .unwrap()
            .insert(method.to_string(), answer);
        let failed = err(change(remote, &fixture, "lib.go", "Keep(a", &["b", "a"], true).await);
        assert!(failed.contains(said), "{method}: {failed}");
        bridge.faults.lock().unwrap().clear();
    }
    // An edit to a file outside the checkout.
    let elsewhere = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
    let stray = std::fs::canonicalize(elsewhere.path()).unwrap().join("x.go");
    std::fs::write(&stray, "package x\n").unwrap();
    bridge.faults.lock().unwrap().insert(
        "textDocument/rename".to_string(),
        json!({ "documentChanges": [ {
            "textDocument": { "uri": uri(&stray), "version": 1 },
            "edits": [ { "range": { "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 7 } }, "newText": "package" } ]
        } ] }),
    );
    let outside = err(change(remote, &fixture, "lib.go", "Keep(a", &["b", "a"], true).await);
    assert!(outside.contains("outside the checkout"), "{outside}");
    bridge.faults.lock().unwrap().clear();

    assert_eq!(fixture.snapshot(), untouched, "a refused change wrote to the checkout");
    assert_eq!(std::fs::read_to_string(&stray).unwrap(), "package x\n");
    let after = fixture.run();
    assert_eq!(after, before);

    // And the same function, asked properly, is reordered.
    let fine = change(remote, &fixture, "lib.go", "Keep(a", &["b", "a"], true)
        .await
        .unwrap_or_else(|e| panic!("a plain reorder applies: {e:#}"));
    assert!(fine.applied);
    assert!(fixture.read("main.go").contains("Keep(2, 1)"));
    assert_eq!(fixture.run(), before);
}
