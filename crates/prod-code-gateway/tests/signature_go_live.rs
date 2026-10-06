/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

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
use std::sync::OnceLock;
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
            .env("PROD_CODE_ENGINE_RESERVE_MIB", "64")
            .env_remove("RUST_LOG")
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

// Count has one unnamed primitive result: only this token may change.
func Count(n int) int { return 7 }

type (
	// Grouped aliases prove that compilation cannot establish primitive identity.
	uint64 = /* requested shadow */ interface{}
	byte   = interface{}
)

func RequestedShadow(n int) int    { return n }
func OldShadow(n int) byte         { return n }
func VariadicResult(xs ...int) int { return len(xs) }
func BodyMismatch(n int) int       { return n }
func TestCaller(n int) int         { return n }
func FreeValueResult(n int) int    { return n }

type ResultValue struct{}
type ResultPointer struct{}

func makeResultValue(tag string) ResultValue {
	trace = append(trace, tag)
	return ResultValue{}
}

func makeResultPointer(tag string) *ResultPointer {
	trace = append(trace, tag)
	return &ResultPointer{}
}

func (value ResultValue) ValueResult(n int) int { return 7 }

func (pointer *ResultPointer) PointerResult(n int) int { return 7 }

func (value ResultValue) ReceiverBodyMismatch(n int) int { return n }

func (value ResultValue) ReceiverTestCaller(n int) int { return n }

var savedResult = FreeValueResult
"#;

const MAIN: &str = r#"package main

import "fmt"

func main() {
	note, prio := "fragile", 2
	fmt.Println(Ship(3, "glass", 1, "LA"), Ship(mark("m", 2), note, prio, "NY")) // Ship(1, "x", 2, "y")
	fmt.Println(Count(mark("count", 1)))
	fmt.Println(makeResultValue("value receiver").ValueResult(mark("value result", 2)), makeResultPointer("pointer receiver").PointerResult(mark("pointer result", 3)))
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

func TestCount(t *testing.T) {
	if Count(1) != 7 {
		t.Fatal("count")
	}
}

func TestReceiverResultCallers(t *testing.T) {
	value := ResultValue{}
	pointer := ResultPointer{}
	if value.ValueResult(1) != 7 || pointer.PointerResult(2) != 7 {
		t.Fatal("receiver result")
	}
}

func TestReceiverResultCallerType(t *testing.T) {
	var value ResultValue
	var got int = value.ReceiverTestCaller(1)
	if got != 1 {
		t.Fatal("typed receiver caller")
	}
}

func TestResultCallerType(t *testing.T) {
	var got int = TestCaller(1)
	if got != 1 {
		t.Fatal("typed caller")
	}
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

/// A committed package whose same-package declaration is a git-tracked linked Go source. The
/// target deliberately has a non-Go extension: `go` follows the link named `shadow.go`, while a
/// directory scan must not follow an arbitrary target just to prove primitive identity.
#[cfg(unix)]
fn symlink_checkout() -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::symlink;

    let dir = tempfile::Builder::new()
        .prefix("gosigsymlink")
        .tempdir()
        .expect("checkout dir");
    for (rel, text) in [
        ("go.work", "go 1.22\n\nuse ./project\n"),
        (
            "project/go.mod",
            "module example.com/gosigsymlink\n\ngo 1.22\n",
        ),
        (
            "project/lib.go",
            "package main\n\nfunc RequestedShadow(n int) int { return n }\nfunc OldShadow(n int) int64 { return n }\ntype Meter struct{}\nfunc (meter Meter) ResultRequestedShadow(n int) int { return n }\nfunc (meter Meter) ResultOldShadow(n int) int64 { return n }\n",
        ),
        (
            "project/main_test.go",
            "package main\n\nimport \"testing\"\n\nfunc TestLinkedSourceCompiles(t *testing.T) {\n\tvar meter Meter\n\tvar got interface{} = OldShadow(1)\n\tvar method interface{} = meter.ResultOldShadow(2)\n\tif got == nil || method == nil { t.Fatal(\"linked source was not loaded\") }\n}\n",
        ),
        (
            "project/shadow_source.txt",
            "package main\n\ntype int64 = interface{}\n",
        ),
    ] {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }
    symlink("shadow_source.txt", dir.path().join("project/shadow.go")).expect("link source");
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

/// A fresh cache makes `go test -x` print the compiler invocation, proving the linked source
/// participates in the package instead of merely existing in the fixture.
#[cfg(unix)]
fn compiler_output(root: &Path) -> (bool, String) {
    let cache = tempfile::tempdir().expect("compiler cache");
    let out = Command::new("go")
        .args(["test", "-x", "-count=1", "."])
        .current_dir(root)
        .env("GOTOOLCHAIN", "local")
        .env("GOFLAGS", "-tags=prodcode_signature -mod=readonly")
        .env("GOCACHE", cache.path())
        .output()
        .expect("go is a prerequisite of this real-server test");
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

/// The client binary is built from this checkout on the same node before a live scenario uses
/// it. Keeping the path derived from the manifest prevents an installed client from masking a
/// candidate regression.
fn source_cli() -> PathBuf {
    static CLI: OnceLock<PathBuf> = OnceLock::new();
    CLI.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("gateway crate lives below workspace")
            .to_path_buf();
        // A seeded target can contain an older binary. Always ask Cargo to establish that
        // this checkout's client is current, including when instrumentation selects a target.
        let status = Command::new("cargo")
            .args(["build", "-p", "prod-code-client", "--bin", "prod-code"])
            .current_dir(&workspace)
            .status()
            .expect("build source client");
        assert!(status.success(), "build source client: {status}");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join("target"));
        let target = if target.is_absolute() {
            target
        } else {
            workspace.join(target)
        };
        let cli = target
            .join("debug")
            .join(format!("prod-code{}", std::env::consts::EXE_SUFFIX));
        assert!(
            cli.is_file(),
            "source-built CLI missing at {}",
            cli.display()
        );
        cli
    })
    .clone()
}

