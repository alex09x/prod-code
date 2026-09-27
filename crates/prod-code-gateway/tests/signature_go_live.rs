//! `code_change_signature` removing unused Go parameters, against the gateway built from this
//! revision and the real gopls it supervises, over a real socket, through the tool call an agent
//! makes.
//!
//! The adapter's scripted-gateway tests put a real gopls behind a stand-in gateway; this one puts
//! nothing in between: the checkout is synced to the daemon, gopls loads it there, and the
//! validation is the gateway's own. The program and its tests are run before and after, and
//! must print the same. `go` and `gopls` are required: without them this test fails rather than
//! passing without having run.
//!
//! The gateway binds port 0 and reports the address it bound, so no two runs share a port.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A gateway process and its storage, stopped when dropped.
struct Gateway {
    child: Child,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
}

impl Gateway {
    fn start() -> Self {
        let storage = tempfile::tempdir().expect("storage dir");
        let child = Command::new(env!("CARGO_BIN_EXE_prod-code-server"))
            .env("PROD_CODE_STORAGE", storage.path())
            // No peers, no gossip: this gateway is alone and must not look for others.
            .env("PROD_CODE_PEERS", "")
            .env("PROD_CODE_BIND", "127.0.0.1:0")
            .env("RUST_LOG", "info")
            .env("GOFLAGS", "-tags=prodcode_signature -mod=readonly")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the server binary starts");
        // Owned before the address is read, so a gateway that never reports one is stopped.
        let mut gateway = Self {
            child,
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            _storage: storage,
        };
        let stdout = gateway.child.stdout.take().expect("stdout is piped");
        gateway.addr = bound_address(stdout);
        gateway
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        // SIGTERM first, so the server returns from `main` and a coverage profile is written.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The address the daemon logs as bound, read on a thread that drains the log for the life of
/// the process.
fn bound_address(stdout: std::process::ChildStdout) -> SocketAddr {
    use std::io::BufRead;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        let mut found = false;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if !found
                && let Some(rest) = line.split("listening on ").nth(1)
                && let Ok(addr) = rest.trim().parse::<SocketAddr>()
            {
                found = true;
                let _ = tx.send(addr);
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(60))
        .expect("the gateway says where it is listening within a minute")
}

const LIB: &str = r#"package main

import "fmt"

var trace []string

// mark records an effect and passes its value on.
func mark(tag string, v int) int {
	trace = append(trace, tag)
	return v
}

// Ship sends qty items to dest; the note and the priority are no longer read.
func Ship(qty int, note string, priority int, dest string) string {
	trace = append(trace, fmt.Sprintf("Ship(%d,%s)", qty, dest))
	return fmt.Sprint(qty, "->", dest)
}

// Drop never reads b; its caller passes an effect for it.
func Drop(a, b int) int { return a }
"#;

const MAIN: &str = r#"package main

import "fmt"

func main() {
	note, prio := "fragile", 2
	fmt.Println(Ship(3, "glass", 1, "LA"), Ship(mark("m", 2), note, prio, "NY")) // Ship(1, "x", 2, "y")
	fmt.Println(Drop(1, mark("call", 2)), note, prio, trace)
}
"#;

const TEST: &str = r#"package main

import "testing"

func TestShip(t *testing.T) {
	if got := Ship(2, "n", 0, "SF"); got != "2->SF" {
		t.Fatalf("got %q", got)
	}
	t.Log(trace)
}
"#;

const RECEIVER_LIB: &str = r#"package main

import "fmt"

var trace []string

type Meter struct { total int }

type Unrelated interface { NotAdd(int) string }
const interfaceDocumentation = `interface { Add(int) string }`

func mark(tag string, value int) int {
	trace = append(trace, tag)
	return value
}

func markMeter(tag string, meter *Meter) *Meter {
	trace = append(trace, tag)
	return meter
}

func (meter Meter) Add(qty int) string {
	meter.total += qty
	trace = append(trace, fmt.Sprintf("Add(%d)", meter.total))
	return fmt.Sprint(meter.total)
}

func (meter *Meter) Scale(qty int) string {
	meter.total *= qty
	trace = append(trace, fmt.Sprintf("Scale(%d)", meter.total))
	return fmt.Sprint(meter.total)
}
"#;

const RECEIVER_MAIN: &str = r#"package main

import "fmt"

func main() {
	meter := Meter{total: 3}
	fmt.Println(markMeter("value receiver", &meter).Add(mark("value argument", 2)))
	fmt.Println(markMeter("pointer receiver", &meter).Scale(mark("pointer argument", 4)))
	fmt.Println(trace)
}
"#;

const RECEIVER_TEST: &str = r#"package main

import (
	"fmt"
	"testing"
)

func TestReceiverMethods(t *testing.T) {
	trace = nil
	meter := Meter{total: 2}
	if got := meter.Add(3); got != "5" { t.Fatalf("Add = %s", got) }
	if got := (&meter).Scale(2); got != "4" { t.Fatalf("Scale = %s", got) }
	if got, want := fmt.Sprint(trace), "[Add(5) Scale(4)]"; got != want { t.Fatalf("trace = %s, want %s", got, want) }
}
"#;

/// A committed checkout whose Go module is nested below the repository root.
fn checkout() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("gosiglive")
        .tempdir()
        .expect("checkout dir");
    for (rel, text) in [
        ("go.work", "go 1.22\n\nuse ./project\n"),
        (
            "project/go.mod",
            "module example.com/gosiglive\n\ngo 1.22\n",
        ),
        (
            "project/tag_enabled.go",
            "//go:build prodcode_signature\n\npackage main\nconst requiredBuildTag = 1\n",
        ),
        (
            "project/tag_required.go",
            "package main\nvar _ = requiredBuildTag\n",
        ),
        ("project/lib.go", LIB),
        ("project/main.go", MAIN),
        ("project/main_test.go", TEST),
    ] {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "user.name=test",
        "commit",
        "-qm",
        "fixture",
    ]);
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    (dir, root)
}

