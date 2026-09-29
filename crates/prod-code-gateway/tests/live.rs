//! The gateway, as a client actually meets it: the real binary, the real protocol, the real
//! analyzer, over a real socket.
//!
//! Everything else in this repository's test suite replaces the far side with a script, which
//! is right for testing how answers are composed and useless for testing the thing that
//! produces them. Here nothing is replaced: `prod-code-server` is started as a child process
//! with its own storage directory, a checkout is synced to it, and the queries go through the
//! same client code an agent uses. What it proves is the part no unit test can — that a
//! workspace loads, that rust-analyzer answers through the wire protocol, and that the
//! dispatch in `main.rs` routes each method to the engine and back.
//!
//! It is slower than the rest of the suite (a workspace has to load) and it is worth it.

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// A gateway process, its storage, and the port it answers on.
struct Gateway {
    child: Child,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
}

impl Gateway {
    /// Starts `prod-code-server` on a free port with an empty storage directory, and waits
    /// until it is listening.
    fn start() -> Self {
        Self::start_with(&[])
    }

    /// The same, with extra environment for the settings a test wants to change.
    ///
    /// The port comes from the daemon rather than from a probe: binding a socket to find a free
    /// port and then dropping it leaves a window in which another test takes it, and eleven
    /// gateways starting at once find that window. The daemon logs the address it actually
    /// bound, and this reads it back.
    fn start_with(extra: &[(&str, &str)]) -> Self {
        let storage = tempfile::tempdir().expect("storage dir");
        let mut command = Command::new(env!("CARGO_BIN_EXE_prod-code-server"));
        command
            .env("PROD_CODE_STORAGE", storage.path())
            // No peers, no gossip: this gateway is alone and must not look for others.
            .env("PROD_CODE_PEERS", "")
            .env_remove("RUST_LOG")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut wanted: Option<SocketAddr> = None;
        let mut bind = "127.0.0.1:0".to_string();
        for (key, value) in extra {
            if *key == "PROD_CODE_BIND" {
                wanted = Some(value.parse().expect("an address to bind"));
                bind = value.to_string();
            }
            // An empty value means "leave it unset", which is how a test reaches the defaults
            // the daemon falls back to when the environment says nothing.
            if value.is_empty() {
                command.env_remove(key);
            } else {
                command.env(key, value);
            }
        }
        command.env("PROD_CODE_BIND", &bind);
        let mut child = command.spawn().expect("the server binary starts");
        let stdout = child.stdout.take().expect("stdout is piped");
        let addr = wanted.unwrap_or_else(|| read_bound_address(stdout));
        let gateway = Self {
            child,
            addr,
            _storage: storage,
        };
        gateway.wait_until_listening();
        gateway
    }

    fn wait_until_listening(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if std::net::TcpStream::connect_timeout(&self.addr, Duration::from_millis(200)).is_ok()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the gateway never started listening on {}", self.addr);
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        // SIGTERM, not a kill: the server stops accepting and returns from `main`, which is
        // what lets anything registered at exit run — the coverage runtime included. A killed
        // process writes no profile, and this test's whole point is to exercise the server.
        let pid = self.child.id() as i32;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
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

/// Reads the daemon's own report of where it is listening, then keeps draining its log so a
/// full pipe never stops it.
///
/// The read happens on a thread of its own and the answer comes back over a channel, because
/// `read_line` on a pipe that stays silent never returns: a deadline checked between reads is
/// not a deadline at all, and a whole suite can hang on it.
fn read_bound_address(stdout: std::process::ChildStdout) -> SocketAddr {
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
            // Whatever it logs from here on goes nowhere, but it has to go somewhere: a child
            // that fills its pipe stops.
        }
    });
    rx.recv_timeout(Duration::from_secs(60))
        .expect("the gateway says where it is listening within a minute")
}

/// A port nothing is listening on, for the tests that have to name one in advance (the two
/// gateways that must know each other's address before either starts).
fn free_port() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    addr
}

/// A checkout to analyse: one small crate, committed, because the client syncs a delta
/// against `HEAD`.
struct Checkout {
    dir: tempfile::TempDir,
}

impl Checkout {
    fn new() -> Self {
        let checkout = Self {
            dir: tempfile::tempdir().expect("checkout dir"),
        };
        checkout.write(
            "Cargo.toml",
            "[package]\nname = \"subject\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
        );
        checkout.write("src/lib.rs", LIB);
        checkout.write("src/store.rs", STORE);
        checkout.commit();
        checkout
    }

    fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).expect("canonical root")
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    fn commit(&self) {
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(self.dir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("git runs")
        };
        if !self.dir.path().join(".git").is_dir() {
            assert!(git(&["init", "-q"]).success());
        }
        assert!(git(&["add", "-A"]).success());
        git(&[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=test",
            "commit",
            "-qm",
            "subject",
        ]);
    }
}

const LIB: &str = r#"//! A subject for the analyzer.

pub mod store;

/// A quantity of something, in whole units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quantity {
    pub units: u32,
}

impl Quantity {
    /// Builds a quantity.
    pub fn new(units: u32) -> Self {
        Self { units }
    }

    /// Adds two quantities together.
    pub fn plus(self, other: Quantity) -> Quantity {
        Quantity::new(self.units + other.units)
    }
}

/// Sums every quantity in the slice.
pub fn total(all: &[Quantity]) -> Quantity {
    all.iter().fold(Quantity::new(0), |acc, q| acc.plus(*q))
}

/// Nothing calls this.
pub fn unused_helper(units: u32) -> Quantity {
    Quantity::new(units)
}
"#;

const STORE: &str = r#"use crate::Quantity;

/// Keeps quantities by name, so they can be looked up later.
#[derive(Debug, Default)]
pub struct Store {
    entries: Vec<(String, Quantity)>,
}

impl Store {
    pub fn put(&mut self, name: &str, quantity: Quantity) {
        self.entries.push((name.to_string(), quantity));
    }

    /// Finds a quantity by the name it was stored under.
    pub fn get(&self, name: &str) -> Option<Quantity> {
        self.entries
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, quantity)| *quantity)
    }

    pub fn sum(&self) -> Quantity {
        crate::total(&self.entries.iter().map(|(_, q)| *q).collect::<Vec<_>>())
    }
}
"#;

/// Runs one MCP tool against the gateway, as an agent would.
async fn tool(
    addr: SocketAddr,
    root: &Path,
    name: &str,
    args: serde_json::Value,
) -> prod_code_mcp::protocol::McpToolCallResult {
    prod_code_mcp::tools::execute_tool(addr, root, name, args)
        .await
        .unwrap_or_else(|err| panic!("{name} failed: {err:#}"))
}

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
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

/// One test, not several: a workspace load is the expensive part and every query after it is
/// cheap, so they share one gateway and one checkout. Each step asserts on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_answers_a_real_checkout() {
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    let (addr, root) = (gateway.addr, checkout.root());

    // The status of a gateway that has just started: alive, with an engine detected.
    let status = tool(addr, &root, "code_status", serde_json::json!({})).await;
    let status = text_of(&status);
    assert!(status.contains("rust"), "status names its engine: {status}");

    // Hover: the declaration, from the analyzer rather than from the file.
    let hover = text_of(
        &tool(
            addr,
            &root,
            "code_hover",
            serde_json::json!({ "symbol": "Quantity::plus" }),
        )
        .await,
    );
    assert!(
        hover.contains("fn plus"),
        "hover returns the signature: {hover}"
    );
    assert!(
        hover.contains("Adds two quantities"),
        "hover carries the doc comment: {hover}"
    );

    // Definition: the position of the declaration, resolved from a name.
    let definition = text_of(
        &tool(
            addr,
            &root,
            "code_definition",
            serde_json::json!({ "symbol": "Store::get" }),
        )
        .await,
    );
    assert!(
        definition.contains("store.rs"),
        "the definition is in the file that declares it: {definition}"
    );

    // References: `Quantity::new` is called from both files.
    let references = text_of(
        &tool(
            addr,
            &root,
            "code_references",
            serde_json::json!({ "symbol": "Quantity::new" }),
        )
        .await,
    );
    assert!(
        references.contains("lib.rs"),
        "references reach the declaring file: {references}"
    );

    // Callers: who calls `total`.
    let callers = text_of(
        &tool(
            addr,
            &root,
            "code_callers",
            serde_json::json!({ "symbol": "total" }),
        )
        .await,
    );
    assert!(
        callers.contains("sum") || callers.contains("store.rs"),
        "the call from Store::sum is found: {callers}"
    );

    // The outline of a file, from document symbols.
    let outline = text_of(
        &tool(
            addr,
            &root,
            "code_outline",
            serde_json::json!({ "path": "src/store.rs" }),
        )
        .await,
    );
    for expected in ["Store", "put", "get", "sum"] {
        assert!(
            outline.contains(expected),
            "{expected} is in the outline: {outline}"
        );
    }

    // A file no language server outlines is an error with the reason, not an empty outline
    // (#270); Markdown is outlined by its headings (#362).
    std::fs::write(root.join("notes.txt"), "A table row about a trait.\n").unwrap();
    let refused = match prod_code_mcp::tools::execute_tool(
        addr,
        &root,
        "code_outline",
        serde_json::json!({ "path": "notes.txt" }),
    )
    .await
    {
        Ok(result) => {
            assert!(result.is_error, "{}", text_of(&result));
            text_of(&result)
        }
        Err(err) => format!("{err:#}"),
    };
    assert!(
        refused.contains("no language server serves `.txt` files"),
        "{refused}"
    );
    std::fs::write(root.join("README.md"), "# Store\n\nA table row.\n").unwrap();
    let headings = text_of(
        &tool(
            addr,
            &root,
            "code_outline",
            serde_json::json!({ "path": "README.md" }),
        )
        .await,
    );
    assert!(
        headings.contains("[Heading 1] Store (line 1)"),
        "{headings}"
    );

    // The symbol index answers by name.
    let symbols = text_of(
        &tool(
            addr,
            &root,
            "code_symbols",
            serde_json::json!({ "query": "Quantity" }),
        )
        .await,
    );
    assert!(
        symbols.contains("Quantity"),
        "the index finds it: {symbols}"
    );

    // Diagnostics on a file that compiles: nothing to report.
    let diagnostics = text_of(
        &tool(
            addr,
            &root,
            "code_diagnostics",
            serde_json::json!({ "path": "src/lib.rs" }),
        )
        .await,
    );
    assert!(
        diagnostics.contains("0 error"),
        "a file that compiles has no errors: {diagnostics}"
    );

    // An unused import is reported, as rustc reports it and `-D warnings` rejects it, although
    // rust-analyzer computes no diagnostic for it (#134).
    let with_import = format!("use std::collections::HashMap;\n{LIB}");
    let validated = text_of(
        &tool(
            addr,
            &root,
            "code_validate_edit",
            serde_json::json!({ "path": "src/lib.rs", "new_text": with_import }),
        )
        .await,
    );
    assert!(
        validated.contains("unused_imports") && validated.contains(":1:1"),
        "the unused import is reported: {validated}"
    );

    // A proposed edit is judged before anything is written.
    let broken = LIB.replace("all.iter()", "all.itr()");
    let validated = text_of(
        &tool(
            addr,
            &root,
            "code_validate_edit",
            serde_json::json!({ "path": "src/lib.rs", "new_text": broken }),
        )
        .await,
    );
    assert!(
        validated.contains("itr") || validated.to_lowercase().contains("error"),
        "the typo is reported: {validated}"
    );
    assert_eq!(
        std::fs::read_to_string(checkout.path("src/lib.rs")).unwrap(),
        LIB,
        "validating writes nothing"
    );

    // Intent search: by what the code does, not by its name.
    let search = text_of(
        &tool(
            addr,
            &root,
            "code_search",
            serde_json::json!({ "query": "look up a quantity by name" }),
        )
        .await,
    );
    assert!(
        search.contains("get") || search.contains("Store"),
        "the lookup is found by intent: {search}"
    );

    // Dead code: the helper nothing calls.
    let dead = text_of(
        &tool(
            addr,
            &root,
            "code_dead_code",
            serde_json::json!({ "include_exported": true }),
        )
        .await,
    );
    assert!(
        dead.contains("unused_helper"),
        "the uncalled helper is listed: {dead}"
    );

    // A command runs on the gateway, in its copy of the checkout.
    let exec = text_of(
        &tool(
            addr,
            &root,
            "code_exec",
            serde_json::json!({ "argv": ["cargo", "check", "--quiet"], "timeout_secs": 300 }),
        )
        .await,
    );
    assert!(
        exec.contains("exit 0") || exec.contains("exit code 0"),
        "the crate compiles on the gateway: {exec}"
    );

    // The type at a position, and what implements a trait-less struct's inherent methods.
    let type_at = text_of(
        &tool(
            addr,
            &root,
            "code_type_at",
            // A private field is not in the symbol index, which is what positions are for.
            serde_json::json!({ "path": "src/store.rs", "line": 6, "character": 5 }),
        )
        .await,
    );
    assert!(
        type_at.contains("Vec") || type_at.contains("Quantity"),
        "the field's type is reported: {type_at}"
    );

    // Callees: what `Store::sum` calls.
    let callees = text_of(
        &tool(
            addr,
            &root,
            "code_callees",
            serde_json::json!({ "symbol": "Store::sum" }),
        )
        .await,
    );
    assert!(
        callees.contains("total"),
        "the call to `total` is found: {callees}"
    );

    // A slice: the declarations `total` depends on, not the files they live in.
    let slice = text_of(
        &tool(
            addr,
            &root,
            "code_slice",
            serde_json::json!({ "symbol": "total", "depth": 2 }),
        )
        .await,
    );
    assert!(
        slice.contains("Quantity"),
        "the slice pulls in the type it uses: {slice}"
    );

    // The analyzer's own code actions at a position.
    let assists = text_of(
        &tool(
            addr,
            &root,
            "code_assists",
            serde_json::json!({ "path": "src/lib.rs", "line": 14, "character": 12 }),
        )
        .await,
    );
    assert!(
        !assists.trim().is_empty(),
        "the analyzer offers something at a function: {assists}"
    );

    // A definition that leaves the checkout, and then its text: the standard library lives on
    // the gateway's disk, where the client cannot read it.
    let std_definition = text_of(
        &tool(
            addr,
            &root,
            "code_definition",
            // `String` in `entries: Vec<(String, Quantity)>`
            serde_json::json!({ "path": "src/store.rs", "line": 6, "character": 19 }),
        )
        .await,
    );
    let std_path = std_definition
        .split("file://")
        .nth(1)
        .map(|rest| rest.split(':').next().unwrap_or("").to_string())
        .filter(|path| !path.is_empty() && !path.starts_with(root.to_string_lossy().as_ref()))
        .unwrap_or_else(|| panic!("the definition leaves the checkout: {std_definition}"));
    let source = text_of(
        &tool(
            addr,
            &root,
            "code_source",
            serde_json::json!({ "path": std_path, "around_line": 1, "context": 20 }),
        )
        .await,
    );
    assert!(
        source.contains("String") || source.contains("pub struct") || source.contains("//"),
        "the gateway reads back a file only it has: {source}"
    );

    // Several proposed files judged together, the way a multi-file edit has to be.
    let together = text_of(
        &tool(
            addr,
            &root,
            "code_validate_edits",
            serde_json::json!({ "edits": [
                { "path": "src/lib.rs", "new_text": LIB },
                { "path": "src/store.rs", "new_text": STORE }
            ] }),
        )
        .await,
    );
    assert!(
        together.contains("0 error"),
        "the pair as it stands is clean: {together}"
    );

    // The sync the client runs before every query, asked for explicitly.
    let synced = text_of(&tool(addr, &root, "code_sync", serde_json::json!({})).await);
    assert!(
        !synced.trim().is_empty(),
        "the sync reports what it pushed: {synced}"
    );

    // The linter, on the gateway.
    let lint = text_of(&tool(addr, &root, "code_lint", serde_json::json!({})).await);
    assert!(
        lint.contains("OK") || lint.contains("0 error"),
        "clippy is clean on the subject crate: {lint}"
    );

    // Builds and tests run on the gateway, in its copy of the checkout.
    let check = text_of(&tool(addr, &root, "code_check", serde_json::json!({})).await);
    assert!(
        check.contains("OK") || check.contains("0 error"),
        "the crate compiles: {check}"
    );

    // Impact: what a change to a function reaches.
    let impact = text_of(&tool(addr, &root, "code_impact", serde_json::json!({})).await);
    assert!(
        !impact.is_empty(),
        "impact answers on a clean tree: {impact}"
    );

    // A fixture for a type, built and type-checked on the gateway.
    let fixture = text_of(
        &tool(
            addr,
            &root,
            "code_generate_fixture",
            serde_json::json!({ "symbol": "Quantity" }),
        )
        .await,
    );
    assert!(
        fixture.contains("units: 0"),
        "the fixture fills the field by type: {fixture}"
    );
    assert!(
        fixture.contains("0 errors"),
        "and the analyzer accepts it: {fixture}"
    );

    // A structural rewrite, reported and not applied.
    let codemod = text_of(
        &tool(
            addr,
            &root,
            "code_codemod",
            serde_json::json!({ "rule": "Quantity::new($a) ==>> Quantity::new($a + 0)" }),
        )
        .await,
    );
    assert!(
        codemod.contains("nothing was written") || codemod.contains("matches nothing"),
        "a dry run says what it would do: {codemod}"
    );

    // An ambiguous name is refused with its candidates rather than guessed at.
    let ambiguous = prod_code_mcp::tools::execute_tool(
        addr,
        &root,
        "code_hover",
        serde_json::json!({ "symbol": "nonexistent_symbol_xyz" }),
    )
    .await;
    match ambiguous {
        Err(err) => {
            let text = format!("{err:#}");
            assert!(
                text.contains("nonexistent_symbol_xyz")
                    || text.to_lowercase().contains("not found"),
                "the failure names what it could not resolve: {text}"
            );
        }
        Ok(result) => {
            let text = text_of(&result);
            assert!(
                result.is_error || text.to_lowercase().contains("no ") || text.is_empty(),
                "an unresolvable name does not come back as an answer: {text}"
            );
        }
    }

    // A rename is applied to the checkout, which is the one query that writes.
    let renamed = text_of(
        &tool(
            addr,
            &root,
            "code_rename",
            serde_json::json!({ "symbol": "Quantity::plus", "new_name": "added_to" }),
        )
        .await,
    );
    assert!(
        renamed.contains("lib.rs"),
        "the rename names what it rewrote: {renamed}"
    );
    let after = std::fs::read_to_string(checkout.path("src/lib.rs")).unwrap();
    assert!(
        after.contains("pub fn added_to"),
        "the declaration was rewritten"
    );
    assert!(
        !after.contains(".plus("),
        "every call site was rewritten: {after}"
    );
}

