/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! `code_slice` against the gateway built from this revision and its real Rust engine, over a
//! real socket, through the same tool call an agent makes.
//!
//! The scripted tests of `prod-code-mcp` decide what the slicer does with each kind of answer;
//! only a real engine shows that its own answers pass the slicer's coordinate checks: UTF-16
//! columns after wide characters, a file with CRLF line breaks, and the inverted range it gives
//! `pub mod name;`. It also shows that a real slice comes back complete, that one cut by the
//! depth limit says it is bounded, and that a seed column past its line is refused.
//!
//! The gateway binds port 0 and reports the address it bound, so no two runs share a port.

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
            .env_remove("RUST_LOG")
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

/// The address the daemon logs as bound. The log is read on its own thread and drained for
/// the life of the process, so a silent pipe cannot hang the test and a full one cannot stop
/// the gateway.
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

const MANIFEST: &str =
    "[package]\nname = \"slice_live\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";
/// `seed` is on line 6, its name at column 8.
const LIB: &str = "pub mod config;\npub mod wide;\n\nuse config::Config;\n\npub fn seed(cfg: &Config) -> u32 {\n    let local = cfg.count;\n    dependency(local)\n}\n\npub fn dependency(value: u32) -> u32 {\n    value + 1\n}\n\npub fn unrelated() -> u32 {\n    7\n}\n";
const CONFIG: &str = "pub struct Config {\n    pub count: u32,\n}\n";
/// CRLF line breaks, and a call after a string with a one-unit and a two-unit character.
const WIDE: &str = "pub fn wide_seed() -> usize {\r\n    let s = \"é😀\"; wide_dep(s)\r\n}\r\n\r\npub fn wide_dep(s: &str) -> usize {\r\n    s.len()\r\n}\r\n";

/// A committed crate, because the client syncs a delta against `HEAD`.
fn checkout() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("checkout dir");
    for (rel, text) in [
        ("Cargo.toml", MANIFEST),
        ("src/lib.rs", LIB),
        ("src/config.rs", CONFIG),
        ("src/wide.rs", WIDE),
    ] {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
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
    // Git keeps the CRLF breaks as written.
    git(&["config", "core.autocrlf", "false"]);
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

/// The text of a `code_slice` call, or its error.
async fn slice(addr: SocketAddr, root: &Path, args: serde_json::Value) -> String {
    match prod_code_mcp::tools::execute_tool(addr, root, "code_slice", args).await {
        Ok(result) => result
            .content
            .iter()
            .map(|item| {
                let prod_code_mcp::protocol::McpContentItem::Text { text } = item;
                text.clone()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Err(e) => format!("error: {e:#}"),
    }
}

/// Asks until the answer starts with `head` and holds every one of `parts`: until the engine
/// has loaded the crate its answers are empty or time out, and the slice says so. Every
/// attempt's first line is logged; the last answer is returned either way.
async fn slice_until(
    addr: SocketAddr,
    root: &Path,
    args: serde_json::Value,
    head: &str,
    parts: &[&str],
) -> String {
    let mut last = String::new();
    for attempt in 1..=60 {
        last = slice(addr, root, args.clone()).await;
        eprintln!(
            "{args} attempt {attempt}: {}",
            last.lines().next().unwrap_or_default()
        );
        if last.starts_with(head) && parts.iter().all(|p| last.contains(p)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }
    eprintln!("{last}\n");
    last
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn code_slice_on_a_real_rust_engine_is_complete_bounded_or_refused_as_it_should_be() {
    let gateway = Gateway::start();
    let (_dir, root) = checkout();
    let addr = gateway.addr;
    eprintln!("gateway from this revision listening on {addr}");

    // The seed, the function it calls and the struct in another file it names; the parameter
    // and the local resolve inside the seed. `pub mod config;` has an inverted range from this
    // engine and costs lib.rs nothing, since modules are not sliced.
    let full = serde_json::json!({ "path": "src/lib.rs", "line": 6, "character": 8, "depth": 3 });
    let text = slice_until(
        addr,
        &root,
        full,
        "slice of `seed`: 3 item(s)",
        &["[function] dependency", "[struct] Config"],
    )
    .await;
    assert!(text.starts_with("slice of `seed`: 3 item(s)"), "{text}");
    for part in [
        "[function] seed",
        "[function] dependency",
        "=== src/config.rs",
        "[struct] Config",
        "% smaller",
    ] {
        assert!(text.contains(part), "{part}: {text}");
    }
    for absent in ["unrelated", "INCOMPLETE", "BOUNDED", "missing evidence"] {
        assert!(!text.contains(absent), "{absent}: {text}");
    }

    // Line-only navigation: column 1 of a line inside the body finds the same seed.
    let line_only =
        serde_json::json!({ "path": "src/lib.rs", "line": 7, "character": 1, "depth": 3 });
    let text = slice_until(addr, &root, line_only, "slice of `seed`: 3 item(s)", &[]).await;
    assert!(text.starts_with("slice of `seed`: 3 item(s)"), "{text}");

    // CRLF breaks and a name after wide characters: the engine's positions fit the file, the
    // call is followed, and the slice is complete.
    let wide = serde_json::json!({ "path": "src/wide.rs", "line": 1, "character": 8, "depth": 2 });
    let text = slice_until(
        addr,
        &root,
        wide,
        "slice of `wide_seed`: 2 item(s)",
        &["[function] wide_dep"],
    )
    .await;
    assert!(
        text.starts_with("slice of `wide_seed`: 2 item(s)"),
        "{text}"
    );
    assert!(text.contains("[function] wide_dep"), "{text}");
    assert!(!text.contains("malformed"), "{text}");

    // Depth 0: the seed alone, with complete evidence, but bounded and saying where.
    let shallow =
        serde_json::json!({ "path": "src/lib.rs", "line": 6, "character": 8, "depth": 0 });
    let text = slice_until(
        addr,
        &root,
        shallow,
        "BOUNDED slice of `seed`: 1 item(s)",
        &[],
    )
    .await;
    assert!(
        text.starts_with("BOUNDED slice of `seed`: 1 item(s)"),
        "{text}"
    );
    assert!(
        text.contains("depth limit 0 reached: the dependencies of 1 item(s) at depth 0"),
        "{text}"
    );
    assert!(text.contains("[function] seed"), "{text}");

    // A column the line cannot hold is refused rather than read as "somewhere on line 6".
    let past = serde_json::json!({ "path": "src/lib.rs", "line": 6, "character": 200, "depth": 1 });
    let text = slice(addr, &root, past).await;
    let line_6 = "pub fn seed(cfg: &Config) -> u32 {";
    assert!(
        text.contains(&format!(
            "no declaration at src/lib.rs:6:200: the seed position 6:200 is past the end of \
             line 6, which is {} UTF-16 unit(s) long",
            line_6.len()
        )),
        "{text}"
    );
    assert!(!text.contains("[function] seed"), "{text}");
}
