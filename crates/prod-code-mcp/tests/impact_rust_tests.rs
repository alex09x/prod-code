//! Public Rust impact selection regression: only runnable attributed tests are selectable.

use prod_code_mcp::impact::{self, CiRun, Symbol};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::process::Command;
use std::sync::Arc;
#[cfg(unix)]
use std::{
    io::{BufRead, BufReader, Read},
    net::{SocketAddr, TcpStream},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ExitStatus, Stdio},
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

const CARGO_TOML: &str =
    "[package]\nname = \"impact-rust-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
const LIB: &str = "pub fn callee() -> u32 {\n    1\n}\n";
const TEST: &str = "fn test_helper() -> u32 {\n    impact_rust_fixture::callee()\n}\n\n#[test]\nfn actual_test() {\n    assert_eq!(test_helper(), 2);\n}\n\nfn unrelated_helper() {}\n";
#[cfg(unix)]
const NATIVE_TEST: &str = "fn edited_bridge() -> u32 {\n    1\n}\n\n#[test]\nfn direct_case() {\n    assert_eq!(edited_bridge(), 2);\n}\n\n#[test]\nfn nested_case() {\n    fn local_bridge() -> u32 {\n        impact_rust_fixture::callee()\n    }\n    assert_eq!(local_bridge(), 2);\n}\n\n#[test]\nfn unrelated_case() {\n    assert_eq!(1, 1);\n}\n";

fn sym(name: &str, file: &str, line: u32, col: u32) -> Symbol {
    Symbol {
        name: name.into(),
        file: file.into(),
        line,
        col,
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(unix)]
struct OwnedProcessGroup {
    child: Child,
    pgid: i32,
    readers: Vec<JoinHandle<Vec<u8>>>,
    reaped: bool,
}

#[cfg(unix)]
impl OwnedProcessGroup {
    fn new(child: Child) -> Self {
        let pgid = child.id() as i32;
        Self {
            child,
            pgid,
            readers: Vec::new(),
            reaped: false,
        }
    }

    fn capture<R: Read + Send + 'static>(&mut self, mut reader: R) {
        self.readers.push(std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = reader.read_to_end(&mut bytes);
            bytes
        }));
    }

    fn capture_output(&mut self) {
        let stdout = self.child.stdout.take().expect("child stdout is piped");
        let stderr = self.child.stderr.take().expect("child stderr is piped");
        self.capture(stdout);
        self.capture(stderr);
    }

    fn capture_gateway_stdout(&mut self) -> mpsc::Receiver<SocketAddr> {
        let stdout = self.child.stdout.take().expect("gateway stdout is piped");
        let (tx, rx) = mpsc::channel();
        self.readers.push(std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut bytes = Vec::new();
            let mut line = Vec::new();
            let mut found = false;
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                bytes.extend_from_slice(&line);
                if !found
                    && let Ok(text) = std::str::from_utf8(&line)
                    && let Some(rest) = text.split("listening on ").nth(1)
                    && let Ok(addr) = rest.trim().parse()
                {
                    found = true;
                    let _ = tx.send(addr);
                }
            }
            bytes
        }));
        rx
    }

    fn capture_stderr(&mut self) {
        let stderr = self.child.stderr.take().expect("gateway stderr is piped");
        self.capture(stderr);
    }

    fn signal(&self, signal: i32) {
        unsafe {
            kill(-self.pgid, signal);
        }
    }

    fn group_alive(&self) -> bool {
        unsafe { kill(-self.pgid, 0) == 0 }
    }

    fn wait_bounded(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = true;
                    self.stop_remaining_descendants();
                    return Some(status);
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(None) | Err(_) => return None,
            }
        }
    }

    fn stop_remaining_descendants(&self) {
        if !self.group_alive() {
            return;
        }
        self.signal(15);
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.group_alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if self.group_alive() {
            self.signal(9);
        }
    }

    fn shutdown(&mut self) {
        if self.reaped {
            return;
        }
        self.signal(15);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    self.reaped = true;
                    break;
                }
                Ok(None) if Instant::now() < deadline && self.group_alive() => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => break,
            }
        }
        if self.group_alive() {
            self.signal(9);
        }
        if !self.reaped {
            let _ = self.child.wait();
            self.reaped = true;
        }
    }

    fn collected_output(&mut self) -> String {
        let mut bytes = Vec::new();
        for reader in self.readers.drain(..) {
            if let Ok(mut read) = reader.join() {
                bytes.append(&mut read);
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

#[cfg(unix)]
impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        self.shutdown();
        let _ = self.collected_output();
    }
}

#[cfg(unix)]
fn run_bounded(mut command: Command, timeout: Duration, label: &str) -> String {
    command
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("{label} starts: {error}"));
    let mut process = OwnedProcessGroup::new(child);
    process.capture_output();
    let Some(status) = process.wait_bounded(timeout) else {
        panic!("{label} did not finish within {timeout:?}");
    };
    let output = process.collected_output();
    assert!(
        status.success(),
        "{label} failed with {status}: {}",
        output
            .chars()
            .rev()
            .take(8000)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    );
    output
}