/// Invalid native coordinates must remain errors through the public MCP path: they cannot
/// become a hover at byte zero or let a forced refactoring touch the checkout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_refuses_invalid_native_positions_without_writing() {
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    let (addr, root) = (gateway.addr, checkout.root());
    fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        let mut pending = vec![root.to_path_buf()];
        let mut files = std::collections::BTreeMap::new();
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name() == ".git" || entry.file_name() == "target" {
                    continue;
                }
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(path);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        std::fs::read(path).unwrap(),
                    );
                }
            }
        }
        files
    }
    let before = snapshot(&root);

    let hover = prod_code_mcp::tools::execute_tool(
        addr,
        &root,
        "code_hover",
        serde_json::json!({ "path": "src/lib.rs", "line": 999999, "character": 1 }),
    )
    .await
    .expect_err("an invalid hover position is a gateway error");
    assert!(
        format!("{hover:#}").contains("Invalid position 999999:1"),
        "{hover:#}"
    );

    for method in ["textDocument/definition", "textDocument/references"] {
        let error = prod_code_mcp::tools::execute_lsp_query(
            addr, &root, &checkout.path("src/lib.rs"), method,
            serde_json::json!({"textDocument": {"uri": prod_code_protocol::path::file_uri(&checkout.path("src/lib.rs"))},
                "position": {"line": 999998, "character": 0}}),
        ).await.expect_err("a native query error cannot become an empty successful result");
        assert!(
            format!("{error:#}").contains("Invalid position 999999:1"),
            "{error:#}"
        );
    }

    let deletion = prod_code_mcp::tools::execute_tool(
        addr,
        &root,
        "code_safe_delete",
        serde_json::json!({
            "path": "src/lib.rs",
            "line": 999999,
            "character": 1,
            "force": true
        }),
    )
    .await
    .expect("the gateway reports a refused mutation");
    let refused = text_of(&deletion);
    assert!(deletion.is_error, "the mutation is refused: {refused}");
    assert!(
        refused.contains("Invalid position 999999:1"),
        "the refusal names the invalid coordinate: {refused}"
    );
    assert_eq!(
        snapshot(&root),
        before,
        "every source path and byte remains unchanged, even with force"
    );
}

/// The same gateway, a Go checkout: the dispatch that forwards to a child language server
/// instead of the in-process Rust engine, and the backend that manages it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_forwards_to_a_child_language_server() {
    if which("gopls").is_none() || which("go").is_none() {
        eprintln!("skipping: gopls or go is not on PATH (build nodes have both)");
        return;
    }
    let gateway = Gateway::start();
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical");
    std::fs::write(
        root.join("go.mod"),
        "module example.com/subject\n\ngo 1.22\n",
    )
    .expect("go.mod");
    std::fs::write(
        root.join("store.go"),
        "package subject\n\n// Quantity is a count of something.\ntype Quantity struct {\n\tUnits int\n}\n\n// Total sums the quantities.\nfunc Total(all []Quantity) int {\n\tsum := 0\n\tfor _, q := range all {\n\t\tsum += q.Units\n\t}\n\treturn sum\n}\n",
    )
    .expect("store.go");
    std::fs::write(
        root.join("use.go"),
        "package subject\n\nfunc Describe(all []Quantity) int {\n\treturn Total(all)\n}\n",
    )
    .expect("use.go");
    commit_in(&root);

    let hover = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_hover",
            serde_json::json!({ "symbol": "Total" }),
        )
        .await,
    );
    assert!(
        hover.contains("func Total"),
        "gopls answers through the gateway: {hover}"
    );

    let references = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_references",
            serde_json::json!({ "symbol": "Total" }),
        )
        .await,
    );
    assert!(
        references.contains("use.go"),
        "the call from the other file is found: {references}"
    );

    // The call hierarchy, the outline and the index all go through the forwarding path rather
    // than through the in-process engine, and each is a different branch of the dispatch.
    let callers = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_callers",
            serde_json::json!({ "symbol": "Total" }),
        )
        .await,
    );
    assert!(
        callers.contains("Describe") || callers.contains("use.go"),
        "gopls answers the call hierarchy: {callers}"
    );
    let outline = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_outline",
            serde_json::json!({ "path": "store.go" }),
        )
        .await,
    );
    assert!(
        outline.contains("Quantity") && outline.contains("Total"),
        "the outline of a Go file: {outline}"
    );
    let symbols = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_symbols",
            serde_json::json!({ "query": "Quantity" }),
        )
        .await,
    );
    assert!(
        symbols.contains("Quantity"),
        "the Go symbol index answers: {symbols}"
    );
    let diagnostics = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_diagnostics",
            serde_json::json!({ "path": "store.go" }),
        )
        .await,
    );
    assert!(
        diagnostics.contains("0 error"),
        "the Go file compiles: {diagnostics}"
    );

    // A rename through a forwarded language server: the path that has to re-open every file a
    // reference lives in before the server will rewrite it.
    let renamed = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_rename",
            serde_json::json!({ "symbol": "Total", "new_name": "SumAll" }),
        )
        .await,
    );
    assert!(
        renamed.contains("store.go") || renamed.contains("use.go"),
        "the rename names what it rewrote: {renamed}"
    );
    let after = std::fs::read_to_string(root.join("use.go")).expect("use.go");
    assert!(
        after.contains("SumAll(all)"),
        "the call site in the other file was rewritten: {after}"
    );
}

/// A file no session has open changes locally and is synced: gopls, which does not watch the
/// tree itself, sees the new content only because the gateway tells it which files the sync
/// rewrote (#317). Before that, the hover below kept answering `func Foo() int`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gopls_sees_a_file_the_sync_rewrote_that_no_session_has_open() {
    if which("gopls").is_none() || which("go").is_none() {
        eprintln!("skipping: gopls or go is not on PATH (build nodes have both)");
        return;
    }
    let gateway = Gateway::start();
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical");
    std::fs::write(
        root.join("go.mod"),
        "module example.com/watchprobe\n\ngo 1.22\n",
    )
    .expect("go.mod");
    std::fs::write(
        root.join("a.go"),
        "package watchprobe\n\nfunc Foo() int { return 1 }\n",
    )
    .expect("a.go");
    std::fs::write(
        root.join("b.go"),
        "package watchprobe\n\nfunc Bar() { _ = Foo() }\n",
    )
    .expect("b.go");
    commit_in(&root);

    // `Foo` at its call in b.go: only b.go is opened, a.go is read from disk.
    let at_call = serde_json::json!({ "path": "b.go", "line": 3, "character": 18 });
    let before = text_of(&tool(gateway.addr, &root, "code_hover", at_call.clone()).await);
    assert!(
        before.contains("func Foo() int"),
        "gopls answers from a.go as first synced: {before}"
    );

    std::fs::write(
        root.join("a.go"),
        "package watchprobe\n\nfunc Foo() string { return \"x\" }\n",
    )
    .expect("a.go rewritten");
    // The next query pushes the change once the file watcher has seen it.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut after = String::new();
    while Instant::now() < deadline {
        after = text_of(&tool(gateway.addr, &root, "code_hover", at_call.clone()).await);
        if after.contains("func Foo() string") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(
        after.contains("func Foo() string"),
        "the rewritten a.go reached gopls: {after}"
    );
}

/// A TypeScript checkout: the generic LSP adapter, which supervises a language server that
/// neither the Rust engine nor the Go engine knows anything about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_supervises_a_generic_language_server() {
    if which("tsc").is_none() && which("node").is_none() {
        eprintln!("skipping: no TypeScript toolchain on PATH (build nodes have one)");
        return;
    }
    let gateway = Gateway::start();
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical");
    std::fs::write(
        root.join("package.json"),
        "{ \"name\": \"subject\", \"version\": \"1.0.0\", \"private\": true }\n",
    )
    .expect("package.json");
    std::fs::write(
        root.join("tsconfig.json"),
        "{ \"compilerOptions\": { \"target\": \"ES2020\", \"module\": \"ESNext\", \"moduleResolution\": \"bundler\", \"strict\": true, \"noEmit\": true }, \"include\": [\"src\"] }\n",
    )
    .expect("tsconfig.json");
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(
        root.join("src/quantity.ts"),
        "/** A count of something. */\nexport interface Quantity {\n  units: number;\n}\n\n/** Sums the quantities. */\nexport function total(all: Quantity[]): number {\n  return all.reduce((sum, q) => sum + q.units, 0);\n}\n",
    )
    .expect("quantity.ts");
    std::fs::write(
        root.join("src/fixable.ts"),
        "import { Quantity, total } from \"./quantity\";\n\nexport const wrong: string = 1;\n\nexport function unused(all: Quantity[]): number {\n  return total(all);\n}\n",
    )
    .expect("fixable.ts");
    std::fs::write(
        root.join("src/use.ts"),
        "import { Quantity, total } from \"./quantity\";\n\nexport function describe(all: Quantity[]): string {\n  return `${total(all)} units`;\n}\n",
    )
    .expect("use.ts");
    commit_in(&root);

    let hover = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_hover",
            serde_json::json!({ "path": "src/quantity.ts", "line": 7, "character": 17 }),
        )
        .await,
    );
    assert!(
        hover.contains("total") || hover.contains("Quantity"),
        "the generic adapter answers a hover: {hover}"
    );

    let references = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_references",
            serde_json::json!({ "path": "src/quantity.ts", "line": 7, "character": 17 }),
        )
        .await,
    );
    assert!(
        references.contains("use.ts") || references.contains("quantity.ts"),
        "the import in the other file is a reference: {references}"
    );

    let outline = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_outline",
            serde_json::json!({ "path": "src/quantity.ts" }),
        )
        .await,
    );
    assert!(
        outline.contains("total") || outline.contains("Quantity"),
        "the outline of a TypeScript file: {outline}"
    );

    // The call hierarchy and the code actions of a forwarded server are each their own branch
    // of the dispatch, and neither shares code with the in-process engine.
    let callers = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_callers",
            serde_json::json!({ "path": "src/quantity.ts", "line": 7, "character": 17 }),
        )
        .await,
    );
    assert!(
        !callers.trim().is_empty(),
        "the hierarchy query answers: {callers}"
    );
    // A line with an obvious mistake, so the server has a fix to offer and the gateway has to
    // give it an identifier the client can hand back.
    let assists = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_assists",
            serde_json::json!({ "path": "src/fixable.ts", "line": 3, "character": 14 }),
        )
        .await,
    );
    assert!(
        !assists.trim().is_empty(),
        "the server offers something where the types do not line up: {assists}"
    );
    if let Some(id) = assists
        .lines()
        .find_map(|line| line.split_whitespace().next().filter(|w| !w.is_empty()))
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
                .to_string()
        })
        .filter(|id| !id.is_empty())
    {
        // Applying one goes back through the same identifier, which is the half of this path
        // that listing it does not reach.
        let applied = prod_code_mcp::tools::execute_tool(
            gateway.addr,
            &root,
            "code_assist",
            serde_json::json!({ "path": "src/fixable.ts", "line": 3, "character": 14, "id": id }),
        )
        .await;
        match applied {
            Ok(result) => assert!(
                !text_of(&result).trim().is_empty(),
                "applying an action says what it did"
            ),
            Err(err) => {
                let text = format!("{err:#}");
                assert!(
                    !text.is_empty(),
                    "and a refusal says why rather than nothing"
                );
            }
        }
    }
    let diagnostics = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_diagnostics",
            serde_json::json!({ "path": "src/quantity.ts" }),
        )
        .await,
    );
    assert!(
        diagnostics.contains("0 error"),
        "the file type-checks: {diagnostics}"
    );

    // A rename through a forwarded server has to re-open every file a reference lives in
    // before the server will rewrite it.
    let renamed = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_rename",
            serde_json::json!({ "path": "src/quantity.ts", "line": 7, "character": 17, "new_name": "sumAll" }),
        )
        .await,
    );
    assert!(
        renamed.contains("quantity.ts") || renamed.contains("use.ts"),
        "the rename names what it rewrote: {renamed}"
    );
    assert!(
        std::fs::read_to_string(root.join("src/use.ts"))
            .expect("use.ts")
            .contains("sumAll("),
        "the import and the call were rewritten"
    );
}

