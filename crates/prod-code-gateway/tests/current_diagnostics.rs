//! Diagnostics from a language server that publishes them (clangd, pyright), through the
//! gateway built from this revision, over a real socket, through the calls an agent makes.
//!
//! A publishing server answers for a text only once it has built it. When no publication for
//! the text last sent comes, there is no report: the gateway must say so, and the tools with
//! it, instead of answering with an empty list that reads as "no errors" (#471). One gateway
//! runs a stand-in `clangd`, which stays silent for a text that says `silent`; another runs the
//! real clangd, the control that real publications, empty ones included, still answer.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A stand-in clangd: it refuses a diagnostic pull as clangd does, publishes for each text it
/// is sent with that text's version (an error for a text that says `broken`, nothing for any
/// other), publishes nothing at all for a text that says `silent`, and offers one quick fix
/// naming how many diagnostics the code action request carried.
const FAKE_CLANGD: &str = r#"#!/usr/bin/env python3
import json, sys, threading

LOCK = threading.Lock()

def send(message):
    body = json.dumps(message).encode()
    with LOCK:
        sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body))
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()

def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    if not length:
        return None
    return json.loads(sys.stdin.buffer.read(length))

ERROR = {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
         "severity": 1, "message": "fake clangd: the text is broken"}

while True:
    message = read()
    if message is None:
        break
    method = message.get("method", "")
    params = message.get("params") or {}
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": {"codeActionProvider": True}}})
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        document = params["textDocument"]
        text = document.get("text") or params["contentChanges"][-1]["text"]
        if "silent" not in text:
            items = [ERROR] if "broken" in text else []
            send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
                "uri": document["uri"], "version": document["version"], "diagnostics": items}})
    elif method == "textDocument/diagnostic":
        if "nonfull" in params["textDocument"]["uri"]:
            send({"jsonrpc": "2.0", "id": message["id"], "result": {"kind": "unchanged", "items": []}})
        else:
            send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "method not found"}})
    elif method == "textDocument/codeAction":
        count = len(params.get("context", {}).get("diagnostics", []))
        send({"jsonrpc": "2.0", "id": message["id"], "result": [
            {"title": "fix %d diagnostic(s)" % count, "kind": "quickfix", "edit": {"changes": {}}}]})
    elif method == "exit":
        break
    elif "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"], "result": None})
"#;

/// A gateway process and its storage, stopped when dropped.
struct Gateway {
    child: Child,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
}

impl Gateway {
    /// Starts `prod-code-server` on a port of its choosing; with `home`, that is its home
    /// directory, whose `.local/bin` the gateway puts first on its `PATH`, so the language
    /// servers it starts are found there.
    fn start(home: Option<&Path>) -> Self {
        let storage = tempfile::tempdir().expect("storage dir");
        let mut command = Command::new(env!("CARGO_BIN_EXE_prod-code-server"));
        command
            .env("PROD_CODE_STORAGE", storage.path())
            // No peers, no gossip: this gateway is alone and must not look for others.
            .env("PROD_CODE_PEERS", "")
            .env("PROD_CODE_BIND", "127.0.0.1:0")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(home) = home {
            command.env("HOME", home);
        }
        let child = command.spawn().expect("the server binary starts");
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

/// The address the daemon logs as bound, read on a thread of its own that drains the log for
/// the life of the process.
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

/// A committed C checkout (`.clangd` makes it one), because the client syncs against `HEAD`.
fn checkout(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("checkout dir");
    std::fs::write(dir.path().join(".clangd"), "").expect("write .clangd");
    for (rel, text) in files {
        std::fs::write(dir.path().join(rel), text).expect("write");
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
        "subject",
    ]);
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");
    (dir, root)
}

/// A home directory with the stand-in clangd in its `.local/bin`.
fn fake_clangd() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("home dir");
    let bin = dir.path().join(".local").join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir .local/bin");
    let path = bin.join("clangd");
    std::fs::write(&path, FAKE_CLANGD).expect("write the stand-in");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    dir
}

/// A report's error count and messages, or its failure.
fn summary(
    report: &anyhow::Result<prod_code_mcp::diagnostics::DiagnosticsReport>,
) -> Result<(usize, Vec<String>), String> {
    match report {
        Ok(r) => Ok((
            r.errors,
            r.items.iter().map(|d| d.message.clone()).collect(),
        )),
        Err(e) => Err(format!("{e:#}")),
    }
}

/// The text of a tool call, or its failure.
async fn tool(
    addr: SocketAddr,
    root: &Path,
    name: &str,
    args: serde_json::Value,
) -> Result<String, String> {
    match prod_code_mcp::tools::execute_tool(addr, root, name, args).await {
        Ok(result) if result.is_error => Err(texts(&result)),
        Ok(result) => Ok(texts(&result)),
        Err(e) => Err(format!("{e:#}")),
    }
}