fn receiver_checkout() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("gosigreceiver")
        .tempdir()
        .expect("checkout dir");
    for (rel, text) in [
        ("go.work", "go 1.22\n\nuse ./project\n"),
        (
            "project/go.mod",
            "module example.com/gosigreceiver\n\ngo 1.22\n",
        ),
        (
            "project/tag_enabled.go",
            "//go:build prodcode_signature\n\npackage main\nconst requiredBuildTag = 1\n",
        ),
        (
            "project/tag_required.go",
            "package main\nvar _ = requiredBuildTag\n",
        ),
        ("project/lib.go", RECEIVER_LIB),
        ("project/main.go", RECEIVER_MAIN),
        ("project/main_test.go", RECEIVER_TEST),
    ] {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&[
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "user.name=test",
        "commit",
        "-qm",
        "fixture",
    ]);
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    (dir, root)
}

/// Every file of the checkout but `.git`, and its bytes.
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("directory entry").path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .expect("file below root")
                        .to_string_lossy()
                        .into_owned(),
                    std::fs::read(path).expect("read"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

/// Runs a command in the checkout; whether it succeeded, and what it printed.
fn run(root: &Path, program: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(program)
        .args(args)
        .current_dir(root)
        .env("GOTOOLCHAIN", "local")
        .env("GOFLAGS", "-tags=prodcode_signature -mod=readonly")
        .output()
        .unwrap_or_else(|e| {
            panic!("{program} is a prerequisite of this real-server test and does not run: {e}")
        });
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// What the program prints, and what its tests report without their timings.
fn behaviour(root: &Path) -> (String, String) {
    let (ran, program) = run(root, "go", &["run", "."]);
    assert!(ran, "go run: {program}");
    let (tested, tests) = run(root, "go", &["test", "-v", "-count=1", "./..."]);
    assert!(tested, "go test: {tests}");
    let tests = tests
        .lines()
        .map(|l| match l.find(" (") {
            Some(cut) if l.starts_with("--- ") => &l[..cut],
            _ if l.starts_with("ok ") => "ok",
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (program, tests)
}

/// The text of a tool call, or its error.
async fn tool(addr: SocketAddr, root: &Path, name: &str, args: serde_json::Value) -> String {
    match prod_code_mcp::tools::execute_tool(addr, root, name, args).await {
        Ok(result) => {
            let text = result
                .content
                .iter()
                .map(|item| {
                    let prod_code_mcp::protocol::McpContentItem::Text { text } = item;
                    text.clone()
                })
                .collect::<Vec<_>>()
                .join("\n");
            if result.is_error {
                format!("error: {text}")
            } else {
                text
            }
        }
        Err(e) => format!("error: {e:#}"),
    }
}

const SENTINEL_CHILD: &str = "PROD_CODE_GO_SENTINEL_CHILD";
const SENTINEL_ROOT: &str = "PROD_CODE_GO_SENTINEL_ROOT";
const SENTINEL_ADDR: &str = "PROD_CODE_GO_SENTINEL_ADDR";

/// This runs in an explicitly spawned test process whose environment contains the failing
/// client-only `go`. The parent has already started the real gateway with its normal toolchain.
async fn client_sentinel_scenario() {
    let root = PathBuf::from(std::env::var_os(SENTINEL_ROOT).expect("sentinel fixture root"));
    let addr = std::env::var(SENTINEL_ADDR)
        .expect("sentinel gateway address")
        .parse()
        .expect("sentinel gateway address parses");
    let before_refusal = snapshot(&root);
    let refused_addition = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({
            "symbol": "Ship",
            "params": ["dest", "qty", "bad: string = 1"],
            "apply": true,
            "force": true
        }),
    )
    .await;
    assert!(
        refused_addition.starts_with("error: ") && refused_addition.contains("does not compile"),
        "{refused_addition}"
    );
    assert_eq!(snapshot(&root), before_refusal, "compiler refusal wrote");

    let added = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({
            "symbol": "Ship",
            "params": ["dest", "qty", "route: string = \"road,air\""],
            "apply": true,
            "force": true
        }),
    )
    .await;
    assert!(
        !added.starts_with("error: ") && added.contains("[applied to 3 file(s)]"),
        "{added}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unused_go_parameters_are_removed_through_the_real_gateway_and_gopls() {
    if std::env::var_os(SENTINEL_CHILD).is_some() {
        client_sentinel_scenario().await;
        return;
    }
    let (ok, version) = run(Path::new("."), "gopls", &["version"]);
    assert!(ok, "gopls version: {version}");
    eprintln!("gopls on this node: {}", version.trim());
    let gateway = Gateway::start();
    let (_dir, root) = checkout();
    let project = root.join("project");
    let addr = gateway.addr;
    eprintln!("gateway from this revision listening on {addr}");
    let before = behaviour(&project);
    eprintln!(
        "original program:\n{}\noriginal tests:\n{}",
        before.0, before.1
    );
    assert!(
        before.0.contains("3->LA 2->NY") && before.1.contains("--- PASS: TestShip"),
        "{before:?}"
    );
    let untouched = snapshot(&root);

    // Until gopls has loaded the module its answers are empty; the hover says when it has.
    let at_ship = serde_json::json!({ "path": "project/lib.go", "line": 14, "character": 6 });
    let mut hover = String::new();
    for attempt in 1..=60 {
        hover = tool(addr, &root, "code_hover", at_ship.clone()).await;
        eprintln!(
            "hover attempt {attempt}: {}",
            hover.lines().next().unwrap_or_default()
        );
        if hover.contains("func Ship") {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(
        hover.contains("func Ship"),
        "gopls loaded the module: {hover}"
    );

    // A preview of removing `note` and `priority` and swapping the rest writes nothing.
    let order = serde_json::json!(["dest", "qty"]);
    let preview = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({ "symbol": "Ship", "params": order }),
    )
    .await;
    eprintln!("preview:\n{preview}");
    assert!(
        !preview.starts_with("error: ")
            && preview.contains("- was: (qty int, note string, priority int, dest string)")
            && preview.contains("- now: (dest string, qty int)")
            && preview.contains("nothing was written"),
        "{preview}"
    );
    assert_eq!(snapshot(&root), untouched, "a preview wrote");

    // Dropping `mark("call", 2)` would stop the program from running it: refused with `force`,
    // and nothing is written.
    let refused = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({ "symbol": "Drop", "params": ["a"], "apply": true, "force": true }),
    )
    .await;
    eprintln!("refused removal:\n{refused}");
    assert!(
        refused.starts_with("error: ")
            && refused.contains(
                "`mark(\"call\", 2)` is passed for the removed `b` and would no longer be evaluated"
            ),
        "{refused}"
    );
    assert_eq!(snapshot(&root), untouched, "a refused removal wrote");

    // Applied by position: every call rewritten on disk, comments and strings as they were.
    let applied = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({
            "path": "project/lib.go", "line": 14, "character": 1, "params": order, "apply": true
        }),
    )
    .await;
    eprintln!("applied:\n{applied}");
    assert!(
        !applied.starts_with("error: ") && applied.contains("[applied to 3 file(s)]"),
        "{applied}"
    );
    let lib = std::fs::read_to_string(project.join("lib.go")).expect("lib.go");
    let main = std::fs::read_to_string(project.join("main.go")).expect("main.go");
    let test = std::fs::read_to_string(project.join("main_test.go")).expect("main_test.go");
    eprintln!("transformed lib.go:\n{lib}\ntransformed main.go:\n{main}");
    assert!(
        lib.contains("func Ship(dest string, qty int) string {"),
        "{lib}"
    );
    assert!(
        main.contains(
            "fmt.Println(Ship(\"LA\", 3), Ship(\"NY\", mark(\"m\", 2))) // Ship(1, \"x\", 2, \"y\")"
        ),
        "{main}"
    );
    assert!(test.contains("Ship(\"SF\", 2)"), "{test}");
    assert_eq!(
        lib.replace("func Ship(dest string, qty int) string {", ""),
        LIB.replace(
            "func Ship(qty int, note string, priority int, dest string) string {",
            ""
        ),
        "only the parameter list of the declaration changed"
    );

    // The gateway inherited the real toolchain. Replace only the client's `go` with a failing
    // sentinel: remote compiler verification must neither find nor execute this program.
    let sentinel = tempfile::tempdir().expect("sentinel dir");
    let marker = sentinel.path().join("invoked");
    let fake_go = sentinel.path().join("go");
    std::fs::write(
        &fake_go,
        "#!/bin/sh\nprintf invoked > \"$PROD_CODE_GO_SENTINEL\"\nexit 97\n",
    )
    .expect("write sentinel");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_go, std::fs::Permissions::from_mode(0o755))
            .expect("chmod sentinel");
    }
    let original_path = std::env::var_os("PATH").expect("PATH");
    let client_path = std::env::join_paths(
        std::iter::once(sentinel.path().to_path_buf()).chain(std::env::split_paths(&original_path)),
    )
    .expect("client PATH");
    // The child alone sees the failing client `go`; it talks to this already-running gateway,
    // whose inherited toolchain remains real. No process-global test environment is mutated.
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "unused_go_parameters_are_removed_through_the_real_gateway_and_gopls",
            "--nocapture",
        ])
        .env("PATH", client_path)
        .env("PROD_CODE_GO_SENTINEL", &marker)
        .env(SENTINEL_CHILD, "1")
        .env(SENTINEL_ROOT, &root)
        .env(SENTINEL_ADDR, addr.to_string())
        .status()
        .expect("sentinel child starts");
    assert!(status.success(), "client sentinel child failed: {status}");
    assert!(
        !marker.exists(),
        "the client invoked its failing Go sentinel"
    );
    let lib = std::fs::read_to_string(project.join("lib.go")).expect("lib.go");
    let main = std::fs::read_to_string(project.join("main.go")).expect("main.go");
    let test = std::fs::read_to_string(project.join("main_test.go")).expect("main_test.go");
    assert!(
        lib.contains("func Ship(dest string, qty int, route string) string {"),
        "{lib}"
    );
    assert!(main.contains("Ship(\"LA\", 3, \"road,air\")"), "{main}");
    assert!(test.contains("Ship(\"SF\", 2, \"road,air\")"), "{test}");
    let after = behaviour(&project);
    eprintln!(
        "transformed program:\n{}\ntransformed tests:\n{}",
        after.0, after.1
    );
    assert_eq!(
        after, before,
        "the changed program or its tests run differently"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receiver_method_additions_use_only_direct_selector_calls() {
    let (ok, version) = run(Path::new("."), "gopls", &["version"]);
    assert!(ok, "gopls version: {version}");
    let gateway = Gateway::start();
    let (_dir, root) = receiver_checkout();
    let project = root.join("project");
    let before = behaviour(&project);
    assert_eq!(
        before.0,
        "5\n12\n[value receiver value argument Add(5) pointer receiver pointer argument Scale(12)]\n"
    );
    let untouched = snapshot(&root);
    let addr = gateway.addr;

    let add = RECEIVER_LIB.find("Add(qty").expect("Add definition");
    let row_start = RECEIVER_LIB[..add].rfind('\n').map_or(0, |i| i + 1);
    let at_add = serde_json::json!({
        "path": "project/lib.go", "line": RECEIVER_LIB[..add].matches('\n').count() + 1,
        "character": RECEIVER_LIB[row_start..add].encode_utf16().count() + 1
    });
    let mut hover = String::new();
    for _ in 1..=60 {
        hover = tool(addr, &root, "code_hover", at_add.clone()).await;
        if hover.contains("Add") {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(hover.contains("Add"), "gopls loaded: {hover}");

    let preview = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({ "symbol": "Add", "params": ["qty", "note: string = \"road\""] }),
    )
    .await;
    assert!(
        !preview.contains("adding parameters to the receiver method `Add` is not supported"),
        "receiver additions are supported: {preview}"
    );
    assert!(!preview.starts_with("error: "), "preview: {preview}");
    assert_eq!(snapshot(&root), untouched, "a receiver preview wrote");

    let applied = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({ "symbol": "Add", "params": ["qty", "note: string = \"road\""], "apply": true }),
    )
    .await;
    assert!(!applied.starts_with("error: "), "apply: {applied}");
    let lib = std::fs::read_to_string(project.join("lib.go")).expect("lib");
    let main = std::fs::read_to_string(project.join("main.go")).expect("main");
    let test = std::fs::read_to_string(project.join("main_test.go")).expect("test");
    assert!(
        lib.contains("func (meter Meter) Add(qty int, note string) string"),
        "{lib}"
    );
    assert!(
        main.contains("Add(mark(\"value argument\", 2), \"road\")"),
        "{main}"
    );
    assert!(test.contains("meter.Add(3, \"road\")"), "{test}");

    let pointer_preview = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({ "symbol": "Scale", "params": ["qty", "factor: int = 1"] }),
    )
    .await;
    assert!(
        !pointer_preview.starts_with("error: "),
        "pointer preview: {pointer_preview}"
    );
    let pointer_applied = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({ "symbol": "Scale", "params": ["qty", "factor: int = 1"], "apply": true }),
    )
    .await;
    assert!(
        !pointer_applied.starts_with("error: "),
        "pointer apply: {pointer_applied}"
    );
    let lib = std::fs::read_to_string(project.join("lib.go")).expect("lib");
    let main = std::fs::read_to_string(project.join("main.go")).expect("main");
    let test = std::fs::read_to_string(project.join("main_test.go")).expect("test");
    assert!(
        lib.contains("func (meter *Meter) Scale(qty int, factor int) string"),
        "{lib}"
    );
    assert!(
        main.contains("Scale(mark(\"pointer argument\", 4), 1)"),
        "{main}"
    );
    assert!(test.contains("(&meter).Scale(2, 1)"), "{test}");
    assert_eq!(
        behaviour(&project),
        before,
        "receiver addition changed behavior"
    );
}