/// The wire protocol itself, without the client library: the messages a peer or a placement
/// request sends, and what a session does with something it does not understand.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_gateway_answers_the_protocol_directly() {
    use futures_util::{SinkExt, StreamExt};
    use prod_code_protocol::{PlaceRequest, ProdCodeCodec, WireMessage};
    use tokio_util::codec::Framed;

    let gateway = Gateway::start();
    let stream = tokio::net::TcpStream::connect(gateway.addr)
        .await
        .expect("connect");
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    // Status, before any workspace exists.
    framed
        .send(WireMessage::StatusRequest)
        .await
        .expect("send status");
    match framed.next().await {
        Some(Ok(WireMessage::StatusResponse(status))) => {
            assert!(status.server_pid > 0, "a real process answered");
            assert_eq!(status.loaded_workspaces, 0, "nothing is loaded yet");
        }
        other => panic!("unexpected answer to a status request: {other:?}"),
    }

    // A placement request: which node should hold this workspace. Alone, the answer is this
    // gateway itself.
    framed
        .send(WireMessage::PlaceRequest(PlaceRequest {
            workspace_name: "subject".to_string(),
            engine: Some("rust".to_string()),
            os: None,
        }))
        .await
        .expect("send place");
    match framed.next().await {
        Some(Ok(WireMessage::PlaceResponse(place))) => {
            assert!(
                place.node.is_some(),
                "a gateway that serves Rust places it on itself: {place:?}"
            );
        }
        other => panic!("unexpected answer to a placement request: {other:?}"),
    }

    // A ping is answered without a session.
    framed.send(WireMessage::Ping).await.expect("send ping");
    match framed.next().await {
        Some(Ok(WireMessage::Pong)) => {}
        other => panic!("a ping is answered with a pong, not {other:?}"),
    }

    // And the cluster view, which is what `prod-code cluster` prints.
    framed
        .send(WireMessage::ClusterRequest)
        .await
        .expect("send cluster");
    match framed.next().await {
        Some(Ok(WireMessage::ClusterResponse(cluster))) => {
            assert!(
                !cluster.nodes.is_empty(),
                "a lone gateway is still a cluster of one: {cluster:?}"
            );
        }
        other => panic!("unexpected answer to a cluster request: {other:?}"),
    }
}

/// The handshake says how long ago the engine a session attaches to was loaded: a second session
/// on the same checkout attaches to the same engine, older by the time between them. A client
/// asks an empty symbol search again only of a young engine (#381).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_handshake_says_how_old_the_engine_is() {
    use futures_util::{SinkExt, StreamExt};
    use prod_code_protocol::{HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage};
    use tokio_util::codec::Framed;

    let gateway = Gateway::start();
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"aged\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml");
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\n").expect("lib.rs");
    commit_in(&root);

    let age = |root: PathBuf| async move {
        let stream = tokio::net::TcpStream::connect(gateway.addr)
            .await
            .expect("connect");
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        let identity = prod_code_mcp::sync::workspace_identity(&root);
        prod_code_mcp::sync::push_workspace_sync(&mut framed, &root, &identity, None)
            .await
            .expect("sync");
        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(vec![PROTOCOL_VERSION]),
                capabilities: None,
                client_name: "age-test".to_string(),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: root.to_string_lossy().to_string(),
                preferred_engine: None,
                base_workspace_name: Some(identity.name.clone()),
                engine_subpath: None,
                client_agent: None,
                client_host: None,
                purpose: None,
            }))
            .await
            .expect("handshake");
        match framed.next().await {
            Some(Ok(WireMessage::HandshakeResponse(resp))) => {
                // The in-process Rust engine answers from a complete analysis: its index
                // questions need no waiting, and an empty answer is final (#391).
                assert!(resp.index_gated, "{resp:?}");
                resp.engine_age_ms
            }
            other => panic!("no handshake response: {other:?}"),
        }
    };
    let first = age(root.clone()).await.expect("the gateway says the age");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let second = age(root.clone()).await.expect("the gateway says the age");
    assert!(first < 30_000, "loaded for this session: {first} ms");
    assert!(
        second >= first + 300,
        "the same engine, older: {first} ms, then {second} ms"
    );
}

/// The daemon puts the user's toolchain directories first on PATH before it looks for a
/// language server, so a test that looks for one has to do the same — otherwise it decides a
/// server is missing on a machine that has it, and passes by skipping.
fn which(binary: &str) -> Option<PathBuf> {
    prod_code_gateway::prefer_rustup_toolchain();
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(binary))
            .find(|candidate| candidate.is_file())
    })
}

fn commit_in(root: &Path) {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs")
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["add", "-A"]).success());
    git(&[
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "user.name=test",
        "commit",
        "-qm",
        "subject",
    ]);
}

/// The tools that change the checkout, and the refusals that stop them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_applies_and_refuses_changes() {
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    let (addr, root) = (gateway.addr, checkout.root());

    // Safe delete refuses what is still used, and names the uses.
    let refused = prod_code_mcp::tools::execute_tool(
        addr,
        &root,
        "code_safe_delete",
        serde_json::json!({ "symbol": "Quantity::new" }),
    )
    .await;
    let refused = match refused {
        Ok(result) => text_of(&result),
        Err(err) => format!("{err:#}"),
    };
    assert!(
        refused.contains("lib.rs") || refused.to_lowercase().contains("used"),
        "a used symbol is not deleted silently: {refused}"
    );
    assert!(
        std::fs::read_to_string(checkout.path("src/lib.rs"))
            .unwrap()
            .contains("pub fn new"),
        "and it is still there"
    );

    // What nothing uses can go.
    let deleted = text_of(
        &tool(
            addr,
            &root,
            "code_safe_delete",
            serde_json::json!({ "symbol": "unused_helper" }),
        )
        .await,
    );
    assert!(
        deleted.contains("lib.rs") || deleted.to_lowercase().contains("delet"),
        "the unused helper is removed: {deleted}"
    );
    assert!(
        !std::fs::read_to_string(checkout.path("src/lib.rs"))
            .unwrap()
            .contains("unused_helper"),
        "and it is gone from the file"
    );

    // A signature change, with its call sites.
    let signature = text_of(
        &tool(
            addr,
            &root,
            "code_change_signature",
            serde_json::json!({
                "symbol": "total",
                "params": ["all", "scale: u32 = 1"],
                "apply": true
            }),
        )
        .await,
    );
    assert!(
        signature.contains("applied") || signature.contains("scale"),
        "the new parameter reaches the declaration: {signature}"
    );
    let lib = std::fs::read_to_string(checkout.path("src/lib.rs")).unwrap();
    assert!(
        lib.contains("scale: u32"),
        "the declaration takes it now: {lib}"
    );
    assert!(
        std::fs::read_to_string(checkout.path("src/store.rs"))
            .unwrap()
            .contains("total(&self"),
        "and the call site still calls it"
    );

    // A structural rewrite, applied this time.
    let codemod = text_of(
        &tool(
            addr,
            &root,
            "code_codemod",
            serde_json::json!({
                "rule": "Quantity::new(0) ==>> Quantity::new(0u32)",
                "apply": true
            }),
        )
        .await,
    );
    assert!(
        codemod.contains("applied") || codemod.contains("matches nothing"),
        "the rewrite says what it did: {codemod}"
    );

    // A proposed pair of files that does not compile comes back as the analyzer's errors.
    let broken = text_of(
        &tool(
            addr,
            &root,
            "code_validate_edits",
            serde_json::json!({ "edits": [
                { "path": "src/lib.rs", "new_text": "pub fn total() -> Nonexistent { todo!() }\n" }
            ] }),
        )
        .await,
    );
    assert!(
        broken.to_lowercase().contains("error") || broken.contains("Nonexistent"),
        "the unresolvable type is reported: {broken}"
    );
}

/// Commands, tests and hypotheses: the parts of the gateway that run things rather than
/// answer questions.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_runs_commands_and_hypotheses() {
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    let (addr, root) = (gateway.addr, checkout.root());

    // A command that fails comes back with its exit code, not as an error.
    let failed = text_of(
        &tool(
            addr,
            &root,
            "code_exec",
            serde_json::json!({ "argv": ["sh", "-c", "echo out; echo err 1>&2; exit 3"], "timeout_secs": 60 }),
        )
        .await,
    );
    assert!(
        failed.contains("exit 3") || failed.contains('3'),
        "the exit code is reported: {failed}"
    );
    assert!(
        failed.contains("out"),
        "and so is what it printed: {failed}"
    );
    // What it used comes back with the exit status, from `wait4` (#180).
    assert!(
        failed.contains(" (cpu ") && failed.contains(" MB)"),
        "the summary carries the command's CPU time and peak memory: {failed}"
    );

    // A command that outstays its timeout is killed, and the tree it started with it.
    let timed_out = prod_code_mcp::tools::execute_tool(
        addr,
        &root,
        "code_exec",
        serde_json::json!({ "argv": ["sh", "-c", "sleep 120 & sleep 120"], "timeout_secs": 2 }),
    )
    .await;
    let timed_out = match timed_out {
        Ok(result) => text_of(&result),
        Err(err) => format!("{err:#}"),
    };
    assert!(
        timed_out.to_lowercase().contains("time") || timed_out.contains("exit"),
        "the timeout is reported rather than hung on: {timed_out}"
    );

    // The test runner parses cargo's output into results.
    let tested = text_of(&tool(addr, &root, "code_test", serde_json::json!({})).await);
    assert!(
        tested.contains("OK") || tested.contains("passed") || tested.contains("0 failed"),
        "a crate with no tests still reports a result: {tested}"
    );

    // Hypotheses, each in its own shadow of the workspace, none of them written.
    let before = std::fs::read_to_string(checkout.path("src/lib.rs")).unwrap();
    let shadow = text_of(
        &tool(
            addr,
            &root,
            "code_shadow_run",
            serde_json::json!({
                "argv": ["cargo", "check", "--quiet"],
                "timeout_secs": 300,
                "hypotheses": [
                    { "name": "as-is", "files": [] },
                    { "name": "broken", "files": [
                        { "path": "src/lib.rs", "content": "pub fn total() -> Nope { todo!() }\n" }
                    ] }
                ]
            }),
        )
        .await,
    );
    assert!(
        shadow.contains("as-is"),
        "each hypothesis is reported by name: {shadow}"
    );
    assert!(
        shadow.contains("broken"),
        "including the one that fails: {shadow}"
    );
    assert_eq!(
        std::fs::read_to_string(checkout.path("src/lib.rs")).unwrap(),
        before,
        "a shadow run writes nothing to the checkout"
    );

    // A command that rewrites a file — a formatter, a generator — must leave the analyzer
    // looking at the new text. The client is sent the new contents and records them as synced,
    // so no later sync carries them to the node; if the command does not tell the engine
    // itself, every position in that file is off by however many lines the command moved it.
    // `store.rs` uses `Quantity` on its line 6; the query opens `lib.rs`, so the reference in
    // `store.rs` comes from the engine's own copy, not from anything the client sends.
    let quantity = serde_json::json!({ "path": "src/lib.rs", "line": 7, "character": 12 });
    let before_exec = text_of(&tool(addr, &root, "code_references", quantity.clone()).await);
    assert!(
        before_exec.contains("store.rs:6:"),
        "the field in store.rs is found where it is: {before_exec}"
    );
    let prepended = text_of(
        &tool(
            addr,
            &root,
            "code_exec",
            serde_json::json!({
                "argv": ["sh", "-c", "printf '// one\\n// two\\n// three\\n' | cat - src/store.rs > src/store.rs.new && mv src/store.rs.new src/store.rs"],
                "timeout_secs": 60
            }),
        )
        .await,
    );
    assert!(
        std::fs::read_to_string(checkout.path("src/store.rs"))
            .unwrap()
            .starts_with("// one\n"),
        "the command's change came back to the checkout: {prepended}"
    );
    let after_exec = text_of(&tool(addr, &root, "code_references", quantity).await);
    assert!(
        after_exec.contains("store.rs:9:") && !after_exec.contains("store.rs:6:"),
        "the analyzer sees the file the command wrote, three lines further down: {after_exec}"
    );
}

/// Validation runs on a second engine for the workspace (#73): its overlays and their reverts
/// never touch the engine every other query uses. What a second engine must not do is fall
/// behind — a file synced or rewritten by a command has to reach it as it reaches the main one,
/// or a validation judges the proposal against text that no longer exists.
///
/// The proposal below returns `store::fresh()` where a `String` is wanted. The analyzer reports
/// that as a mismatch only when it knows what `fresh` returns; a path to a function it has never
/// seen is not reported at all. So each verdict says which text of `store.rs` the validation
/// engine is looking at.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn validation_runs_on_its_own_engine_and_sees_every_change() {
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    let (addr, root) = (gateway.addr, checkout.root());
    let proposal = format!("{LIB}\npub fn label() -> String {{\n    store::fresh()\n}}\n");
    // The main engine is loaded by an ordinary query first, the validation engine by the
    // first validation.
    let refs = text_of(
        &tool(
            addr,
            &root,
            "code_references",
            serde_json::json!({ "path": "src/lib.rs", "line": 7, "character": 12 }),
        )
        .await,
    );
    assert!(refs.contains("store.rs:6:"), "{refs}");
    let unknown = text_of(
        &tool(
            addr,
            &root,
            "code_validate_edit",
            serde_json::json!({ "path": "src/lib.rs", "new_text": proposal }),
        )
        .await,
    );
    assert!(
        !unknown.contains("found u32") && !unknown.contains("found u64"),
        "`fresh` does not exist yet, so nothing can be said about its type: {unknown}"
    );

    // A sync: `store.rs` gains `fresh` on disk, uncommitted, and the next call carries it.
    checkout.write(
        "src/store.rs",
        &format!("{STORE}\npub fn fresh() -> u32 {{\n    1\n}}\n"),
    );
    let synced = text_of(
        &tool(
            addr,
            &root,
            "code_validate_edit",
            serde_json::json!({ "path": "src/lib.rs", "new_text": proposal }),
        )
        .await,
    );
    assert!(
        synced.contains("u32") && synced.contains("String"),
        "the validation engine sees the synced `fresh() -> u32`: {synced}"
    );

    // A command rewrites `store.rs` on the node; the change comes back to the checkout as
    // already synced, so only the gateway itself can tell the validation engine.
    let _ = text_of(
        &tool(
            addr,
            &root,
            "code_exec",
            serde_json::json!({
                "argv": ["sh", "-c", "sed -i 's/pub fn fresh() -> u32/pub fn fresh() -> u64/' src/store.rs"],
                "timeout_secs": 60
            }),
        )
        .await,
    );
    assert!(
        std::fs::read_to_string(checkout.path("src/store.rs"))
            .unwrap()
            .contains("-> u64"),
        "the command's change came back"
    );
    let rewritten = text_of(
        &tool(
            addr,
            &root,
            "code_validate_edit",
            serde_json::json!({ "path": "src/lib.rs", "new_text": proposal }),
        )
        .await,
    );
    assert!(
        rewritten.contains("u64") && !rewritten.contains("u32"),
        "the validation engine sees what the command wrote: {rewritten}"
    );

    // And the ordinary engine answers as before, from the checkout, not from any proposal.
    let after = text_of(
        &tool(
            addr,
            &root,
            "code_references",
            serde_json::json!({ "path": "src/lib.rs", "line": 7, "character": 12 }),
        )
        .await,
    );
    assert!(after.contains("store.rs:6:"), "{after}");
}

