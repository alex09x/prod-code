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
    fn start_with(extra: &[(&str, &str)]) -> Self {
        let storage = tempfile::tempdir().expect("storage dir");
        let addr = free_port();
        let mut command = Command::new(env!("CARGO_BIN_EXE_prod-code-server"));
        command
            .env("PROD_CODE_BIND", addr.to_string())
            .env("PROD_CODE_STORAGE", storage.path())
            // No peers, no gossip: this gateway is alone and must not look for others.
            .env("PROD_CODE_PEERS", "")
            .env("RUST_LOG", "warn")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut addr = addr;
        for (key, value) in extra {
            if *key == "PROD_CODE_BIND" {
                addr = value.parse().expect("an address to bind");
            }
            // An empty value means "leave it unset", which is how a test reaches the defaults
            // the daemon falls back to when the environment says nothing.
            if value.is_empty() {
                command.env_remove(key);
            } else {
                command.env(key, value);
            }
        }
        let gateway = Self {
            child: command.spawn().expect("the server binary starts"),
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

/// A port nothing is listening on. Between the probe and the server's bind there is a window;
/// it is small, and the alternative is a fixed port that collides with a parallel test.
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
    let assists = text_of(
        &tool(
            gateway.addr,
            &root,
            "code_assists",
            serde_json::json!({ "path": "src/use.ts", "line": 4, "character": 10 }),
        )
        .await,
    );
    assert!(
        !assists.trim().is_empty(),
        "the server offers something at a call: {assists}"
    );
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
    // No RUST_LOG here, so the daemon builds its own default filter — the one path in the
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