const PUBLIC_REFUSAL_LIB: &str = r#"package main

import "fmt"

type Meter struct{}

func (meter Meter) Value(n int) string { return fmt.Sprint(n) }
func (meter Meter) Expression(n int) string { return fmt.Sprint(n) }
func (meter *Meter) Pointer(n int) string { return fmt.Sprint(n) }
func (meter Meter) Direct(n int) string { return fmt.Sprint(n) }
func (meter Meter) Dynamic(n int) string { return fmt.Sprint(n) }
func (meter Meter) Capture(existing int) string { captured := existing; return fmt.Sprint(captured) }

type Box[T any] struct{ value T }
func (box Box[T]) Generic(n int) T { return box.value }

func (meter Meter) Compile(n int) string { return fmt.Sprint(n) }
"#;

const PUBLIC_REFUSAL_INTERFACES: &str = r#"package main

type Directer interface { Direct(int) string }
type Dynamicer interface { Dynamic(int) string }
"#;

const PUBLIC_REFUSAL_MAIN: &str = r#"package main

import "fmt"

func main() {
	meter := Meter{}
	value := meter.Value
	expression := Meter.Expression
	pointer := (*Meter).Pointer
	var direct Directer = meter
	_, dynamic := any(meter).(Dynamicer)
	box := Box[int]{value: 7}
	fmt.Println(value(1), expression(meter, 2), pointer(&meter, 3), direct.Direct(4), dynamic, box.Generic(5), meter.Capture(6), meter.Compile(7))
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn receiver_refusals_run_through_the_source_built_gateway() {
    let gateway = Gateway::start();
    let (_dir, root) = receiver_checkout();
    let project = root.join("project");
    std::fs::write(project.join("lib.go"), PUBLIC_REFUSAL_LIB).unwrap();
    std::fs::write(project.join("interface.go"), PUBLIC_REFUSAL_INTERFACES).unwrap();
    std::fs::write(project.join("main.go"), PUBLIC_REFUSAL_MAIN).unwrap();
    std::fs::write(
        project.join("main_test.go"),
        "package main\nimport \"testing\"\nfunc TestFixture(t *testing.T) {}\n",
    )
    .unwrap();
    let before = behaviour(&project);
    assert_eq!(before.0, "1 2 3 4 true 7 6 7\n");
    let untouched = snapshot(&root);
    let position = |needle: &str| {
        let at = PUBLIC_REFUSAL_LIB.find(needle).expect("method definition");
        let row_start = PUBLIC_REFUSAL_LIB[..at].rfind('\n').map_or(0, |i| i + 1);
        (
            PUBLIC_REFUSAL_LIB[..at].matches('\n').count() + 1,
            PUBLIC_REFUSAL_LIB[row_start..at].encode_utf16().count() + 1,
        )
    };
    let (line, character) = position("Value(n");
    let mut hover = String::new();
    for _ in 0..60 {
        hover = tool(
            gateway.addr,
            &root,
            "code_hover",
            serde_json::json!({
                "path": "project/lib.go", "line": line, "character": character
            }),
        )
        .await;
        if hover.contains("func") && hover.contains("Value") {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(hover.contains("func") && hover.contains("Value"), "{hover}");
    for (needle, params, reason) in [
        (
            "Value(n",
            serde_json::json!(["n", "extra: int = 0"]),
            "used as a value",
        ),
        (
            "Expression(n",
            serde_json::json!(["n", "extra: int = 0"]),
            "method expression",
        ),
        (
            "Pointer(n",
            serde_json::json!(["n", "extra: int = 0"]),
            "method expression",
        ),
        (
            "Direct(n",
            serde_json::json!(["n", "extra: int = 0"]),
            "interface declaration",
        ),
        (
            "Dynamic(n",
            serde_json::json!(["n", "extra: int = 0"]),
            "interface declaration",
        ),
        (
            "Generic(n",
            serde_json::json!(["n", "extra: int = 0"]),
            "not an ordinary named value",
        ),
        (
            "Capture(existing",
            serde_json::json!(["existing", "meter: int = 0"]),
            "duplicates the receiver binding",
        ),
        (
            "Capture(existing",
            serde_json::json!(["existing", "captured: int = 0"]),
            "shadow existing references",
        ),
        (
            "Compile(n",
            serde_json::json!(["n", "bad: string = 1"]),
            "does not compile",
        ),
    ] {
        let (line, character) = position(needle);
        let refused = tool(
            gateway.addr,
            &root,
            "code_change_signature",
            serde_json::json!({
                "path": "project/lib.go", "line": line, "character": character,
                "params": params, "apply": true, "force": true
            }),
        )
        .await;
        assert!(
            refused.starts_with("error: ") && refused.contains(reason),
            "{needle}: {refused}"
        );
        assert_eq!(snapshot(&root), untouched, "refusal wrote: {needle}");
    }
    assert_eq!(behaviour(&project), before, "refusals changed execution");
}