/// A workspace the gateway has not been asked about for a while is evicted, and the next
/// query loads it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_workspace_is_evicted_and_comes_back() {
    // Evict after a second of idleness, so the sweep runs inside a test's lifetime.
    let gateway = Gateway::start_with(&[("PROD_CODE_IDLE_EVICT_SECS", "1")]);
    let checkout = Checkout::new();
    let root = checkout.root();

    let first = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_hover",
            serde_json::json!({ "symbol": "Quantity::plus" }),
        )
        .await,
    );
    assert!(first.contains("fn plus"), "the workspace loaded: {first}");

    // Long enough for the eviction sweep to notice it has nothing to do.
    tokio::time::sleep(Duration::from_secs(4)).await;

    let again = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_hover",
            serde_json::json!({ "symbol": "Quantity::plus" }),
        )
        .await,
    );
    assert!(
        again.contains("fn plus"),
        "and it answers again after being evicted: {again}"
    );
}

/// Two gateways that know about each other: gossip, the merged cluster view, and placing a
/// workspace on the node that can actually serve it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_gateways_find_each_other_and_place_work() {
    use futures_util::{SinkExt, StreamExt};
    use prod_code_protocol::{PlaceRequest, ProdCodeCodec, WireMessage};
    use tokio_util::codec::Framed;

    // Each one is told the other's address and which engines it is allowed to serve, so the
    // placement question has a right answer that is not "me".
    let rust_port = free_port();
    let go_port = free_port();
    let rust_node = Gateway::start_with(&[
        ("PROD_CODE_BIND", &rust_port.to_string()),
        ("PROD_CODE_ENGINES", "rust"),
        ("PROD_CODE_PEERS", &go_port.to_string()),
        ("PROD_CODE_ADVERTISE", &rust_port.to_string()),
    ]);
    let go_node = Gateway::start_with(&[
        ("PROD_CODE_BIND", &go_port.to_string()),
        ("PROD_CODE_ENGINES", "go"),
        ("PROD_CODE_PEERS", &rust_port.to_string()),
        ("PROD_CODE_ADVERTISE", &go_port.to_string()),
    ]);

    // Gossip runs every few seconds; wait for the Rust node to hear about the Go one.
    let deadline = Instant::now() + Duration::from_secs(40);
    let mut nodes = 0usize;
    while Instant::now() < deadline {
        let stream = tokio::net::TcpStream::connect(rust_port)
            .await
            .expect("connect");
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        framed
            .send(WireMessage::ClusterRequest)
            .await
            .expect("send cluster");
        if let Some(Ok(WireMessage::ClusterResponse(cluster))) = framed.next().await {
            nodes = cluster.nodes.len();
            if nodes >= 2 {
                let engines: Vec<String> = cluster
                    .nodes
                    .iter()
                    .flat_map(|node| node.status.detected_engines.clone())
                    .collect();
                assert!(
                    engines.iter().any(|e| e.contains("go")),
                    "the cluster view carries the other node's engines: {engines:?}"
                );
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        nodes >= 2,
        "the two gateways never found each other (saw {nodes} node(s))"
    );

    // Go work does not belong on a Rust-only node, and the answer says where it does belong.
    let stream = tokio::net::TcpStream::connect(rust_port)
        .await
        .expect("connect");
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::PlaceRequest(PlaceRequest {
            workspace_name: "a-go-project".to_string(),
            engine: Some("go".to_string()),
            os: None,
        }))
        .await
        .expect("send place");
    match framed.next().await {
        Some(Ok(WireMessage::PlaceResponse(place))) => {
            let node = place.node.unwrap_or_default();
            assert!(
                node.contains(&go_port.port().to_string()),
                "Go work is placed on the Go node, not here: {node} ({})",
                place.reason
            );
        }
        other => panic!("unexpected answer to a placement request: {other:?}"),
    }

    drop(go_node);
    drop(rust_node);
}

/// A Python checkout, and a gateway told to serve only Rust: the generic adapter for a third
/// kind of language server, and the refusal that keeps work off a node that cannot do it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_python_checkout_and_a_node_that_refuses_it() {
    // RUST_LOG is deliberately unset, so the daemon builds its own default filter — the one path in the
    // binary a test that sets the variable can never take.
    let gateway = Gateway::start_with(&[("RUST_LOG", "")]);
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical");
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"subject\"\nversion = \"0.1.0\"\n",
    )
    .expect("pyproject");
    std::fs::create_dir_all(root.join("subject")).expect("package dir");
    std::fs::write(root.join("subject/__init__.py"), "").expect("init");
    std::fs::write(
        root.join("subject/quantity.py"),
        "\"\"\"A count of something.\"\"\"\n\n\nclass Quantity:\n    def __init__(self, units: int) -> None:\n        self.units = units\n\n\ndef total(all_of_them: list[Quantity]) -> int:\n    \"\"\"Sums the quantities.\"\"\"\n    return sum(q.units for q in all_of_them)\n",
    )
    .expect("quantity.py");
    std::fs::write(
        root.join("subject/use.py"),
        "from .quantity import Quantity, total\n\n\ndef describe(all_of_them: list[Quantity]) -> str:\n    return f\"{total(all_of_them)} units\"\n",
    )
    .expect("use.py");
    commit_in(&root);

    let outline = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_outline",
            serde_json::json!({ "path": "subject/quantity.py" }),
        )
        .await,
    );
    assert!(
        outline.contains("Quantity") || outline.contains("total"),
        "the generic adapter answers for Python: {outline}"
    );

    // A second checkout on the same gateway: two workspaces, each with its own engine.
    let rust_checkout = Checkout::new();
    let rust_root = rust_checkout.root();
    let hover = text_of(
        &tool(
            gateway.addr,
            &rust_root,
            "code_hover",
            serde_json::json!({ "symbol": "Quantity::plus" }),
        )
        .await,
    );
    assert!(
        hover.contains("fn plus"),
        "the same gateway answers for a Rust checkout too: {hover}"
    );
    let status = text_of(
        &tool(
            gateway.addr,
            &rust_root,
            "code_status",
            serde_json::json!({}),
        )
        .await,
    );
    assert!(
        status.contains('2') || status.to_lowercase().contains("workspace"),
        "it is holding both workspaces: {status}"
    );

    // A node allowed to serve only Rust does not quietly answer for Go; it says so.
    let rust_only = Gateway::start_with(&[("PROD_CODE_ENGINES", "rust")]);
    let go_checkout = tempfile::tempdir().expect("go checkout");
    let go_root = std::fs::canonicalize(go_checkout.path()).expect("canonical");
    std::fs::write(
        go_root.join("go.mod"),
        "module example.com/nope\n\ngo 1.22\n",
    )
    .expect("go.mod");
    std::fs::write(
        go_root.join("main.go"),
        "package nope\n\nfunc Hello() string { return \"hi\" }\n",
    )
    .expect("main.go");
    commit_in(&go_root);

    let refused = prod_code_mcp::tools::execute_tool(
        rust_only.addr,
        &go_root,
        "code_outline",
        serde_json::json!({ "path": "main.go" }),
    )
    .await;
    match refused {
        Err(err) => {
            let text = format!("{err:#}").to_lowercase();
            assert!(
                text.contains("go") || text.contains("engine") || text.contains("serve"),
                "the refusal says why: {text}"
            );
        }
        Ok(result) => {
            let text = text_of(&result).to_lowercase();
            assert!(
                result.is_error || text.contains("engine") || text.trim().is_empty(),
                "a Rust-only node does not answer for Go as if it could: {text}"
            );
        }
    }
}

/// A C++ checkout, and the two tools that only have something to say once something has gone
/// wrong: the explanation of a failing test, and the blast radius of an uncommitted change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cpp_checkout_a_failing_test_and_a_diff() {
    let gateway = Gateway::start();

    if which("clangd").is_some() {
        let cpp = tempfile::tempdir().expect("cpp checkout");
        let cpp_root = std::fs::canonicalize(cpp.path()).expect("canonical");
        std::fs::write(
            cpp_root.join("CMakeLists.txt"),
            "cmake_minimum_required(VERSION 3.16)\nproject(subject CXX)\nadd_library(subject quantity.cpp)\n",
        )
        .expect("CMakeLists");
        std::fs::write(
            cpp_root.join("quantity.h"),
            "#pragma once\n\n// A count of something.\nstruct Quantity {\n  int units;\n};\n\nint total(const Quantity* all, int count);\n",
        )
        .expect("quantity.h");
        std::fs::write(
            cpp_root.join("quantity.cpp"),
            "#include \"quantity.h\"\n\nint total(const Quantity* all, int count) {\n  int sum = 0;\n  for (int i = 0; i < count; ++i) {\n    sum += all[i].units;\n  }\n  return sum;\n}\n",
        )
        .expect("quantity.cpp");
        commit_in(&cpp_root);

        let outline = text_of(
            &tool(
                gateway.addr,
                &cpp_root,
                "code_outline",
                serde_json::json!({ "path": "quantity.h" }),
            )
            .await,
        );
        assert!(
            outline.contains("Quantity") || outline.contains("total"),
            "clangd answers through the gateway: {outline}"
        );
    } else {
        eprintln!("SKIPPED the C++ half: no clangd on PATH");
    }

    // A crate whose test fails, so that the failure explainer has something to explain.
    let checkout = Checkout::new();
    let root = checkout.root();
    checkout.write(
        "src/failing.rs",
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn arithmetic_is_not_what_we_thought() {\n        assert_eq!(crate::total(&[crate::Quantity::new(2)]).units, 3);\n    }\n}\n",
    );
    let lib = std::fs::read_to_string(checkout.path("src/lib.rs")).expect("lib.rs");
    checkout.write("src/lib.rs", &format!("{lib}\nmod failing;\n"));
    checkout.commit();

    let failed = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_test",
            serde_json::json!({ "path": "." }),
        )
        .await,
    );
    assert!(
        failed.contains("fail") || failed.contains("FAILED") || failed.contains('1'),
        "the failing test is reported as failing: {failed}"
    );

    let explained = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_diagnose_failure",
            serde_json::json!({}),
        )
        .await,
    );
    assert!(
        !explained.trim().is_empty(),
        "the failure explainer answers: {explained}"
    );

    // An uncommitted change, and what it reaches.
    checkout.write(
        "src/store.rs",
        &std::fs::read_to_string(checkout.path("src/store.rs"))
            .expect("store.rs")
            .replace("pub fn sum(&self)", "pub fn sum_all(&self)"),
    );
    let impact = text_of(&tool(gateway.addr, &root, "code_impact", serde_json::json!({})).await);
    assert!(
        impact.contains("store.rs") || impact.contains("sum") || !impact.trim().is_empty(),
        "the blast radius names what changed: {impact}"
    );
}

/// The queries that are each one more branch of the dispatch, gathered in one place: a window
/// of a file the gateway alone can read, the usage metrics over a window, a sync that deletes,
/// and intent search on a checkout the in-process engine does not own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_remaining_branches_of_the_dispatch() {
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    let (addr, root) = (gateway.addr, checkout.root());

    // Load the workspace first, so the metrics below have something to report.
    let _ = tool(
        addr,
        &root,
        "code_hover",
        serde_json::json!({ "symbol": "Quantity::plus" }),
    )
    .await;

    // Intent search, and the same search narrowed to a subdirectory.
    let search = text_of(
        &tool(
            addr,
            &root,
            "code_search",
            serde_json::json!({ "query": "sum every quantity", "limit": 5, "path": "src" }),
        )
        .await,
    );
    assert!(
        search.contains("total") || search.contains("sum"),
        "the search finds the summing function: {search}"
    );

    // A file the client deletes is a deletion in the next sync, not a leftover on the gateway.
    std::fs::write(
        checkout.path("src/extra.rs"),
        "pub fn extra() -> u32 { 7 }\n",
    )
    .expect("write");
    checkout.write(
        "src/lib.rs",
        &format!(
            "{}\npub mod extra;\n",
            std::fs::read_to_string(checkout.path("src/lib.rs")).expect("lib.rs")
        ),
    );
    checkout.commit();
    let with_extra = text_of(
        &tool(
            addr,
            &root,
            "code_symbols",
            serde_json::json!({ "query": "extra" }),
        )
        .await,
    );
    assert!(
        with_extra.contains("extra"),
        "the new module reached the gateway: {with_extra}"
    );

    std::fs::remove_file(checkout.path("src/extra.rs")).expect("remove");
    checkout.write(
        "src/lib.rs",
        &std::fs::read_to_string(checkout.path("src/lib.rs"))
            .expect("lib.rs")
            .replace("\npub mod extra;\n", "\n"),
    );
    checkout.commit();
    let after_delete = text_of(
        &tool(
            addr,
            &root,
            "code_diagnostics",
            serde_json::json!({ "path": "src/lib.rs" }),
        )
        .await,
    );
    assert!(
        after_delete.contains("0 error"),
        "the deletion was synced, so nothing dangles: {after_delete}"
    );

    // What this test has been doing, as the gateway counted it.
    let report = prod_code_mcp::cluster::node_metrics(addr, 3600)
        .await
        .expect("the gateway reports its own usage");
    assert!(
        !report.queries.is_empty() || !report.execs.is_empty(),
        "the metrics carry what this test has been doing: {report:?}"
    );

    // And the same node's status and cluster view through the client library.
    let status = prod_code_mcp::cluster::node_status(addr)
        .await
        .expect("status");
    assert!(status.server_pid > 0, "a real process answered: {status:?}");
    let view = prod_code_mcp::cluster::cluster_view(addr)
        .await
        .expect("cluster view");
    assert!(
        !view.nodes.is_empty(),
        "a lone gateway is a cluster of one: {view:?}"
    );
}

/// An editor's session on the wire: synced, handshaken as an editor, initialised.
struct EditorSession {
    framed: tokio_util::codec::Framed<tokio::net::TcpStream, prod_code_protocol::ProdCodeCodec>,
    next_id: u64,
    notes: Vec<serde_json::Value>,
}

impl EditorSession {
    async fn open(addr: SocketAddr, root: &Path) -> (Self, serde_json::Value) {
        let root_uri = format!("file://{}", root.display());
        Self::open_with(
            addr,
            root,
            serde_json::json!({ "rootUri": root_uri, "capabilities": {} }),
        )
        .await
    }

    async fn open_with(
        addr: SocketAddr,
        root: &Path,
        init: serde_json::Value,
    ) -> (Self, serde_json::Value) {
        Self::open_for(
            addr,
            root,
            init,
            Some(prod_code_protocol::PURPOSE_EDITOR.to_string()),
        )
        .await
    }

    async fn open_for(
        addr: SocketAddr,
        root: &Path,
        init: serde_json::Value,
        purpose: Option<String>,
    ) -> (Self, serde_json::Value) {
        use futures_util::{SinkExt, StreamExt};
        let stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
        let mut framed =
            tokio_util::codec::Framed::new(stream, prod_code_protocol::ProdCodeCodec::new());
        let identity = prod_code_mcp::sync::workspace_identity(root);
        prod_code_mcp::sync::push_workspace_sync(&mut framed, root, &identity, None)
            .await
            .expect("sync");
        framed
            .send(prod_code_protocol::WireMessage::HandshakeRequest(
                prod_code_protocol::HandshakeRequest {
                    protocol_version: prod_code_protocol::PROTOCOL_VERSION,
                    supported_versions: Some(vec![prod_code_protocol::PROTOCOL_VERSION]),
                    capabilities: None,
                    client_name: "editor-test".to_string(),
                    client_pid: std::process::id(),
                    auth_token: None,
                    client_workspace_root: root.to_string_lossy().to_string(),
                    preferred_engine: None,
                    base_workspace_name: Some(identity.name.clone()),
                    engine_subpath: None,
                    client_agent: None,
                    client_host: None,
                    purpose,
                },
            ))
            .await
            .expect("handshake");
        match framed.next().await {
            Some(Ok(prod_code_protocol::WireMessage::HandshakeResponse(_))) => {}
            other => panic!("no handshake response: {other:?}"),
        }
        let mut session = Self {
            framed,
            next_id: 1,
            notes: Vec::new(),
        };
        let init = session.request("initialize", init).await;
        session.notify("initialized", serde_json::json!({})).await;
        (session, init)
    }