/// Drive the public CLI built from this revision, not an installed `prod-code` binary.
fn cli(root: &Path, addr: SocketAddr, args: &[&str]) -> (bool, String) {
    let out = Command::new(source_cli())
        .args(["--remote", &addr.to_string()])
        .args(args)
        .current_dir(root)
        .output()
        .expect("source CLI runs");
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
    let (result_ok, result_refusal) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "BodyMismatch",
            "--param",
            "n",
            "--returns",
            "string",
            "--apply",
            "--force",
        ],
    );
    assert!(
        !result_ok && result_refusal.contains("does not compile"),
        "{result_refusal}"
    );
    assert_eq!(
        snapshot(&root),
        before_refusal,
        "result compiler refusal wrote"
    );
    let (receiver_ok, receiver_result) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "ValueResult",
            "--param",
            "n",
            "--returns",
            "int",
            "--apply",
            "--force",
        ],
    );
    assert!(
        receiver_ok && receiver_result.contains("[applied to 1 file(s)]"),
        "{receiver_result}"
    );
    let restored = cli(
        &root,
        addr,
        &[
            "change-signature",
            "ValueResult",
            "--param",
            "n",
            "--returns",
            "int64",
            "--apply",
            "--force",
        ],
    );
    assert!(
        restored.0 && restored.1.contains("[applied to 1 file(s)]"),
        "{}",
        restored.1
    );
    assert_eq!(
        snapshot(&root),
        before_refusal,
        "receiver result sentinel round trip changed bytes"
    );
    let (refused, refused_addition) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "Ship",
            "--param",
            "dest",
            "--param",
            "qty",
            "--param",
            "bad: string = 1",
            "--apply",
            "--force",
        ],
    );
    assert!(
        !refused && refused_addition.contains("does not compile"),
        "{refused_addition}"
    );
    assert_eq!(snapshot(&root), before_refusal, "compiler refusal wrote");

    let (added_ok, added) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "Ship",
            "--param",
            "dest",
            "--param",
            "qty",
            "--param",
            "route: string = \"road,air\"",
            "--apply",
            "--force",
        ],
    );
    assert!(
        added_ok && added.contains("[applied to 3 file(s)]"),
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

    // The source-built public CLI previews and applies a result replacement. The real gateway
    // compiles the package plus its test caller before either action.
    let (preview_ok, count_preview) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "Count",
            "--param",
            "n",
            "--returns",
            "int64",
        ],
    );
    assert!(
        preview_ok
            && count_preview.contains("returns: `int` → `int64`")
            && count_preview.contains("nothing was written"),
        "{count_preview}"
    );
    assert_eq!(snapshot(&root), untouched, "result preview wrote");
    let (applied_ok, count_applied) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "Count",
            "--param",
            "n",
            "--returns",
            "int64",
            "--apply",
        ],
    );
    assert!(applied_ok, "{count_applied}");
    let count_lib = std::fs::read_to_string(project.join("lib.go")).expect("lib.go");
    assert!(
        count_lib.contains("func Count(n int) int64 { return 7 }"),
        "{count_lib}"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("main.go")).expect("main.go"),
        MAIN,
        "result replacement touched a caller"
    );
    let after_result = behaviour(&project);
    assert_eq!(after_result, before, "result replacement changed execution");
    assert!(
        after_result
            .0
            .contains("[Ship(3,LA) m Ship(2,NY) count value receiver value result pointer receiver pointer result call]"),
        "function and receiver evaluation trace was not observed: {after_result:?}"
    );
    // Exercise non-no-op MCP preview/apply against the same real compiler as well. Restoring
    // the original result must restore every source byte before the CLI reapplies its change.
    let mcp_preview = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({
            "symbol": "Count", "params": ["n"], "returns": "int"
        }),
    )
    .await;
    assert!(
        !mcp_preview.starts_with("error: ") && mcp_preview.contains("nothing was written"),
        "{mcp_preview}"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("lib.go")).unwrap(),
        count_lib,
        "MCP preview wrote"
    );
    let mcp_apply = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({
            "symbol": "Count", "params": ["n"], "returns": "int", "apply": true
        }),
    )
    .await;
    assert!(!mcp_apply.starts_with("error: "), "{mcp_apply}");
    assert_eq!(
        snapshot(&root),
        untouched,
        "restoring the result did not restore all fixture bytes"
    );
    let (reapplied, output) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "Count",
            "--param",
            "n",
            "--returns",
            "int64",
            "--apply",
        ],
    );
    assert!(reapplied, "{output}");
    assert_eq!(
        std::fs::read_to_string(project.join("lib.go")).unwrap(),
        count_lib
    );

    // The same source-built CLI and MCP tool support ordinary value and pointer receiver results
    // only when every use is a direct selector call and the real compiler accepts test callers.
    let receiver_untouched = snapshot(&root);
    let (value_preview_ok, value_preview) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "ValueResult",
            "--param",
            "n",
            "--returns",
            "int64",
        ],
    );
    assert!(
        value_preview_ok && value_preview.contains("nothing was written"),
        "{value_preview}"
    );
    assert_eq!(
        snapshot(&root),
        receiver_untouched,
        "value receiver preview wrote"
    );
    let (value_apply_ok, value_apply) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "ValueResult",
            "--param",
            "n",
            "--returns",
            "int64",
            "--apply",
        ],
    );
    assert!(value_apply_ok, "{value_apply}");
    let value_applied = snapshot(&root);
    let (value_noop_ok, value_noop) = cli(
        &root,
        addr,
        &[
            "change-signature",
            "ValueResult",
            "--param",
            "n",
            "--returns",
            "int64",
            "--apply",
            "--force",
        ],
    );
    assert!(
        value_noop_ok && value_noop.contains("nothing was written"),
        "{value_noop}"
    );
    assert_eq!(snapshot(&root), value_applied, "value receiver no-op wrote");
    let pointer_preview = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({ "symbol": "PointerResult", "params": ["n"], "returns": "int64" }),
    )
    .await;
    assert!(
        !pointer_preview.starts_with("error: ") && pointer_preview.contains("nothing was written"),
        "{pointer_preview}"
    );
    let pointer_apply = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({ "symbol": "PointerResult", "params": ["n"], "returns": "int64", "apply": true }),
    )
    .await;
    assert!(!pointer_apply.starts_with("error: "), "{pointer_apply}");
    let pointer_applied = snapshot(&root);
    let pointer_noop = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({
            "symbol": "PointerResult", "params": ["n"], "returns": "int64",
            "apply": true, "force": true
        }),
    )
    .await;
    assert!(
        !pointer_noop.starts_with("error: ") && pointer_noop.contains("nothing was written"),
        "{pointer_noop}"
    );
    assert_eq!(
        snapshot(&root),
        pointer_applied,
        "pointer receiver no-op wrote"
    );
    let receiver_lib = std::fs::read_to_string(project.join("lib.go")).expect("receiver lib");
    assert!(
        receiver_lib.contains("func (value ResultValue) ValueResult(n int) int64 { return 7 }")
            && receiver_lib
                .contains("func (pointer *ResultPointer) PointerResult(n int) int64 { return 7 }"),
        "{receiver_lib}"
    );
    for path in ["project/main.go", "project/main_test.go"] {
        assert_eq!(
            snapshot(&root)[path],
            receiver_untouched[path],
            "receiver result touched {path}"
        );
    }
    assert_eq!(
        behaviour(&project),
        before,
        "receiver result conversion changed behavior"
    );
    let untouched = snapshot(&root);

    // A no-op still lists complete references and compiles packages plus test callers remotely.
    let no_op = tool(
        addr,
        &root,
        "code_change_signature",
        serde_json::json!({
            "symbol": "Count", "params": ["n"], "returns": "int64",
            "apply": true, "force": true
        }),
    )
    .await;
    assert!(
        !no_op.starts_with("error: ") && no_op.contains("nothing was written"),
        "{no_op}"
    );
    assert_eq!(snapshot(&root), untouched, "result no-op wrote");

    // Compiler and semantic refusals include an incompatible body, a typed test-only caller,
    // requested and existing shadowed primitive names, a variadic declaration with no calls, and
    // an indirect function value. Force cannot turn any of them into a write.
    for (symbol, returns, reason) in [
        ("BodyMismatch", "string", "does not compile"),
        ("TestCaller", "int64", "does not compile"),
        ("ReceiverBodyMismatch", "string", "does not compile"),
        ("ReceiverTestCaller", "int64", "does not compile"),
        (
            "RequestedShadow",
            "uint64",
            "primitive type identity cannot be proven",
        ),
        (
            "OldShadow",
            "string",
            "primitive type identity cannot be proven",
        ),
        ("VariadicResult", "int64", "variadic function"),
        ("FreeValueResult", "int64", "used as a value"),
    ] {
        let refused = tool(
            addr,
            &root,
            "code_change_signature",
            serde_json::json!({
                "symbol": symbol, "params": if symbol == "VariadicResult" {
                    serde_json::json!(["xs"])
                } else {
                    serde_json::json!(["n"])
                },
                "returns": returns, "apply": true, "force": true
            }),
        )
        .await;
        assert!(
            refused.starts_with("error: ") && refused.contains(reason),
            "{symbol}: {refused}"
        );
        assert_eq!(snapshot(&root), untouched, "result refusal wrote: {symbol}");
    }

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
        )
        .replace(
            "func Count(n int) int { return 7 }",
            "func Count(n int) int64 { return 7 }"
        )
        .replace(
            "func (value ResultValue) ValueResult(n int) int { return 7 }",
            "func (value ResultValue) ValueResult(n int) int64 { return 7 }"
        )
        .replace(
            "func (pointer *ResultPointer) PointerResult(n int) int { return 7 }",
            "func (pointer *ResultPointer) PointerResult(n int) int64 { return 7 }"
        ),
        "only the requested signature tokens changed"
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