#[cfg(unix)]
struct NativeGateway {
    process: OwnedProcessGroup,
    addr: SocketAddr,
    _storage: tempfile::TempDir,
}

#[cfg(unix)]
impl NativeGateway {
    fn start(binary: &Path) -> Self {
        let storage = tempfile::tempdir().expect("private gateway storage");
        let mut command = Command::new(binary);
        command
            .env("PROD_CODE_STORAGE", storage.path())
            .env("PROD_CODE_PEERS", "")
            .env("PROD_CODE_BIND", "127.0.0.1:0")
            .env_remove("RUST_LOG")
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command.spawn().expect("source-built gateway starts");
        let mut process = OwnedProcessGroup::new(child);
        let address = process.capture_gateway_stdout();
        process.capture_stderr();
        let addr = address
            .recv_timeout(Duration::from_secs(60))
            .expect("gateway reports its loopback address within a minute");
        let deadline = Instant::now() + Duration::from_secs(30);
        while TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_err() {
            assert!(
                Instant::now() < deadline,
                "source-built gateway never became ready at {addr}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        Self {
            process,
            addr,
            _storage: storage,
        }
    }
}

#[cfg(unix)]
impl Drop for NativeGateway {
    fn drop(&mut self) {
        self.process.shutdown();
        let _ = self.process.collected_output();
    }
}

#[cfg(unix)]
fn source_built_gateway() -> (tempfile::TempDir, PathBuf) {
    let target = tempfile::tempdir().expect("separate gateway target directory");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let mut build = Command::new("cargo");
    build
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .args([
            "build",
            "--locked",
            "--quiet",
            "-p",
            "prod-code-gateway",
            "--bin",
            "prod-code-server",
            "--target-dir",
        ])
        .arg(target.path());
    run_bounded(build, Duration::from_secs(600), "source gateway build");
    let binary = target.path().join("debug").join("prod-code-server");
    assert!(
        binary.is_file(),
        "source-built gateway at {}",
        binary.display()
    );
    (target, binary)
}

#[test]
fn rust_attributes_belong_only_to_the_declaration_they_annotate() {
    let nested = "#[test]\nfn actual() {\n    { fn deep() {} deep(); }\n    fn helper() { callee(); }\n    helper();\n}\n";
    assert_eq!(
        impact::test_marker("rust", nested, 2, "actual").as_deref(),
        Some("actual")
    );
    assert_eq!(
        impact::test_marker("rust", nested, 4, "helper"),
        None,
        "the outer test attribute must not make a nested helper a test"
    );
    assert_eq!(impact::test_marker("rust", nested, 3, "deep"), None);

    let adjacent = "#[test] fn first() {} fn second() {}\n";
    assert!(impact::test_marker("rust", adjacent, 1, "first").is_some());
    assert_eq!(impact::test_marker("rust", adjacent, 1, "second"), None);

    let spaced = "# /* between */ [ tokio\n    :: /* path */ test(\n        flavor = \"current_thread\"\n    ) ]\npub async fn spaced() {}\n";
    assert!(
        impact::test_marker("rust", spaced, 5, "module::spaced").is_some(),
        "ordinary whitespace and comments may separate attribute tokens"
    );

    let opaque = r####"const TEXT: &str = "#[test] fn invented() {}";
const RAW: &str = r##"#[test] fn invented_too() {}"##;
fn borrow<'a>(text: &'a str) -> &'a str { let marker = '#'; text }
// #[test]
fn plain() {}
#[contest]
fn longer_attribute() {}
#[test]
fn after_lifetimes() {}
"####;
    assert_eq!(impact::test_marker("rust", opaque, 5, "plain"), None);
    assert_eq!(
        impact::test_marker("rust", opaque, 7, "longer_attribute"),
        None
    );
    assert!(
        impact::test_marker("rust", opaque, 9, "after_lifetimes").is_some(),
        "lifetimes are not character literals and literals cannot invent attributes"
    );