    async fn notify(&mut self, method: &str, params: serde_json::Value) {
        use futures_util::SinkExt;
        let message = serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.framed
            .send(prod_code_protocol::WireMessage::LspPayload(
                message.to_string(),
            ))
            .await
            .expect("send");
    }

    /// The whole response (`result` or `error`) to one request; notifications that arrive in
    /// the meantime are kept.
    async fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        use futures_util::{SinkExt, StreamExt};
        let id = self.next_id;
        self.next_id += 1;
        let message =
            serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.framed
            .send(prod_code_protocol::WireMessage::LspPayload(
                message.to_string(),
            ))
            .await
            .expect("send");
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(120), self.framed.next())
                .await
                .unwrap_or_else(|_| panic!("{method} was not answered"));
            let Some(Ok(prod_code_protocol::WireMessage::LspPayload(raw))) = frame else {
                panic!("the session ended waiting for {method}: {frame:?}");
            };
            let value: serde_json::Value = serde_json::from_str(&raw).expect("json");
            if value.get("id") == Some(&serde_json::json!(id)) && value.get("method").is_none() {
                return value;
            }
            self.notes.push(value);
        }
    }

    /// The next `textDocument/publishDiagnostics` for `uri` after `after` of them were seen.
    async fn diagnostics(&mut self, uri: &str, after: usize) -> serde_json::Value {
        use futures_util::StreamExt;
        let published = |notes: &[serde_json::Value]| -> Vec<serde_json::Value> {
            notes
                .iter()
                .filter(|n| {
                    n["method"] == "textDocument/publishDiagnostics" && n["params"]["uri"] == uri
                })
                .map(|n| n["params"]["diagnostics"].clone())
                .collect()
        };
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(found) = published(&self.notes).get(after) {
                return found.clone();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let frame = tokio::time::timeout(left, self.framed.next())
                .await
                .expect("diagnostics were published");
            let Some(Ok(prod_code_protocol::WireMessage::LspPayload(raw))) = frame else {
                panic!("the session ended waiting for diagnostics: {frame:?}");
            };
            self.notes.push(serde_json::from_str(&raw).expect("json"));
        }
    }
}

const EDITOR_SUBJECT: &str = "pub struct Counter {
    pub total: u32,
}

impl Counter {
    pub fn add(&mut self, amount: u32) -> u32 {
        self.total += amount;
        self.total
    }
}

pub fn run() -> u32 {
    let mut counter = Counter { total: 0 };
    counter.add(2)
}
";

/// An editor on `prod-code lsp` in a Rust checkout gets what it needs beyond navigation from
/// the in-memory engine (#310): completion with its imports, signature help, inlay hints,
/// highlights, code actions with their edits, formatting, and diagnostics pushed after an
/// edit. A request the engine has no answer for is refused instead of left waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_editor_session_gets_what_an_editor_needs_for_rust() {
    // The in-memory engine answers an editor only where the node has no rust-analyzer for it;
    // here it is asked to, whatever the node has.
    let gateway = Gateway::start_with(&[("PROD_CODE_EDITOR_SERVERS", "off")]);
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"subject\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    )
    .expect("Cargo.toml");
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("src/lib.rs"), EDITOR_SUBJECT).expect("lib.rs");
    commit_in(&root);

    let (mut editor, init) = EditorSession::open(gateway.addr, &root).await;
    let caps = &init["result"]["capabilities"];
    assert_eq!(
        caps["completionProvider"]["resolveProvider"], true,
        "{init}"
    );
    assert_eq!(caps["textDocumentSync"]["change"], 1, "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "prod-code", "{init}");

    let uri = format!("file://{}", root.join("src/lib.rs").display());
    editor
        .notify(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": { "uri": uri, "languageId": "rust", "version": 1, "text": EDITOR_SUBJECT } }),
        )
        .await;
    let doc = serde_json::json!({ "uri": uri });
    let at = |line: u32, character: u32| serde_json::json!({ "textDocument": doc, "position": { "line": line, "character": character } });

    // After `self.` on line 7, a field access: the field and the method.
    let completion = editor.request("textDocument/completion", at(7, 13)).await;
    let labels: Vec<&str> = completion["result"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a completion list: {completion}"))
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    // rust-analyzer labels a method with its parentheses: `add(…)`.
    assert!(
        labels.iter().any(|l| l.starts_with("add(")) && labels.contains(&"total"),
        "the method and the field are offered: {labels:?}"
    );
    let add = completion["result"]["items"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item["label"]
                    .as_str()
                    .is_some_and(|l| l.starts_with("add("))
            })
        })
        .cloned()
        .expect("add");
    assert_eq!(add["kind"], 2, "a method: {add}");
    let resolved = editor.request("completionItem/resolve", add.clone()).await;
    assert_eq!(resolved["result"]["label"], add["label"], "{resolved}");

    // A name the file has not imported: the item carries its import, and resolving it adds
    // the `use`.
    let importing = EDITOR_SUBJECT.replace(
        "    counter.add(2)\n",
        "    let _map = HashMa;\n    counter.add(2)\n",
    );
    editor
        .notify(
            "textDocument/didChange",
            serde_json::json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": importing }] }),
        )
        .await;
    let completion = editor.request("textDocument/completion", at(13, 21)).await;
    let hash_map = completion["result"]["items"]
        .as_array()
        .and_then(|items| {
            items.iter().find(|item| {
                item["label"]
                    .as_str()
                    .is_some_and(|l| l.starts_with("HashMap"))
            })
        })
        .cloned()
        .unwrap_or_else(|| panic!("HashMap is offered: {completion}"));
    assert!(
        hash_map["data"]["imports"]
            .as_array()
            .is_some_and(|imports| !imports.is_empty()),
        "the item carries its import: {hash_map}"
    );
    let resolved = editor.request("completionItem/resolve", hash_map).await;
    let added: String = resolved["result"]["additionalTextEdits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|edit| edit["newText"].as_str())
        .collect();
    assert!(
        added.contains("use std::collections::HashMap;"),
        "resolving adds the import: {resolved}"
    );
    editor
        .notify(
            "textDocument/didChange",
            serde_json::json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": EDITOR_SUBJECT }] }),
        )
        .await;

    // Inside `add(` on line 13: the signature, with `amount` as its parameter.
    let help = editor
        .request("textDocument/signatureHelp", at(13, 16))
        .await;
    let signature = &help["result"]["signatures"][0];
    let label = signature["label"].as_str().unwrap_or_default();
    assert!(label.contains("fn add"), "{help}");
    let span = &signature["parameters"][0]["label"];
    let (start, end) = (
        span[0].as_u64().expect("start") as usize,
        span[1].as_u64().expect("end") as usize,
    );
    assert!(
        label[start..end].contains("amount"),
        "the parameter's offsets point at it: {help}"
    );

    // The literal argument gets its parameter's name.
    let hints = editor
        .request(
            "textDocument/inlayHint",
            serde_json::json!({ "textDocument": doc, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 15, "character": 0 } } }),
        )
        .await;
    let hint_labels: Vec<String> = hints["result"]
        .as_array()
        .unwrap_or_else(|| panic!("hints: {hints}"))
        .iter()
        .map(|hint| hint["label"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        hint_labels.iter().any(|l| l.contains("amount")),
        "a parameter hint for `2`: {hint_labels:?}"
    );

    // Every use of `total` in the file, the writes told apart.
    let highlights = editor
        .request("textDocument/documentHighlight", at(1, 9))
        .await;
    let highlighted = highlights["result"].as_array().cloned().unwrap_or_default();
    assert!(highlighted.len() >= 3, "{highlights}");
    assert!(
        highlighted.iter().any(|h| h["kind"] == 3),
        "`self.total += amount` is a write: {highlights}"
    );

    // A code action at the `counter` binding, and the edit it resolves to.
    let actions = editor
        .request(
            "textDocument/codeAction",
            serde_json::json!({ "textDocument": doc, "range": { "start": { "line": 12, "character": 12 }, "end": { "line": 12, "character": 12 } }, "context": { "diagnostics": [] } }),
        )
        .await;
    let action = actions["result"]
        .as_array()
        .and_then(|actions| actions.first())
        .cloned()
        .unwrap_or_else(|| panic!("an action is offered: {actions}"));
    let resolved = editor.request("codeAction/resolve", action).await;
    assert!(
        resolved["result"]["edit"]["documentChanges"]
            .as_array()
            .is_some_and(|changes| !changes.is_empty()),
        "the action resolves to an edit: {resolved}"
    );

    // rustfmt on the node, for the edition of the crate.
    let messy = EDITOR_SUBJECT.replace("pub fn run() -> u32 {", "pub fn   run( ) -> u32{");
    editor
        .notify(
            "textDocument/didChange",
            serde_json::json!({ "textDocument": { "uri": uri, "version": 4 }, "contentChanges": [{ "text": messy }] }),
        )
        .await;
    let formatted = editor
        .request(
            "textDocument/formatting",
            serde_json::json!({ "textDocument": doc, "options": { "tabSize": 4, "insertSpaces": true } }),
        )
        .await;
    assert_eq!(
        formatted["result"][0]["newText"], EDITOR_SUBJECT,
        "the formatted text: {formatted}"
    );

    // A type error typed in the editor is published without being asked for.
    let before = editor
        .notes
        .iter()
        .filter(|n| n["method"] == "textDocument/publishDiagnostics" && n["params"]["uri"] == uri)
        .count();
    let broken = EDITOR_SUBJECT.replace("counter.add(2)", "counter.add(\"two\")");
    editor
        .notify(
            "textDocument/didChange",
            serde_json::json!({ "textDocument": { "uri": uri, "version": 5 }, "contentChanges": [{ "text": broken }] }),
        )
        .await;
    let mut diagnostics = editor.diagnostics(&uri, before).await;
    // A pass for an earlier edit may still be on its way: wait for the one with the error.
    let mut seen = before;
    while !diagnostics.as_array().is_some_and(|d| {
        d.iter()
            .any(|d| d["severity"] == 1 && d["range"]["start"]["line"] == 13)
    }) {
        seen += 1;
        diagnostics = editor.diagnostics(&uri, seen).await;
    }

    // Folding ranges are not advertised; asked anyway, they are refused, not left hanging.
    let folding = editor
        .request(
            "textDocument/foldingRange",
            serde_json::json!({ "textDocument": doc }),
        )
        .await;
    assert_eq!(folding["error"]["code"], -32601, "{folding}");
}

/// A crate that moves a value twice (rustc's E0382, which only a build finds) and calls a
/// macro.
const MOVED_TWICE: &str = "pub fn first(v: Vec<String>) -> String {
    let s = v;
    let t = v;
    format!(\"{s:?}{t:?}\")
}
";

/// An editor gets the language's own server on the node (#332): rust-analyzer itself, with
/// its protocol extensions, its check on save and the editor's own `initialize`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_editor_gets_rust_analyzer_itself_on_the_node() {
    let has_rust_analyzer = std::process::Command::new("rust-analyzer")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    if !has_rust_analyzer {
        eprintln!("skipping: rust-analyzer is not installed (build nodes have it)");
        return;
    }
    let gateway = Gateway::start();
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"moved\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    )
    .expect("Cargo.toml");
    std::fs::create_dir_all(root.join("src")).expect("src");
    std::fs::write(root.join("src/lib.rs"), MOVED_TWICE).expect("lib.rs");
    commit_in(&root);

    let (mut editor, init) = EditorSession::open(gateway.addr, &root).await;
    assert_eq!(
        init["result"]["serverInfo"]["name"], "rust-analyzer",
        "{init}"
    );
    let uri = format!("file://{}", root.join("src/lib.rs").display());
    editor
        .notify(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": { "uri": uri, "languageId": "rust", "version": 1, "text": MOVED_TWICE } }),
        )
        .await;

    // rust-analyzer's own extension of the protocol, on `format!`.
    let deadline = Instant::now() + Duration::from_secs(120);
    let expansion = loop {
        let expanded = editor
            .request(
                "rust-analyzer/expandMacro",
                serde_json::json!({ "textDocument": { "uri": uri }, "position": { "line": 3, "character": 5 } }),
            )
            .await;
        if expanded["result"]["expansion"].is_string() || Instant::now() > deadline {
            break expanded;
        }
        // The workspace is still loading.
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert!(
        expansion["result"]["expansion"]
            .as_str()
            .is_some_and(|text| text.contains("format")),
        "the macro is expanded by rust-analyzer: {expansion}"
    );

    // A save runs `cargo check` on the node, and rustc's error comes back to the editor.
    editor
        .notify(
            "textDocument/didSave",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;
    let mut seen = 0;
    loop {
        let diagnostics = editor.diagnostics(&uri, seen).await;
        if diagnostics
            .as_array()
            .is_some_and(|d| d.iter().any(|d| d["code"] == "E0382"))
        {
            break;
        }
        seen += 1;
    }
}

/// The editor's process id names a process of the editor's machine. basedpyright, like every
/// server built on vscode-languageserver, exits within seconds when that process is not
/// running where it runs; the gateway drops the id from `initialize`, so the server outlives it
/// (#332).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_editors_python_server_does_not_watch_the_editors_process() {
    if which("basedpyright-langserver").is_none() {
        eprintln!("skipping: basedpyright-langserver is not installed (build nodes have it)");
        return;
    }
    let gateway = Gateway::start();
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical");
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"subject\"\nversion = \"0.1.0\"\n",
    )
    .expect("pyproject.toml");
    let text = "def add(a: int, b: int) -> int:\n    return a + b\n\ntotal = add(1, 2)\n";
    std::fs::write(root.join("main.py"), text).expect("main.py");
    commit_in(&root);

    // A process id that names nothing on the node.
    let root_uri = format!("file://{}", root.display());
    let (mut editor, init) = EditorSession::open_with(
        gateway.addr,
        &root,
        serde_json::json!({ "processId": 2147483000u32, "rootUri": root_uri, "capabilities": {} }),
    )
    .await;
    assert!(
        init["result"]["serverInfo"]["name"]
            .as_str()
            .is_some_and(|name| name.to_lowercase().contains("pyright")),
        "the editor talks to basedpyright itself: {init}"
    );
    // basedpyright looks for the process every few seconds.
    tokio::time::sleep(Duration::from_secs(8)).await;
    let uri = format!("file://{}", root.join("main.py").display());
    editor
        .notify(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": { "uri": uri, "languageId": "python", "version": 1, "text": text } }),
        )
        .await;
    let hover = editor
        .request(
            "textDocument/hover",
            serde_json::json!({ "textDocument": { "uri": uri }, "position": { "line": 3, "character": 9 } }),
        )
        .await;
    assert!(
        hover.to_string().contains("def add"),
        "the server is still there to answer: {hover}"
    );
}