fn texts(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| {
            let prod_code_mcp::protocol::McpContentItem::Text { text } = item;
            text.clone()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A document the server published nothing for is an error through diagnostics, validation and
/// code actions, naming the document; one it published for, empty or not, is answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_publishing_server_that_stays_silent_is_an_error_not_a_clean_report() {
    let home = fake_clangd();
    let gateway = Gateway::start(Some(home.path()));
    let (_dir, root) = checkout(&[
        ("clean.c", "int clean;\n"),
        ("broken.c", "int broken; // broken\n"),
        ("silent.c", "int quiet; // silent\n"),
        ("silent_nonfull.c", "int quiet; // silent\n"),
    ]);
    let addr = gateway.addr;
    eprintln!("gateway from this revision listening on {addr}");
    use prod_code_mcp::diagnostics::{diagnostics, validate_text};

    let clean = summary(&diagnostics(addr, &root, Path::new("clean.c")).await);
    eprintln!("clean.c: {clean:?}");
    assert_eq!(
        clean,
        Ok((0, Vec::new())),
        "a published empty list is a report"
    );
    let broken = summary(&diagnostics(addr, &root, Path::new("broken.c")).await);
    eprintln!("broken.c: {broken:?}");
    assert_eq!(
        broken,
        Ok((1, vec!["fake clangd: the text is broken".to_string()]))
    );
    let proposed =
        summary(&validate_text(addr, &root, Path::new("clean.c"), "int clean; // broken\n").await);
    eprintln!("clean.c proposed broken: {proposed:?}");
    assert_eq!(proposed.map(|(errors, _)| errors), Ok(1));
    let fixes = tool(
        addr,
        &root,
        "code_assists",
        serde_json::json!({ "path": "broken.c", "line": 1, "character": 1 }),
    )
    .await;
    eprintln!("broken.c assists: {fixes:?}");
    assert!(
        fixes
            .as_deref()
            .is_ok_and(|t| t.contains("fix 1 diagnostic(s)")),
        "the quick fix request carried the current diagnostic: {fixes:?}"
    );

    // The gateway waits for a publication before it gives up, so the three silent cases run
    // together.
    let started = Instant::now();
    let (silent, nonfull, silent_proposal, silent_fixes) = tokio::join!(
        diagnostics(addr, &root, Path::new("silent.c")),
        diagnostics(addr, &root, Path::new("silent_nonfull.c")),
        validate_text(addr, &root, Path::new("clean.c"), "int clean; // silent\n"),
        tool(
            addr,
            &root,
            "code_assists",
            serde_json::json!({ "path": "silent.c", "line": 1, "character": 1 }),
        ),
    );
    let (silent, silent_proposal) = (summary(&silent), summary(&silent_proposal));
    eprintln!("nonfull report: {:?}", summary(&nonfull));
    eprintln!(
        "after {:?}: silent.c: {silent:?}; clean.c proposed silent: {silent_proposal:?}; \
         silent.c assists: {silent_fixes:?}",
        started.elapsed()
    );
    for (what, outcome, file) in [
        ("diagnostics", silent.map(|r| format!("{r:?}")), "silent.c"),
        (
            "validation",
            silent_proposal.map(|r| format!("{r:?}")),
            "clean.c",
        ),
        ("code actions", silent_fixes, "silent.c"),
        (
            "uncached unchanged report",
            summary(&nonfull).map(|r| format!("{r:?}")),
            "silent_nonfull.c",
        ),
    ] {
        let err = outcome.expect_err(&format!(
            "{what}: no publication for the text is not a clean report"
        ));
        assert!(
            err.contains("no current diagnostics") && err.contains(file),
            "{what}: the failure says what is missing and for which document: {err}"
        );
    }
}

/// The control: the real clangd publishes for each text it is sent, and those publications,
/// the empty ones included, answer through the gateway as before. Build nodes have clangd; a
/// run without it fails rather than passing with nothing verified.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_real_clangd_still_answers_with_its_publications() {
    let version = Command::new("clangd")
        .arg("--version")
        .output()
        .expect("clangd is installed: the control needs a real publishing server");
    eprintln!(
        "real server: {}",
        String::from_utf8_lossy(&version.stdout)
            .lines()
            .next()
            .unwrap_or_default()
    );
    let gateway = Gateway::start(None);
    let (_dir, root) = checkout(&[
        ("clean.c", "int clean(void) { return 0; }\n"),
        ("broken.c", "int broken(void) { return undefined_name; }\n"),
    ]);
    let addr = gateway.addr;
    use prod_code_mcp::diagnostics::{diagnostics, validate_text};

    let clean = summary(&diagnostics(addr, &root, Path::new("clean.c")).await);
    eprintln!("clangd clean.c: {clean:?}");
    assert_eq!(clean, Ok((0, Vec::new())), "a clean file is a clean report");
    let broken = summary(&diagnostics(addr, &root, Path::new("broken.c")).await);
    eprintln!("clangd broken.c: {broken:?}");
    let (errors, messages) = broken.expect("a report");
    assert!(
        errors >= 1 && messages.iter().any(|m| m.contains("undefined_name")),
        "{messages:?}"
    );
    let proposed = summary(
        &validate_text(
            addr,
            &root,
            Path::new("clean.c"),
            "int clean(void) { return missing_name; }\n",
        )
        .await,
    );
    eprintln!("clangd clean.c proposed broken: {proposed:?}");
    let (errors, messages) = proposed.expect("a report");
    assert!(
        errors >= 1 && messages.iter().any(|m| m.contains("missing_name")),
        "{messages:?}"
    );
    let fixed = summary(
        &validate_text(
            addr,
            &root,
            Path::new("broken.c"),
            "int broken(void) { return 1; }\n",
        )
        .await,
    );
    eprintln!("clangd broken.c proposed fixed: {fixed:?}");
    assert_eq!(fixed.map(|(errors, _)| errors), Ok(0));
}

/// This is the first pull of a fresh server, before any unsupported response can disable
/// pulling. An uncached unchanged result is not a full empty report (#479).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_uncached_unchanged_pull_report_is_not_a_full_clean_report() {
    let home = fake_clangd();
    let gateway = Gateway::start(Some(home.path()));
    let (_dir, root) = checkout(&[("silent_nonfull.c", "int quiet; // silent\n")]);
    let report =
        prod_code_mcp::diagnostics::diagnostics(gateway.addr, &root, Path::new("silent_nonfull.c"))
            .await;
    let result = summary(&report);
    eprintln!("first uncached unchanged report: {result:?}");
    let error = result.expect_err("an uncached unchanged report cannot establish zero errors");
    assert!(error.contains("no current diagnostics"), "{error}");
}