    let longer_name = "#[test]\nfn actual_more() {}\n";
    assert_eq!(
        impact::test_marker("rust", longer_name, 2, "actual"),
        None,
        "declarations match whole identifiers"
    );
}

#[tokio::test]
async fn rust_impact_selects_the_attributed_test_and_executes_it_not_test_helpers() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", LIB),
        ("tests/impact.rs", TEST),
    ]);
    let root = ws.root();
    ws.write("src/lib.rs", &LIB.replace("1", "2"));
    let lib_uri = prod_code_protocol::path::file_uri(ws.path("src/lib.rs").as_path());
    let test_uri = prod_code_protocol::path::file_uri(ws.path("tests/impact.rs").as_path());
    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("callee", 12, 1, 3, 8),])
        }
        "textDocument/prepareCallHierarchy" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|uri| uri.as_str())
                .unwrap_or("");
            let line = params
                .pointer("/position/line")
                .and_then(|line| line.as_u64());
            match (uri, line) {
                (uri, Some(0)) if uri == lib_uri => {
                    serde_json::json!([{ "name": "callee", "uri": lib_uri, "_id": "callee" }])
                }
                (uri, Some(0)) if uri == test_uri => serde_json::json!([
                    { "name": "test_helper", "uri": test_uri, "_id": "test_helper" }
                ]),
                (uri, Some(5)) if uri == test_uri => serde_json::json!([
                    { "name": "tests::actual_test", "uri": test_uri, "_id": "actual_test" }
                ]),
                _ => serde_json::json!([]),
            }
        }
        "callHierarchy/incomingCalls" => {
            match params.pointer("/item/_id").and_then(|id| id.as_str()) {
                Some("callee") => serde_json::json!([{
                    "from": {
                        "name": "test_helper",
                        "uri": test_uri,
                        "selectionRange": { "start": { "line": 0, "character": 3 } }
                    }
                }]),
                Some("test_helper") => serde_json::json!([{
                    "from": {
                        "name": "tests::actual_test",
                        "uri": test_uri,
                        "selectionRange": { "start": { "line": 5, "character": 3 } }
                    }
                }]),
                _ => serde_json::json!([]),
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    let report = impact::analyze(remote, &root, None, 4)
        .await
        .expect("analysis runs");

    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert_eq!(report.changed, vec![sym("callee", "src/lib.rs", 1, 8)]);
    assert_eq!(
        report.callers,
        vec![sym("test_helper", "tests/impact.rs", 1, 4)]
    );
    assert_eq!(
        report.tests,
        vec![sym("tests::actual_test", "tests/impact.rs", 6, 4)]
    );
    let command = report.test_command.expect("the actual test is selectable");
    assert!(
        !command.iter().any(|arg| arg == "test_helper"),
        "{command:?}"
    );
    let output = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", root.join("impact-target"))
        .output()
        .expect("the generated test command starts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("actual_test"), "{stdout}");
    assert!(!stdout.contains("test_helper ... ok"), "{stdout}");
}

#[cfg(unix)]
#[tokio::test]
async fn native_rust_analyzer_selects_and_executes_real_tests_through_helpers() {
    let (gateway_target, binary) = source_built_gateway();
    let gateway = NativeGateway::start(&binary);
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", LIB),
        ("tests/impact.rs", NATIVE_TEST),
    ]);
    let root = ws.root();
    ws.write("src/lib.rs", &LIB.replace("1", "2"));
    ws.write(
        "tests/impact.rs",
        &NATIVE_TEST.replacen(
            "fn edited_bridge() -> u32 {\n    1",
            "fn edited_bridge() -> u32 {\n    2",
            1,
        ),
    );

    let report = tokio::time::timeout(
        Duration::from_secs(180),
        impact::analyze(gateway.addr, &root, None, 6),
    )
    .await
    .expect("native impact protocol requests finish within three minutes")
    .expect("native impact analysis succeeds");
    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    let mut changed: Vec<&str> = report
        .changed
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    changed.sort_unstable();
    assert_eq!(changed, ["callee", "edited_bridge"]);
    assert!(
        report
            .callers
            .iter()
            .any(|symbol| symbol.name == "local_bridge"),
        "{:?}",
        report.callers
    );
    let mut selected: Vec<&str> = report
        .tests
        .iter()
        .map(|symbol| symbol.name.rsplit("::").next().unwrap_or(&symbol.name))
        .collect();
    selected.sort_unstable();
    assert_eq!(
        selected,
        ["direct_case", "nested_case"],
        "{:?}",
        report.tests
    );

    let generated = report.test_command.expect("selected tests have a command");
    for name in ["direct_case", "nested_case"] {
        assert!(generated.iter().any(|arg| arg == name), "{generated:?}");
    }
    for helper in ["edited_bridge", "local_bridge"] {
        assert!(!generated.iter().any(|arg| arg == helper), "{generated:?}");
    }
    let fixture_target = tempfile::tempdir().expect("separate fixture target directory");
    let mut command = Command::new(&generated[0]);
    command
        .args(&generated[1..])
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", fixture_target.path());
    let output = run_bounded(
        command,
        Duration::from_secs(180),
        "generated multi-test cargo command",
    );
    for name in ["direct_case", "nested_case"] {
        assert!(output.contains(&format!("test {name} ... ok")), "{output}");
    }
    assert!(
        output.matches("1 passed").count() >= 2,
        "each selected filter must execute a nonzero real test count:\n{output}"
    );
    drop(gateway);
    drop(gateway_target);
}

#[tokio::test]
async fn malformed_rust_test_evidence_runs_the_whole_suite() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", LIB)]);
    let root = ws.root();
    ws.write(
        "src/lib.rs",
        "pub fn callee() -> u32 {\n    2\n}\nconst UNTERMINATED: &str = \"#[test];\n",
    );
    let uri = prod_code_protocol::path::file_uri(ws.path("src/lib.rs").as_path());
    let remote = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("callee", 12, 1, 3, 8),])
        }
        "textDocument/prepareCallHierarchy" => {
            serde_json::json!([{ "name": "callee", "uri": uri, "_id": "callee" }])
        }
        "callHierarchy/incomingCalls" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await
    .addr();

    let report = impact::analyze(remote, &root, None, 4)
        .await
        .expect("analysis runs");

    assert_eq!(report.ci_decision().run, CiRun::WholeSuite);
    assert!(
        report
            .incomplete
            .iter()
            .any(|gap| gap.describe().contains("unterminated string")),
        "{:?}",
        report.incomplete
    );
}