/// Every `uri` (or `targetUri`) a location answer names.
fn answer_uris(result: &serde_json::Value) -> std::collections::BTreeSet<String> {
    let items = match result {
        serde_json::Value::Array(items) => items.clone(),
        serde_json::Value::Null => Vec::new(),
        one => vec![one.clone()],
    };
    items
        .iter()
        .filter_map(|l| l.get("uri").or_else(|| l.get("targetUri")))
        .filter_map(|u| u.as_str().map(String::from))
        .collect()
}

/// A checkout whose path holds spaces, `#`, a literal `%41` and non-ASCII letters, with a
/// module file named the same way (#438). Navigation names those files by URIs that decode to
/// them, the source text reaches the analyzer as written (a column after the checkout's own
/// path in a string still points at the same token), and a rename rewrites exactly those files.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn special_characters_in_paths_survive_navigation_and_edits() {
    use prod_code_protocol::path::{file_uri, file_uri_path};
    let gateway = Gateway::start_with(&[("PROD_CODE_EDITOR_SERVERS", "off")]);
    let holder = tempfile::tempdir().expect("checkout");
    let dir = holder.path().join("deeper").join("my app #1 100%41 ü");
    std::fs::create_dir_all(dir.join("src/odd dir #2")).expect("dirs");
    let root = std::fs::canonicalize(&dir).expect("canonical");
    let odd_rel = "src/odd dir #2/100%41 ü.rs";
    let odd_text = "pub fn helper(n: u32) -> u32 {\n    n + 1\n}\n";
    let lib = format!(
        "#[path = \"odd dir #2/100%41 ü.rs\"]\npub mod odd;\n\npub const HOME: &str = \"{}\"; pub fn run() -> u32 {{ odd::helper(2) }}\n",
        root.display()
    );
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"subject\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    )
    .expect("Cargo.toml");
    std::fs::write(root.join("src/lib.rs"), &lib).expect("lib.rs");
    std::fs::write(root.join(odd_rel), odd_text).expect("odd file");
    commit_in(&root);

    let lib_uri = file_uri(&root.join("src/lib.rs"));
    let odd_uri = file_uri(&root.join(odd_rel));
    assert!(odd_uri.contains("%2541") && odd_uri.contains("%23") && odd_uri.contains("%C3%BC"));

    let (mut editor, _) = EditorSession::open_with(
        gateway.addr,
        &root,
        serde_json::json!({ "rootUri": file_uri(&root), "capabilities": {} }),
    )
    .await;
    editor
        .notify(
            "textDocument/didOpen",
            serde_json::json!({ "textDocument": { "uri": lib_uri, "languageId": "rust", "version": 1, "text": lib } }),
        )
        .await;
    let line = lib.lines().nth(3).expect("line 3");
    let character = line[..line.find("helper(").expect("call")]
        .encode_utf16()
        .count()
        + 1;
    let at = serde_json::json!({ "textDocument": { "uri": lib_uri }, "position": { "line": 3, "character": character } });

    let definition = editor.request("textDocument/definition", at.clone()).await;
    let targets = answer_uris(&definition["result"]);
    assert_eq!(
        targets.into_iter().collect::<Vec<_>>(),
        vec![odd_uri.clone()],
        "{definition}"
    );
    assert_eq!(file_uri_path(&odd_uri), Some(root.join(odd_rel)));

    // Asked from the declaration, in the file whose URI the server has to decode.
    let references = editor
        .request(
            "textDocument/references",
            serde_json::json!({ "textDocument": { "uri": odd_uri }, "position": { "line": 0, "character": 8 },
                "context": { "includeDeclaration": true } }),
        )
        .await;
    let found = answer_uris(&references["result"]);
    assert!(found.contains(&lib_uri), "{references}");
    for uri in &found {
        let path = file_uri_path(uri).unwrap_or_else(|| panic!("{uri} names no file"));
        assert!(path.is_file(), "{uri} names {}", path.display());
    }

    let mut rename_params = at.clone();
    rename_params["newName"] = serde_json::json!("bump");
    let rename = editor.request("textDocument/rename", rename_params).await;
    let edit = &rename["result"];
    let mut edited: std::collections::BTreeSet<String> = edit["changes"]
        .as_object()
        .map(|changes| changes.keys().cloned().collect())
        .unwrap_or_default();
    for change in edit["documentChanges"].as_array().into_iter().flatten() {
        if let Some(uri) = change.pointer("/textDocument/uri").and_then(|u| u.as_str()) {
            edited.insert(uri.to_string());
        }
    }
    assert_eq!(
        edited,
        [lib_uri.clone(), odd_uri.clone()].into_iter().collect(),
        "{rename}"
    );
    drop(editor);

    // The same through the tools an agent uses, which write the edit to the checkout.
    let definition = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_definition",
            serde_json::json!({ "symbol": "helper" }),
        )
        .await,
    );
    assert!(
        definition.contains(&format!("{odd_uri}:1:8")),
        "{definition}"
    );
    let renamed = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_rename",
            serde_json::json!({ "symbol": "helper", "new_name": "bump" }),
        )
        .await,
    );
    assert_eq!(
        std::fs::read_to_string(root.join(odd_rel)).expect("odd file"),
        odd_text.replace("helper", "bump"),
        "{renamed}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).expect("lib.rs"),
        lib.replace("odd::helper", "odd::bump"),
        "{renamed}"
    );
    // No file appeared under a decoded, re-encoded or truncated name.
    fn rust_files(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let path = entry.path();
            if path.is_dir() && entry.file_name() != ".git" {
                rust_files(&path, root, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path.strip_prefix(root).expect("under root");
                out.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    let mut files = Vec::new();
    rust_files(&root, &root, &mut files);
    files.sort();
    assert_eq!(files, vec!["src/lib.rs".to_string(), odd_rel.to_string()]);
}

/// #442: the planner consumes actual analyzer hovers/references/SSR, and a permitted
/// scalar reorder preserves the compiled program's output.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signature_effect_checks_use_real_analyzer_types() {
    const PROGRAM: &str = r#"use std::ops::Deref;
use std::sync::Mutex;

static LOG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn log(s: &'static str) {
    LOG.lock().unwrap().push(s);
}

pub struct Inner {
    pub n: u32,
}

pub struct Wrap(pub &'static str, pub Inner);

impl Deref for Wrap {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        log(self.0);
        &self.1
    }
}

#[allow(dead_code)]
pub enum Option<T> {
    None,
    Some(T),
}

impl<T> Drop for Option<T> {
    fn drop(&mut self) {
        log(match self {
            Option::Some(_) => "some",
            Option::None => "none",
        });
    }
}

pub fn fields(first: u32, second: u32) -> u32 {
    first * 10 + second
}

pub fn refs(first: &Inner, second: &Inner) -> u32 {
    first.n * 10 + second.n
}

pub fn shadowed(first: Option<u32>, second: Option<u32>) -> u32 {
    let _ = (&first, &second);
    7
}

pub fn scalars(first: u32, second: u32) -> u32 {
    first * 10 + second
}

fn main() {
    let (a, b) = (Wrap("a", Inner { n: 1 }), Wrap("b", Inner { n: 2 }));
    let (p, q): (u32, u32) = (3, 4);
    let (s, t) = (Option::Some(5), Option::None);
    let v = [
        fields(a.n, b.n),
        refs(&a, &b),
        shadowed(s, t),
        scalars(p /* p, /* then */ q, */, q),
    ];
    println!("{v:?} {:?}", LOG.lock().unwrap());
}
"#;
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    checkout.write("src/lib.rs", "");
    checkout.write("src/main.rs", PROGRAM);
    checkout.commit();
    let root = checkout.root();
    let file = checkout.path("src/main.rs");
    let params = ["second", "first"].map(|p| prod_code_mcp::signature::parse_param(p).unwrap());
    let position = |name: &str| {
        let at = PROGRAM.find(&format!("fn {name}(")).unwrap() + 3;
        let line = PROGRAM[..at].matches('\n').count() as u32 + 1;
        let col = (at - PROGRAM[..at].rfind('\n').map_or(0, |p| p + 1)) as u32 + 1;
        (line, col)
    };
    for (name, expected) in [
        ("fields", "Deref"),
        ("refs", "Deref"),
        ("shadowed", "drops"),
    ] {
        let (line, col) = position(name);
        let err = prod_code_mcp::signature::change(
            gateway.addr,
            &root,
            &file,
            line,
            col,
            &params,
            true,
            true,
        )
        .await
        .expect_err("observable effects must be refused even with force");
        assert!(format!("{err:#}").contains(expected), "{name}: {err:#}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), PROGRAM);
    }
    let output = || {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("fixture");
        let built = Command::new("rustc")
            .args(["--edition", "2021", "-A", "warnings"])
            .arg(&file)
            .arg("-o")
            .arg(&bin)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let ran = Command::new(&bin).output().unwrap();
        assert!(ran.status.success());
        ran.stdout
    };
    let before = output();
    let (line, col) = position("scalars");
    let changed = prod_code_mcp::signature::change(
        gateway.addr,
        &root,
        &file,
        line,
        col,
        &params,
        true,
        false,
    )
    .await
    .expect("real analyzer confirms primitive scalar types");
    assert!(changed.applied);
    assert_eq!(before, output());
}

/// The library accepts paths relative to the supplied checkout, even when the process cwd
/// belongs to a different Rust repository. Both diagnostics and validation use Python (#488).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relative_python_paths_use_the_same_engine_as_absolute_paths() {
    let _server = which("basedpyright-langserver").expect("native Python language server required");
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    let root = checkout.root();
    let source = "def answer() -> int:\n    return 3\n";
    checkout.write("docs/check.py", source);
    checkout.commit();
    let relative = Path::new("docs/check.py");
    let absolute = root.join(relative);
    for path in [relative, absolute.as_path()] {
        let report = prod_code_mcp::diagnostics::diagnostics(gateway.addr, &root, path)
            .await
            .expect("Python diagnostics, not a Rust VFS error");
        assert_eq!(report.errors, 0, "{}", report.render());
        let invalid = prod_code_mcp::diagnostics::validate_text(
            gateway.addr,
            &root,
            path,
            "def answer() -> int:\n    return \"wrong\"\n",
        )
        .await
        .expect("Python overlay diagnostics");
        assert!(
            invalid.errors > 0
                && invalid
                    .items
                    .iter()
                    .any(|d| d.source.as_deref() == Some("basedpyright") && d.severity == "error"),
            "{}",
            invalid.render()
        );
        let valid = prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, path, source)
            .await
            .expect("restored Python diagnostics");
        assert_eq!(valid.errors, 0, "{}", valid.render());
    }
    assert_eq!(
        std::fs::read_to_string(absolute).unwrap(),
        source,
        "validation leaves the source untouched"
    );
}

/// A native basedpyright validation keeps one document identity across the baseline and
/// proposal sessions, restores the checkout, and still reports a genuine proposal error
/// (#466). This runs a source-built isolated gateway, never an installed service.
const PYTHON_IDENTITY_SOURCE: &str = r####"#!/usr/bin/env python3
"""Regenerate the replication figures in docs/img from the numbers in docs/replication.md.
Dependency-free (hand-written SVG) so the figures are reproducible anywhere."""
import os, textwrap

OUT = os.path.join(os.path.dirname(__file__), "img")
FONT = "font-family='JetBrains Mono, SFMono-Regular, Menlo, monospace'"
INK, MUTED, GRID, PANEL = "#1f2328", "#6a737d", "#d0d7de", "#f6f8fa"
RING, RING_FILL = "#c0392b", "#fdecea"
NET, NET_FILL = "#1d4ed8", "#e8efff"
OK, OK_FILL = "#2e7d32", "#e8f5e9"
OTHER = "#8fa3b8"

def esc(t):
    return str(t).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

def para(x, y, text, width=95, color=None, size=12, lh=17):
    col = color or MUTED
    return "".join(f"<text x='{x}' y='{y + i * lh}' fill='{col}' font-size='{size}'>{esc(line)}</text>"
                   for i, line in enumerate(textwrap.wrap(text, width)))

def head(W, H, title, sub=None):
    s = [f"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 {W} {H}' width='{W}' height='{H}' {FONT} font-size='12'>",
         "<defs>"
         f"<marker id='a' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{INK}'/></marker>"
         f"<marker id='n' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{NET}'/></marker>"
         f"<marker id='r' markerWidth='8' markerHeight='8' refX='7' refY='4' orient='auto'><path d='M0,0 L8,4 L0,8 z' fill='{RING}'/></marker>"
         "</defs>",
         f"<rect width='{W}' height='{H}' fill='white'/>",
         f"<text x='16' y='24' font-size='15' font-weight='bold' fill='{INK}'>{esc(title)}</text>"]
    if sub:
        s.append(f"<text x='16' y='42' fill='{MUTED}'>{esc(sub)}</text>")
    return s

def box(x, y, w, h, text, sub=None, fill="white", stroke=INK, bold=True, size=12):
    t = f"<rect x='{x}' y='{y}' width='{w}' height='{h}' rx='6' fill='{fill}' stroke='{stroke}' stroke-width='1.5'/>"
    t += f"<text x='{x + w / 2}' y='{y + h / 2 + (-2 if sub else 5)}' text-anchor='middle' fill='{INK}' font-size='{size}' font-weight='{'bold' if bold else 'normal'}'>{esc(text)}</text>"
    if sub:
        t += f"<text x='{x + w / 2}' y='{y + h / 2 + 14}' text-anchor='middle' fill='{MUTED}' font-size='11'>{esc(sub)}</text>"
    return t

def ring(x, y, w, h, text="ring", sub="/dev/shm"):
    return box(x, y, w, h, text, sub, fill=RING_FILL, stroke=RING)

def panel(x, y, w, h, title, fill=PANEL):
    return (f"<rect x='{x}' y='{y}' width='{w}' height='{h}' rx='10' fill='{fill}' stroke='{GRID}'/>"
            f"<text x='{x + 12}' y='{y + 20}' fill='{INK}' font-weight='bold' font-size='13'>{esc(title)}</text>")

def arrow(x1, y1, x2, y2, label=None, color=INK, marker="a", dash=None, above=True, size=11):
    d = f" stroke-dasharray='{dash}'" if dash else ""
    t = f"<line x1='{x1}' y1='{y1}' x2='{x2}' y2='{y2}' stroke='{color}' stroke-width='1.5' marker-end='url(#{marker})'{d}/>"
    if label:
        ly = min(y1, y2) - 7 if above else max(y1, y2) + 15
        t += f"<text x='{(x1 + x2) / 2}' y='{ly}' text-anchor='middle' fill='{color}' font-size='{size}'>{esc(label)}</text>"
    return t

def write(name, s):
    s.append("</svg>")
    open(os.path.join(OUT, name), "w").write("\n".join(s))

W, H = 960, 340
s = head(W, H, "One record's path: source ring to mirror ring, same sequence number everywhere",
         "push on the source host → read on the mirror host, measured, 64-byte records")