/// Linked Go sources are part of a package to the compiler, but the primitive-result guard must
/// never follow them: an in-checkout link can still be retargeted outside the checkout between
/// inspection and apply. Both a requested and an old primitive spelling therefore refuse before
/// the gateway writes any proposal.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_go_source_refuses_result_replacement_without_writing() {
    let (ok, version) = run(Path::new("."), "gopls", &["version"]);
    assert!(ok, "gopls version: {version}");
    let gateway = Gateway::start();
    let (_dir, root) = symlink_checkout();
    let project = root.join("project");
    let link = project.join("shadow.go");
    let target = std::fs::read_link(&link).expect("linked Go source");
    let git_index = Command::new("git")
        .args(["ls-files", "-s", "project/shadow.go"])
        .current_dir(&root)
        .output()
        .expect("git lists linked source");
    assert!(git_index.status.success(), "git index: {git_index:?}");
    assert!(
        String::from_utf8_lossy(&git_index.stdout).starts_with("120000 "),
        "linked source is git tracked: {}",
        String::from_utf8_lossy(&git_index.stdout)
    );
    let (compiled, compiler) = compiler_output(&project);
    assert!(compiled, "go test -x: {compiler}");
    assert!(
        compiler.contains("shadow.go"),
        "compiler command did not include linked Go source: {compiler}"
    );
    let untouched = snapshot(&root);

    // Wait for the real gopls behind the public gateway, rather than accepting a fixture-only
    // scan as evidence that this is a loadable package.
    let mut hover = String::new();
    for _ in 1..=60 {
        hover = tool(
            gateway.addr,
            &root,
            "code_hover",
            serde_json::json!({ "path": "project/lib.go", "line": 3, "character": 6 }),
        )
        .await;
        if hover.contains("RequestedShadow") {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(hover.contains("RequestedShadow"), "gopls load: {hover}");

    for (symbol, returns) in [
        ("RequestedShadow", "int64"),
        ("OldShadow", "string"),
        ("ResultRequestedShadow", "int64"),
        ("ResultOldShadow", "string"),
    ] {
        let refused = tool(
            gateway.addr,
            &root,
            "code_change_signature",
            serde_json::json!({
                "symbol": symbol,
                "params": ["n"],
                "returns": returns,
                "apply": true,
                "force": true
            }),
        )
        .await;
        assert!(
            refused.starts_with("error: ") && refused.contains("linked Go source"),
            "{symbol}: {refused}"
        );
        assert_eq!(snapshot(&root), untouched, "refusal wrote: {symbol}");
        assert_eq!(std::fs::read_link(&link).expect("linked Go source"), target);
    }
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
func (meter Meter) String() string { return "meter" }
func (meter Meter) Expression(n int) string { return fmt.Sprint(n) }
func (meter *Meter) Pointer(n int) string { return fmt.Sprint(n) }
func (meter Meter) Direct(n int) string { return fmt.Sprint(n) }
func (meter Meter) Dynamic(n int) string { return fmt.Sprint(n) }
func (meter Meter) Capture(existing int) string { captured := existing; return fmt.Sprint(captured) }

func (meter Meter) ResultBodyMismatch(n int) int { return n }
func (meter Meter) ResultTestCaller(n int) int { return n }
func (meter Meter) ResultValue(n int) int { return n }
func (meter Meter) ResultExpression(n int) int { return n }
func (meter *Meter) ResultPointer(n int) int { return n }
func (meter Meter) ResultDirect(n int) int { return n }
func (meter Meter) ResultDynamic(n int) int { return n }
func (meter Meter) ResultCombine(a, b int) int { return a + b }
func (meter Meter) ResultVariadic(xs ...int) int { return len(xs) }

type (
	uint64 = interface{}
	byte = interface{}
)
func (meter Meter) ResultRequestedShadow(n int) int { return n }
func (meter Meter) ResultOldShadow(n int) byte { return n }

type Box[T any] struct{ value T }
func (box Box[T]) Generic(n int) T { return box.value }

type ResultBox[T any] struct{}
func (box ResultBox[T]) ResultGenericReceiver(n int) int { return n }
func ResultGenericFunction[T any](n int) int { return n }
func ResultVariadicFunction(xs ...int) int { return len(xs) }

func (meter Meter) Compile(n int) string { return fmt.Sprint(n) }
"#;

const PUBLIC_REFUSAL_INTERFACES: &str = r#"package main

type Directer interface { Direct(int) string }
type Dynamicer interface { Dynamic(int) string }
type ResultDirecter interface { ResultDirect(int) int }
type ResultDynamicer interface { ResultDynamic(int) int }
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
	_, stringer := any(meter).(fmt.Stringer)
	box := Box[int]{value: 7}
	resultValue := meter.ResultValue
	resultExpression := Meter.ResultExpression
	resultPointer := (*Meter).ResultPointer
	var resultDirect ResultDirecter = meter
	resultDynamic, _ := any(meter).(ResultDynamicer)
	_ = resultValue
	_ = resultExpression
	_ = resultPointer
	_ = resultDirect.ResultDirect(8)
	_ = resultDynamic.ResultDynamic(9)
	_ = meter.ResultCombine(1, 2)
	fmt.Println(value(1), expression(meter, 2), pointer(&meter, 3), direct.Direct(4), dynamic, box.Generic(5), meter.Capture(6), meter.Compile(7), stringer)
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
        "package main\nimport \"testing\"\nfunc TestFixture(t *testing.T) { var meter Meter; var got int = meter.ResultTestCaller(1); if got != 1 { t.Fatal(got) } }\n",
    )
    .unwrap();
    let before = behaviour(&project);
    assert_eq!(before.0, "1 2 3 4 true 7 6 7 true\n");
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
            "String()",
            serde_json::json!(["extra: int = 0"]),
            "interface",
        ),
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
            "interface implementation evidence",
        ),
        (
            "Dynamic(n",
            serde_json::json!(["n", "extra: int = 0"]),
            "interface implementation evidence",
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
    for (needle, params, returns, reason) in [
        (
            "ResultBodyMismatch(n",
            serde_json::json!(["n"]),
            "string",
            "does not compile",
        ),
        (
            "ResultTestCaller(n",
            serde_json::json!(["n"]),
            "int64",
            "does not compile",
        ),
        (
            "String()",
            serde_json::json!([]),
            "int64",
            "interface implementation evidence",
        ),
        (
            "ResultValue(n",
            serde_json::json!(["n"]),
            "int64",
            "used as a value",
        ),
        (
            "ResultExpression(n",
            serde_json::json!(["n"]),
            "int64",
            "method expression",
        ),
        (
            "ResultPointer(n",
            serde_json::json!(["n"]),
            "int64",
            "method expression",
        ),
        (
            "ResultDirect(n",
            serde_json::json!(["n"]),
            "int64",
            "interface implementation evidence",
        ),
        (
            "ResultDynamic(n",
            serde_json::json!(["n"]),
            "int64",
            "interface implementation evidence",
        ),
        (
            "ResultGenericReceiver(n",
            serde_json::json!(["n"]),
            "int64",
            "not an ordinary named value",
        ),
        (
            "ResultVariadic(xs",
            serde_json::json!(["xs"]),
            "int64",
            "variadic function",
        ),
        (
            "ResultGenericFunction[T",
            serde_json::json!(["n"]),
            "int64",
            "generic function",
        ),
        (
            "ResultVariadicFunction(xs",
            serde_json::json!(["xs"]),
            "int64",
            "variadic function",
        ),
        (
            "ResultCombine(a",
            serde_json::json!(["b", "a"]),
            "int64",
            "parameter list exactly unchanged",
        ),
        (
            "ResultRequestedShadow(n",
            serde_json::json!(["n"]),
            "uint64",
            "primitive type identity cannot be proven",
        ),
        (
            "ResultOldShadow(n",
            serde_json::json!(["n"]),
            "string",
            "primitive type identity cannot be proven",
        ),
    ] {
        let (line, character) = position(needle);
        let refused = tool(
            gateway.addr,
            &root,
            "code_change_signature",
            serde_json::json!({
                "path": "project/lib.go", "line": line, "character": character,
                "params": params, "returns": returns, "apply": true, "force": true
            }),
        )
        .await;
        assert!(
            refused.starts_with("error: ") && refused.contains(reason),
            "{needle}: {refused}"
        );
        assert_eq!(snapshot(&root), untouched, "result refusal wrote: {needle}");
    }
    assert_eq!(behaviour(&project), before, "refusals changed execution");
}
