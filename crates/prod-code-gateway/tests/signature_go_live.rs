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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unused_go_parameters_are_removed_through_the_real_gateway_and_gopls() {
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
    // This test binary contains one test, and the server is already a separate process with its
    // original environment, so the temporary client-only process environment is isolated here.
    unsafe {
        std::env::set_var("PATH", &client_path);
        std::env::set_var("PROD_CODE_GO_SENTINEL", &marker);
    }

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

    // The source-built gateway also carries an explicitly typed addition through public MCP.
    // gopls supplies the complete reference set; the adapter inserts only the pure literal and
    // compiles every source and test caller in a private shadow before writing.
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
    eprintln!("applied addition:\n{added}");
    assert!(
        !added.starts_with("error: ") && added.contains("[applied to 3 file(s)]"),
        "{added}"
    );
    unsafe {
        std::env::set_var("PATH", original_path);
        std::env::remove_var("PROD_CODE_GO_SENTINEL");
    }
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