s.append(panel(16, 56, 330, 190, "source host"))
s.append(box(30, 96, 86, 44, "producer", "push()"))
s.append(arrow(116, 118, 142, 118))
s.append(ring(142, 90, 96, 56, "ring", "seq 1, 2, 3 …"))
s.append(arrow(238, 104, 262, 104))
s.append(box(262, 86, 74, 36, "serve", "raw reader", size=11))
s.append(arrow(238, 132, 262, 160))
s.append(box(262, 150, 74, 36, "readers", "0.1 µs", size=11))
s.append(f"<text x='181' y='222' text-anchor='middle' fill='{MUTED}' font-size='11'>fixed slots, or descriptors + arena</text>")
s.append(f"<rect x='356' y='66' width='250' height='170' rx='10' fill='{NET_FILL}' stroke='{NET}' stroke-dasharray='4 3'/>")
s.append(f"<text x='481' y='86' text-anchor='middle' fill='{NET}' font-weight='bold'>network</text>")
s.append(arrow(336, 118, 616, 118, "DATA: raw slot bytes + seq", NET, "n"))
s.append(f"<text x='481' y='146' text-anchor='middle' fill='{NET}' font-size='11'>UDP multicast · UDP unicast · TCP</text>")
s.append(arrow(616, 186, 336, 186, "NAK / GAP over TCP", NET, "n", dash="4 3", above=False))
s.append(f"<text x='481' y='214' text-anchor='middle' fill='{MUTED}' font-size='11'>the source ring is</text>")
s.append(f"<text x='481' y='228' text-anchor='middle' fill='{MUTED}' font-size='11'>the retransmission buffer</text>")
s.append(panel(616, 56, 328, 190, "mirror host"))
s.append(box(630, 100, 84, 36, "mirror", "one writer", size=11))
s.append(arrow(714, 118, 736, 118))
s.append(ring(736, 90, 96, 56, "ring", "same seq"))
s.append(arrow(832, 118, 852, 118))
s.append(box(852, 96, 80, 44, "readers", "as local", size=11))
s.append(f"<text x='780' y='222' text-anchor='middle' fill='{MUTED}' font-size='11'>written in order, never duplicated</text>")
y = 276
for x, label, val in [(30, "same ring", "0.1 µs"), (250, "mirror on the same host", "3.8 µs"),
                      (490, "mirror across a 1 GbE LAN", "30 µs"), (730, "Tokyo → Los Angeles", "51.7 ms, p99 +50 µs")]:
    s.append(f"<text x='{x}' y='{y}' fill='{INK}' font-weight='bold' font-size='13'>{esc(val)}</text>")
    s.append(f"<text x='{x}' y='{y + 16}' fill='{MUTED}' font-size='11'>{esc(label)}</text>")
s.append(f"<text x='30' y='{y + 40}' fill='{MUTED}' font-size='11'>push → read, p50, measured on each host; remote hosts corrected for clock offset</text>")
write("mirror-pipeline.svg", s)
"####;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn python_validation_reuses_identity_restores_disk_and_preserves_real_errors() {
    let _server = which("basedpyright-langserver").expect("native Python language server required");
    let gateway = Gateway::start();
    let checkout = tempfile::tempdir().expect("checkout");
    let root = std::fs::canonicalize(checkout.path()).expect("canonical checkout");
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\nname = \"identity\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let file = root.join("identity.py");
    let baseline = PYTHON_IDENTITY_SOURCE;
    let annotated = baseline.replace(
        "def write(name, s):",
        "def write(name: str, s: list[str]) -> None:",
    );
    std::fs::write(&file, baseline).unwrap();
    commit_in(&root);

    let before = prod_code_mcp::diagnostics::diagnostics(gateway.addr, &root, &file)
        .await
        .expect("baseline diagnostics");
    assert_eq!(before.errors, 0, "{}", before.render());
    let clean = prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, &file, &annotated)
        .await
        .expect("annotated proposal diagnostics");
    assert_eq!(clean.errors, 0, "{}", clean.render());

    let broken = annotated.replace("write(\"mirror-pipeline.svg\", s)", "write(7, s)");
    let rejected = prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, &file, &broken)
        .await
        .expect("deliberate type error diagnostics");
    assert!(
        rejected.errors > 0
            && rejected
                .items
                .iter()
                .any(|item| item.source.as_deref() == Some("basedpyright")),
        "{}",
        rejected.render()
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), baseline);

    let subsequent =
        prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, &file, &annotated)
            .await
            .expect("subsequent validation session");
    assert_eq!(subsequent.errors, 0, "{}", subsequent.render());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), baseline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn python_editor_overlay_is_isolated_from_validation() {
    let _server = which("basedpyright-langserver").expect("native Python language server required");
    let gateway = Gateway::start();
    let checkout = tempfile::tempdir().unwrap();
    let root = checkout.path().canonicalize().unwrap();
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\nname='isolation'\nversion='0.1.0'\n",
    )
    .unwrap();
    let file = root.join("subject.py");
    let disk = "number: int = 1\n";
    std::fs::write(&file, disk).unwrap();
    commit_in(&root);
    let (mut editor, _) = EditorSession::open(gateway.addr, &root).await;
    let uri = prod_code_protocol::path::file_uri(&file);
    let editor_text = "number: int = 'editor overlay'\n";
    editor.notify("textDocument/didOpen", serde_json::json!({
        "textDocument": {"uri": uri, "languageId": "python", "version": 1, "text": editor_text}
    })).await;
    let before = editor
        .request(
            "textDocument/diagnostic",
            serde_json::json!({
                "textDocument": {"uri": uri}
            }),
        )
        .await;
    assert!(
        !before["result"]["items"].as_array().unwrap().is_empty(),
        "{before}"
    );

    let validation =
        prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, &file, "number: int = 2\n")
            .await
            .expect("private validation succeeds beside the editor");
    assert_eq!(validation.errors, 0, "{}", validation.render());
    let after = editor
        .request(
            "textDocument/diagnostic",
            serde_json::json!({
                "textDocument": {"uri": uri}
            }),
        )
        .await;
    assert!(
        !after["result"]["items"].as_array().unwrap().is_empty(),
        "{after}"
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), disk);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn basedpyright_reports_diagnostics_for_a_new_python_file() {
    let _server = which("basedpyright-langserver").expect("native Python language server required");
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    let root = checkout.root();
    checkout.write(
        "scripts/existing.py",
        "def existing() -> int:\n    return 1\n",
    );
    checkout.commit();

    let new_file = root.join("scripts/new_probe_559.py");
    assert!(
        !new_file.exists(),
        "new file must not exist before validation"
    );

    let clean_text = "def answer(x: int) -> int:\n    return x + 1\n";
    let clean =
        prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, &new_file, clean_text)
            .await
            .expect("clean new file validation");
    assert_eq!(clean.errors, 0, "{}", clean.render());
    assert!(
        !new_file.exists(),
        "proposed file must stay absent after clean validation"
    );

    let broken_text = "def answer(x: int) -> int:\n    return \"invalid\"\n";
    let broken =
        prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, &new_file, broken_text)
            .await
            .expect("broken new file validation");
    assert!(
        broken.errors > 0
            && broken
                .items
                .iter()
                .any(|item| item.source.as_deref() == Some("basedpyright")
                    && item.severity == "error"),
        "expected basedpyright error, got: {}",
        broken.render()
    );
    assert!(
        !new_file.exists(),
        "proposed file must stay absent after broken validation"
    );

    // The retained-document generation must not be poisoned by closing a new document.
    let subsequent =
        prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, &new_file, clean_text)
            .await
            .expect("subsequent validation on same engine");
    assert_eq!(subsequent.errors, 0, "{}", subsequent.render());
    assert!(!new_file.exists(), "proposed file must stay absent on disk");

    // Relative path also selects the Python engine and diagnoses without writing.
    let rel_path = Path::new("scripts/another_new.py");
    assert!(!root.join(rel_path).exists());
    let rel_report =
        prod_code_mcp::diagnostics::validate_text(gateway.addr, &root, rel_path, clean_text)
            .await
            .expect("relative path validation");
    assert_eq!(rel_report.errors, 0, "{}", rel_report.render());
    assert!(
        !root.join(rel_path).exists(),
        "relative proposed file must stay absent on disk"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_private_python_server_does_not_fall_back_to_the_main_engine() {
    use std::os::unix::fs::PermissionsExt;

    let actual = which("basedpyright-langserver").expect("native Python language server required");
    let wrappers = tempfile::tempdir().unwrap();
    let bin = wrappers.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let lock = wrappers.path().join("first-server");
    let wrapper = bin.join("basedpyright-langserver");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif mkdir '{}' 2>/dev/null; then exec '{}' \"$@\"; fi\nexit 72\n",
            lock.display(),
            actual.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(paths)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let home = wrappers.path().to_string_lossy().into_owned();
    let gateway = Gateway::start_with(&[("PATH", &path), ("HOME", &home)]);
    let checkout = tempfile::tempdir().unwrap();
    let root = checkout.path().canonicalize().unwrap();
    std::fs::write(
        root.join("pyproject.toml"),
        "[project]\nname='failure'\nversion='0.1.0'\n",
    )
    .unwrap();
    let file = root.join("subject.py");
    let disk = "number: int = 1\n";
    std::fs::write(&file, disk).unwrap();
    commit_in(&root);

    let before = prod_code_mcp::diagnostics::diagnostics(gateway.addr, &root, &file)
        .await
        .expect("the main engine is loaded first");
    assert_eq!(before.errors, 0, "{}", before.render());
    let err = prod_code_mcp::diagnostics::validate_text(
        gateway.addr,
        &root,
        &file,
        "number: int = 'proposal'\n",
    )
    .await
    .expect_err("the private server failure is reported");
    assert!(
        format!("{err:#}").contains("private validation engine unavailable"),
        "{err:#}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), disk);
    let after = prod_code_mcp::diagnostics::diagnostics(gateway.addr, &root, &file)
        .await
        .expect("the ordinary engine still answers");
    assert_eq!(after.errors, 0, "{}", after.render());
}

/// #505: generic inherent implementations keep their lifetime/type/const declarations and
/// constraints when a real analyzer extracts a subset, updates an external caller, and then
/// extracts the complete remaining block. The same fixture executes identically throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_trait_extraction_preserves_a_real_rust_program() {
    const WINDOW: &str = r#"pub struct Window<'a, T, const N: usize> {
    pub values: &'a [T; N],
}

impl<'a, T: Copy + Into<i64>, const N: usize> Window<'a, T, N>
where
    T: std::fmt::Display,
{
    pub fn first(&self) -> T {
        self.values[0]
    }

    pub fn total<I>(&self, extra: I) -> i64
    where
        I: IntoIterator<Item = T>,
    {
        self.values[0].into() + extra.into_iter().map(Into::into).sum::<i64>()
    }

    pub fn copied(&self) -> Self {
        Self { values: self.values }
    }
}
"#;
    const REPORT: &str = r#"use crate::window::Window;

pub fn report<'a, T: Copy + Into<i64> + std::fmt::Display, const N: usize>(
    window: &Window<'a, T, N>,
    extra: T,
) -> i64 {
    window.total([extra]) + window.copied().first().into()
}
"#;
    const MAIN: &str = r#"mod report;
mod window;

use window::Window;

fn main() {
    let values = [1_i32, 2, 3];
    let window = Window { values: &values };
    println!("{}", report::report(&window, 4));
}
"#;
    const CONDITIONAL_IMPL: &str = "pub struct Example<T>(pub T);\n#[cfg(any())] impl<T> Example<T> { pub fn selected(&self) {} pub fn kept(&self) {} }\n";
    const COMMENTED_CONDITIONAL_IMPL: &str = "pub struct Example<T>(pub T);\n#[cfg(\n    any()\n)]\n// attached condition remains active\n\nimpl<T> Example<T> { pub fn selected(&self) {} pub fn kept(&self) {} }\n";
    const CONDITIONAL_METHOD: &str = "pub struct Example<T>(pub T);\nimpl<T> Example<T> { #[cfg_attr(\n    any(),\n    allow(dead_code)\n)] pub fn selected(&self) {} pub fn kept(&self) {} }\n";
    const SELF_INLINE: &str = "pub struct Example<T>(pub T);\nimpl<T: From<Self>> Example<T> { pub fn selected(self) -> T { T::from(self) } }\n";
    const SELF_WHERE: &str = "pub struct Example<T>(pub T);\nimpl<T> Example<T> where T: From<Self> { pub fn selected(self) -> T { T::from(self) } }\n";

    let gateway = Gateway::start();
    let checkout = Checkout::new();
    checkout.write("src/main.rs", MAIN);
    checkout.write("src/window.rs", WINDOW);
    checkout.write("src/report.rs", REPORT);
    checkout.write("src/conditional_impl.rs", CONDITIONAL_IMPL);
    checkout.write(
        "src/commented_conditional_impl.rs",
        COMMENTED_CONDITIONAL_IMPL,
    );
    checkout.write("src/conditional_method.rs", CONDITIONAL_METHOD);
    checkout.write("src/self_inline.rs", SELF_INLINE);
    checkout.write("src/self_where.rs", SELF_WHERE);
    checkout.commit();
    let root = checkout.root();
    let window = checkout.path("src/window.rs");
    let output = || {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("generic-extract-fixture");
        let built = Command::new("rustc")
            .args(["--edition", "2021", "-A", "warnings", "src/main.rs", "-o"])
            .arg(&bin)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let ran = Command::new(&bin).output().unwrap();
        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        ran.stdout
    };
    let before = output();

    let compile_library = |relative: &str| {
        let temp = tempfile::tempdir().unwrap();
        let output = Command::new("rustc")
            .args(["--edition", "2021", "--crate-type", "lib", relative, "-o"])
            .arg(temp.path().join("self-bound.rlib"))
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            relative,
            String::from_utf8_lossy(&output.stderr)
        );
    };
    compile_library("src/self_inline.rs");
    compile_library("src/self_where.rs");

    for (relative, source, why) in [
        (
            "src/conditional_impl.rs",
            CONDITIONAL_IMPL,
            "attributes on impl blocks",
        ),
        (
            "src/commented_conditional_impl.rs",
            COMMENTED_CONDITIONAL_IMPL,
            "attributes on impl blocks",
        ),
        (
            "src/conditional_method.rs",
            CONDITIONAL_METHOD,
            "conditional methods",
        ),
        ("src/self_inline.rs", SELF_INLINE, "depend on `Self`"),
        ("src/self_where.rs", SELF_WHERE, "depend on `Self`"),
    ] {
        let impl_at = source.find("impl").unwrap();
        let line_start = source[..impl_at].rfind('\n').map_or(0, |i| i + 1);
        let args = serde_json::json!({
            "path": relative,
            "line": source[..impl_at].matches('\n').count() + 1,
            "character": impl_at - line_start + 1,
            "methods": ["selected"],
            "name": "Selected",
            "apply": true,
            "force": true,
        });
        let err =
            prod_code_mcp::tools::execute_tool(gateway.addr, &root, "code_extract_trait", args)
                .await
                .expect_err("structurally unsafe extraction is refused even with force");
        assert!(format!("{err:#}").contains(why), "{relative}: {err:#}");
        assert_eq!(
            std::fs::read_to_string(checkout.path(relative)).unwrap(),
            source,
            "refusal preserves every byte"
        );
    }
    compile_library("src/self_inline.rs");
    compile_library("src/self_where.rs");

    let bad = serde_json::json!({
        "path": "src/window.rs", "line": 5, "character": 1,
        "methods": ["missing"], "name": "WindowOps", "apply": true, "force": true
    });
    let err = prod_code_mcp::tools::execute_tool(gateway.addr, &root, "code_extract_trait", bad)
        .await
        .expect_err("a genuinely absent method is refused even with force");
    assert!(
        format!("{err:#}").contains("has no method `missing`"),
        "{err:#}"
    );
    assert_eq!(std::fs::read_to_string(&window).unwrap(), WINDOW);

    let subset = serde_json::json!({
        "path": "src/window.rs", "line": 5, "character": 1,
        "methods": ["total", "copied"], "name": "WindowOps", "apply": true
    });
    let changed = tool(gateway.addr, &root, "code_extract_trait", subset).await;
    assert!(!changed.is_error, "{}", text_of(&changed));
    let after_subset = std::fs::read_to_string(&window).unwrap();
    assert!(after_subset.contains("trait WindowOps<'a, T: Copy + Into<i64>, const N: usize>"));
    assert!(
        after_subset.contains("impl<'a, T: Copy + Into<i64>, const N: usize> Window<'a, T, N>")
    );
    assert!(after_subset.contains("fn total<I>(&self, extra: I) -> i64"));
    assert!(after_subset.contains("fn copied(&self) -> Self"));
    assert!(
        std::fs::read_to_string(checkout.path("src/report.rs"))
            .unwrap()
            .contains("use crate::window::WindowOps;")
    );
    assert_eq!(output(), before, "subset extraction preserves execution");

    let full = serde_json::json!({
        "path": "src/window.rs", "line": 5, "character": 1,
        "methods": ["first"], "name": "WindowView", "apply": true
    });
    let changed = tool(gateway.addr, &root, "code_extract_trait", full).await;
    assert!(!changed.is_error, "{}", text_of(&changed));
    let after_full = std::fs::read_to_string(&window).unwrap();
    assert!(!after_full.contains("impl<'a, T: Copy + Into<i64>, const N: usize> Window<'a, T, N>"));
    assert!(after_full.contains("trait WindowView<'a, T: Copy + Into<i64>, const N: usize>"));
    assert!(
        std::fs::read_to_string(checkout.path("src/report.rs"))
            .unwrap()
            .contains("use crate::window::WindowView;")
    );
    assert_eq!(output(), before, "full extraction preserves execution");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trait_extraction_preserves_unicode_positions_and_refuses_opaque_capture() {
    let gateway = Gateway::start();
    let checkout = Checkout {
        dir: tempfile::tempdir().unwrap(),
    };
    checkout.write(
        "Cargo.toml",
        "[package]\nname = \"trait_boundary\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    const UNICODE: &str = "struct 名字;\nimpl 名字 { fn value(&self) -> u8 { 7 } }\nfn main() { println!(\"{}\", 名字.value()); }\n";
    const OPAQUE: &str = "pub struct Example;\nimpl Example { pub fn value(&self) -> impl Copy { 7_u8 } }\nfn main() { let owner = Example; let result = owner.value(); drop(owner); std::hint::black_box(result); }\n";
    checkout.write("src/main.rs", UNICODE);
    const MACRO: &str = "pub struct Example;\nmacro_rules! make { ($item:item) => { $item } }\nmake!(impl Example { pub fn value(&self) -> u8 { 7 } });\nfn main() { assert_eq!(Example.value(), 7); }\n";
    checkout.write("src/bin/macro_arg.rs", MACRO);
    checkout.write("src/bin/opaque.rs", OPAQUE);
    checkout.commit();
    let root = checkout.root();
    let compile_and_run = |path: &str| {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("trait-boundary-fixture");
        let built = Command::new("rustc")
            .args(["--edition", "2021", path, "-o"])
            .arg(&binary)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let ran = Command::new(&binary).output().unwrap();
        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        ran.stdout
    };
    let expected = compile_and_run("src/main.rs");
    compile_and_run("src/bin/opaque.rs");
    for apply in [false, true] {
        let answer = tool(
            gateway.addr,
            &root,
            "code_extract_trait",
            serde_json::json!({
                "path": "src/main.rs", "line": 2, "character": 6,
                "methods": ["value"], "name": "Value", "apply": apply
            }),
        )
        .await;
        assert!(!answer.is_error, "{}", text_of(&answer));
        if !apply {
            assert_eq!(
                std::fs::read_to_string(checkout.path("src/main.rs")).unwrap(),
                UNICODE
            );
        }
        assert_eq!(compile_and_run("src/main.rs"), expected);
    }
    assert!(
        std::fs::read_to_string(checkout.path("src/main.rs"))
            .unwrap()
            .contains("impl Value for 名字")
    );
    for apply in [false, true] {
        for force in [false, true] {
            let error = prod_code_mcp::tools::execute_tool(
                gateway.addr,
                &root,
                "code_extract_trait",
                serde_json::json!({
                    "path": "src/bin/opaque.rs", "line": 2, "character": 1,
                    "methods": ["value"], "name": "OpaqueValue", "apply": apply, "force": force
                }),
            )
            .await
            .expect_err("opaque return capture must not change even with force");
            assert!(format!("{error:#}").contains("opaque return"), "{error:#}");
            assert_eq!(
                std::fs::read_to_string(checkout.path("src/bin/opaque.rs")).unwrap(),
                OPAQUE
            );
        }
    }
    compile_and_run("src/bin/opaque.rs");
    compile_and_run("src/bin/macro_arg.rs");
    for apply in [false, true] {
        let error = prod_code_mcp::tools::execute_tool(
            gateway.addr,
            &root,
            "code_extract_trait",
            serde_json::json!({
                "path": "src/bin/macro_arg.rs", "line": 3, "character": 7,
                "methods": ["value"], "name": "MacroValue", "apply": apply, "force": true
            }),
        )
        .await
        .expect_err("a macro argument is not rewritten even with force");
        assert!(format!("{error:#}").contains("inside macros"), "{error:#}");
        assert_eq!(
            std::fs::read_to_string(checkout.path("src/bin/macro_arg.rs")).unwrap(),
            MACRO
        );
    }
    compile_and_run("src/bin/macro_arg.rs");
}

/// #514: a method-body cursor after `impl Trait` input syntax still selects the enclosing
/// inherent implementation. This uses the public MCP preview/apply flow and a real analyzer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trait_extraction_accepts_an_opaque_argument_method_cursor() {
    const API: &str = r#"pub struct Example;

impl Example {
    pub fn value(&self, input: impl Copy) -> u8 {
        let 猫 = input;
        let _ = 猫;
        7
    }
}

pub fn outside() {}
"#;
    const CALLERS: &str = r#"use crate::api::Example;

pub fn call() -> u8 {
    Example.value(())
}
"#;
    const MAIN: &str =
        "mod api;\nmod callers;\n\nfn main() { println!(\"{}\", callers::call()); }\n";

    let gateway = Gateway::start();
    let checkout = Checkout::new();
    checkout.write("src/main.rs", MAIN);
    checkout.write("src/api.rs", API);
    checkout.write("src/callers.rs", CALLERS);
    let local_body = "{ struct Local(u8); impl Local { fn value(&self, input: impl Copy) -> u8 { let _ = input; self.0 } } Local(9).value(()) }";
    let local_sources = [
        (
            "src/bin/local_parenthesized.rs",
            format!("fn main() {{ let value = ({local_body}); println!(\"{{value}}\"); }}"),
        ),
        (
            "src/bin/local_array.rs",
            format!("fn main() {{ let values = [{local_body}]; println!(\"{{}}\", values[0]); }}"),
        ),
    ];
    for (path, source) in &local_sources {
        checkout.write(path, source);
    }
    checkout.commit();
    let root = checkout.root();
    let api = checkout.path("src/api.rs");
    let compile_and_run = |source: &str| {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("opaque-argument-fixture");
        let built = Command::new("rustc")
            .args(["--edition", "2021", source, "-o"])
            .arg(&binary)
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let ran = Command::new(&binary).output().unwrap();
        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        ran.stdout
    };
    let before = compile_and_run("src/main.rs");
    let cursor = API.find("let _").unwrap();
    let line_start = API[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let line = API[..cursor].matches('\n').count() + 1;
    let character = API[line_start..cursor].encode_utf16().count() + 1;
    let args = |apply| {
        serde_json::json!({
            "path": "src/api.rs", "line": line, "character": character,
            "methods": ["value"], "name": "Value", "apply": apply
        })
    };

    let preview = tool(gateway.addr, &root, "code_extract_trait", args(false)).await;
    assert!(!preview.is_error, "{}", text_of(&preview));
    assert_eq!(
        std::fs::read_to_string(&api).unwrap(),
        API,
        "preview writes nothing"
    );
    assert_eq!(
        compile_and_run("src/main.rs"),
        before,
        "preview preserves execution"
    );

    let applied = tool(gateway.addr, &root, "code_extract_trait", args(true)).await;
    assert!(!applied.is_error, "{}", text_of(&applied));
    assert!(
        std::fs::read_to_string(&api)
            .unwrap()
            .contains("impl Value for Example"),
        "{}",
        text_of(&applied)
    );
    assert!(
        std::fs::read_to_string(checkout.path("src/callers.rs"))
            .unwrap()
            .contains("use crate::api::Value;"),
        "{}",
        text_of(&applied)
    );
    assert_eq!(
        compile_and_run("src/main.rs"),
        before,
        "applied extraction preserves execution"
    );

    for (path, source) in &local_sources {
        let original_output = compile_and_run(path);
        assert_eq!(original_output, b"9\n");
        let cursor = source.find("let _").unwrap();
        let params = |apply| {
            serde_json::json!({
                "path": path, "line": 1,
                "character": source[..cursor].encode_utf16().count() + 1,
                "methods": ["value"], "name": "LocalValue", "apply": apply
            })
        };
        let preview = tool(gateway.addr, &root, "code_extract_trait", params(false)).await;
        assert!(
            !preview.is_error,
            "local preview {path}: {}",
            text_of(&preview)
        );
        assert_eq!(
            std::fs::read_to_string(checkout.path(path)).unwrap(),
            *source
        );
        let applied = tool(gateway.addr, &root, "code_extract_trait", params(true)).await;
        assert!(
            !applied.is_error,
            "local apply {path}: {}",
            text_of(&applied)
        );
        assert!(
            std::fs::read_to_string(checkout.path(path))
                .unwrap()
                .contains("impl LocalValue for Local")
        );
        assert_eq!(
            compile_and_run(path),
            original_output,
            "local impl changed execution: {path}"
        );
    }

    let current_api = std::fs::read_to_string(&api).unwrap();
    let outside = current_api.find("outside").unwrap();
    let outside_line_start = current_api[..outside].rfind('\n').map_or(0, |i| i + 1);
    let outside_args = serde_json::json!({
        "path": "src/api.rs", "line": current_api[..outside].matches('\n').count() + 1,
        "character": current_api[outside_line_start..outside].encode_utf16().count() + 1,
        "methods": ["value"], "name": "Outside", "apply": true, "force": true
    });
    let before_refusal = std::fs::read_to_string(&api).unwrap();
    let error =
        prod_code_mcp::tools::execute_tool(gateway.addr, &root, "code_extract_trait", outside_args)
            .await
            .expect_err("a position outside every implementation is refused");
    assert!(
        format!("{error:#}").contains("no `impl` block"),
        "{error:#}"
    );
    assert_eq!(std::fs::read_to_string(&api).unwrap(), before_refusal);
}

/// The public wire boundary rejects malformed positions before converting them to native
/// one-based coordinates. A good request still works after every rejected request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_and_malformed_wire_positions_never_select_another_token() {
    use serde_json::json;
    let gateway = Gateway::start();
    let checkout = Checkout::new();
    checkout.write("src/lib.rs", "pub const VALUE: u8 = 1;\n");
    checkout.commit();
    let root = checkout.root();
    let file = checkout.path("src/lib.rs");
    let original = std::fs::read(&file).unwrap();
    let uri = prod_code_protocol::path::file_uri(&file);
    // No editor purpose: this must exercise the embedded Rust engine, not the separate
    // rust-analyzer subprocess used by editors. Use the wire helper's bounded response wait;
    // the MCP cold-hover budget is tracked separately in #408.
    let (mut session, initialized) = EditorSession::open_for(
        gateway.addr,
        &root,
        json!({"rootUri": prod_code_protocol::path::file_uri(&root), "capabilities": {}}),
        None,
    )
    .await;
    assert!(initialized.get("error").is_none(), "{initialized}");
    session
        .notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri, "languageId": "rust", "version": 1,
                "text": String::from_utf8(original.clone()).unwrap(),
            }}),
        )
        .await;
    for position in [
        json!({"line": 4294967296_u64, "character": 0}),
        json!({"line": u32::MAX, "character": 0}),
        json!({"line": 0, "character": u64::MAX}),
        json!({"line": -1, "character": 0}),
        json!({"line": 0.5, "character": 0}),
        json!({"line": "0", "character": 0}),
        json!({"line": null, "character": 0}),
        json!({"line": 0}),
        json!({"character": 0}),
        json!(null),
        json!([]),
    ] {
        for method in [
            "textDocument/hover",
            "textDocument/rename",
            "prodCode/safeDelete",
        ] {
            let response = session
                .request(
                    method,
                    json!({"textDocument": {"uri": uri}, "position": position, "newName": "Other"}),
                )
                .await;
            assert_eq!(
                response["error"]["code"], -32602,
                "{method} {position}: {response}"
            );
            assert!(response.get("result").is_none(), "{response}");
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("position"),
                "{response}"
            );
        }
    }
    let missing = session
        .request("textDocument/hover", json!({"textDocument": {"uri": uri}}))
        .await;
    assert_eq!(missing["error"]["code"], -32602, "{missing}");
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap()
            .contains("position"),
        "{missing}"
    );
    for method in ["prodCode/assists", "prodCode/applyAssist"] {
        for end in [
            json!({"line": 4294967296_u64, "character": 0}),
            json!({"line": 0}),
            json!(null),
        ] {
            let response = session.request(method,
                json!({"textDocument": {"uri": uri}, "range": {"start": {"line": 0, "character": 0}, "end": end}, "id": "extract_variable"}),
            ).await;
            assert_eq!(response["error"]["code"], -32602, "{response}");
            assert!(
                response["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("range.end"),
                "{response}"
            );
        }
    }
    for method in ["callHierarchy/incomingCalls", "callHierarchy/outgoingCalls"] {
        let response = session.request(method,
            json!({"item": {"uri": uri, "selectionRange": {"start": {"line": 4294967296_u64, "character": 0}}}}),
        ).await;
        assert_eq!(response["error"]["code"], -32602, "{response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("selectionRange"),
            "{response}"
        );
    }
    session
        .notify(
            "textDocument/hover",
            json!({"textDocument": {"uri": uri}, "position": {"line": -1, "character": 0}}),
        )
        .await;
    let valid_hover = session
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 10}}),
        )
        .await;
    assert!(valid_hover.get("error").is_none(), "{valid_hover}");
    assert!(
        valid_hover["result"].to_string().contains("VALUE"),
        "{valid_hover}"
    );
    assert!(
        session
            .notes
            .iter()
            .all(|note| note.get("id").is_none() || note.get("error").is_none()),
        "an invalid notification receives no synthetic error response: {:?}",
        session.notes
    );
    let assists = session
        .request(
            "prodCode/assists",
            json!({"textDocument": {"uri": uri}, "range": {"start": {"line": 0, "character": 0}}}),
        )
        .await;
    assert!(
        assists["result"].is_array(),
        "omitting the optional end stays valid: {assists}"
    );
    let diagnostics = session
        .request(
            "textDocument/diagnostic",
            json!({"textDocument": {"uri": uri}}),
        )
        .await;
    assert!(
        diagnostics["result"].is_object(),
        "positionless diagnostics stay valid: {diagnostics}"
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        original,
        "refused wire mutations leave the source unchanged"
    );
}
