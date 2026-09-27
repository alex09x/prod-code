#![cfg(unix)]

//! Public CLI and MCP proof for Go function safe-delete against the gateway and gopls built from
//! this revision. The gateway owns the only compiler used by deletion; an isolated child puts a
//! failing go executable first on the client PATH.

use prod_code_mcp::protocol::McpContentItem;
use std::collections::BTreeMap;
use std::io::BufRead;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const SENTINEL_CHILD: &str = "PROD_CODE_SAFE_DELETE_GO_SENTINEL_CHILD";
const SENTINEL_ROOT: &str = "PROD_CODE_SAFE_DELETE_GO_SENTINEL_ROOT";
const SENTINEL_ADDR: &str = "PROD_CODE_SAFE_DELETE_GO_SENTINEL_ADDR";
const CLEANUP_CHILD: &str = "PROD_CODE_SAFE_DELETE_GO_CLEANUP_CHILD";
const CLEANUP_READY: &str = "PROD_CODE_SAFE_DELETE_GO_CLEANUP_READY";

struct Gateway {
    child: Child,
    addr: SocketAddr,
    stdout_reader: Option<std::thread::JoinHandle<()>>,
    _storage: tempfile::TempDir,
}

impl Gateway {
    fn start() -> Self {
        let storage = tempfile::tempdir().expect("storage dir");
        let mut command = Command::new(env!("CARGO_BIN_EXE_prod-code-server"));
        command
            .env("PROD_CODE_STORAGE", storage.path())
            .env("PROD_CODE_PEERS", "")
            .env("PROD_CODE_BIND", "127.0.0.1:0")
            .env_remove("RUST_LOG")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        Self::from_child(command.spawn().expect("gateway starts"), storage)
    }

    fn from_child(child: Child, storage: tempfile::TempDir) -> Self {
        let mut gateway = Self {
            child,
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            stdout_reader: None,
            _storage: storage,
        };
        gateway.addr = bound_address(&mut gateway);
        gateway
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        retire_owned_group(
            &mut self.child,
            self.stdout_reader.take(),
            Duration::from_secs(20),
        );
    }
}

fn wait_for_exit(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn retire_owned_group(
    child: &mut Child,
    stdout_reader: Option<std::thread::JoinHandle<()>>,
    grace: Duration,
) {
    let group = -(child.id() as i32);
    unsafe {
        libc::kill(group, libc::SIGTERM);
    }
    let reaped = wait_for_exit(child, grace).ok().flatten().is_some();
    unsafe {
        libc::kill(group, libc::SIGKILL);
    }
    if !reaped {
        let _ = child.wait();
    }
    if let Some(reader) = stdout_reader {
        let _ = reader.join();
    }
}

fn bound_address(gateway: &mut Gateway) -> SocketAddr {
    let stdout = gateway.child.stdout.take().expect("gateway stdout");
    let (send, receive) = std::sync::mpsc::channel();
    gateway.stdout_reader = Some(std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            if let Some(value) = line.split("listening on ").nth(1)
                && let Ok(addr) = value.trim().parse()
            {
                let _ = send.send(addr);
            }
        }
    }));
    receive
        .recv_timeout(Duration::from_secs(60))
        .expect("gateway reports its address")
}

fn checkout(source: &str, test: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("gosafedelete")
        .tempdir()
        .expect("fixture dir");
    for (rel, text) in [
        ("go.work", "go 1.22\n\nuse ./project\n"),
        (
            "project/go.mod",
            "module example.com/gosafedelete\n\ngo 1.22\n",
        ),
        ("project/main.go", source),
        ("project/main_test.go", test),
    ] {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("mkdir");
        std::fs::write(path, text).expect("fixture write");
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
    let root = std::fs::canonicalize(dir.path()).expect("fixture root");
    (dir, root)
}

fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(directory).expect("read fixture") {
            let path = entry.expect("fixture entry").path();
            if path.is_dir() {
                files.insert(
                    format!(
                        "{}/",
                        path.strip_prefix(root)
                            .expect("directory below root")
                            .to_string_lossy()
                    ),
                    Vec::new(),
                );
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .expect("path below root")
                        .to_string_lossy()
                        .into_owned(),
                    std::fs::read(path).expect("fixture bytes"),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

fn source_cli() -> PathBuf {
    static CLI: OnceLock<PathBuf> = OnceLock::new();
    CLI.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("gateway crate below workspace")
            .to_path_buf();
        let status = Command::new("cargo")
            .args(["build", "-p", "prod-code-client", "--bin", "prod-code"])
            .current_dir(&workspace)
            .status()
            .expect("source client build starts");
        assert!(status.success(), "source client build: {status}");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join("target"));
        let target = if target.is_absolute() {
            target
        } else {
            workspace.join(target)
        };
        let binary = target
            .join("debug")
            .join(format!("prod-code{}", std::env::consts::EXE_SUFFIX));
        assert!(
            binary.is_file(),
            "source client missing: {}",
            binary.display()
        );
        binary
    })
    .clone()
}

fn cli(root: &Path, addr: SocketAddr, args: &[&str]) -> (bool, String) {
    let output = Command::new(source_cli())
        .args(["--remote", &addr.to_string()])
        .args(args)
        .current_dir(root)
        .output()
        .expect("source client runs");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn run(root: &Path, program: &str, args: &[&str]) -> (bool, String) {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .env("GOTOOLCHAIN", "local")
        .env("GOFLAGS", "-mod=readonly")
        .output()
        .unwrap_or_else(|error| panic!("{program} must run: {error}"));
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn behaviour(project: &Path) -> (String, String) {
    let (ran, program) = run(project, "go", &["run", "."]);
    assert!(ran, "go run: {program}");
    let (tested, tests) = run(project, "go", &["test", "-count=1", "./..."]);
    assert!(tested, "go test: {tests}");
    let tests = tests
        .lines()
        .map(|line| if line.starts_with("ok ") { "ok" } else { line })
        .collect::<Vec<_>>()
        .join("\n");
    (program, tests)
}

async fn tool(
    addr: SocketAddr,
    root: &Path,
    name: &str,
    args: serde_json::Value,
) -> (bool, String) {
    match prod_code_mcp::tools::execute_tool(addr, root, name, args).await {
        Ok(result) => {
            let text = result
                .content
                .iter()
                .map(|item| {
                    let McpContentItem::Text { text } = item;
                    text.as_str()
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!result.is_error, text)
        }
        Err(error) => (false, format!("{error:#}")),
    }
}

async fn wait_for_gopls(addr: SocketAddr, root: &Path, needle: &str, expected: &str) {
    let source = std::fs::read_to_string(root.join("project/main.go")).expect("fixture source");
    let offset = source.find(needle).expect("declaration needle");
    let before = &source[..offset];
    let args = serde_json::json!({
        "path": "project/main.go",
        "line": before.matches('\n').count() as u32 + 1,
        "character": before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .encode_utf16()
            .count() as u32 + 1
    });
    let mut answer = String::new();
    for _ in 0..60 {
        let (_, text) = tool(addr, root, "code_hover", args.clone()).await;
        answer = text;
        if answer.contains(expected) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("gopls did not load the fixture: {answer}");
}

const SOURCE: &str = "package main\n\ntype Mystruct struct{ value string }\n\nfunc unused() Mystruct { return Mystruct{value: `unused } {`} }\n\nfunc keep(value string) string { return value + `!` }\n\nfunc main() { println(keep(`same`)) }\n";
const TEST: &str = "package main\n\nimport `testing`\n\nfunc TestKeep(t *testing.T) {\n\tif keep(`x`) != `x!` { t.Fatal(`keep`) }\n}\n";
const COMPILE_FAIL_SOURCE: &str = "package main\n\nimport `strings`\n\nfunc unused() string { return strings.TrimSpace(` x `) }\n\nfunc keep(value string) string { return value + `!` }\n\nfunc main() {}\n";
const METHOD_VALUE_SOURCE: &str = "package main\n\ntype item struct{ value string }\n\nfunc (value item) unusedValue() string { return value.value }\n\nfunc main() {}\n";
const METHOD_POINTER_SOURCE: &str = "package main\n\ntype item struct{ value string }\n\nfunc (value *item) unusedPointer() string { return value.value }\n\nfunc main() {}\n";
const METHOD_USES_SOURCE: &str = "package main\n\ntype item struct{}\n\nfunc (value item) direct() {}\nfunc (value item) recursive() { value.recursive() }\nfunc (value item) methodValue() {}\nfunc (value *item) methodExpression() {}\n\nfunc uses(value item) {\n\tvalue.direct()\n\t_ = value.methodValue\n\t_ = (*item).methodExpression\n}\nfunc main() {}\n";
const METHOD_INTERFACES_SOURCE: &str = "package main\n\ntype item struct{}\n\nfunc (value item) localContract() {}\nfunc (value item) asserted() {}\n\ntype local interface { localContract() }\nvar _ local = item{}\nfunc runtime(value any) { _, _ = value.(interface { asserted() }) }\nfunc main() {}\n";
const EMPTY_TEST: &str = "package main\n";

fn tool_position(source: &str, needle: &str) -> (u32, u32) {
    let offset = source.find(needle).expect("tool position needle");
    let before = &source[..offset];
    (
        before.matches('\n').count() as u32 + 1,
        before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .encode_utf16()
            .count() as u32
            + 1,
    )
}

async fn sentinel_child() {
    let root = PathBuf::from(std::env::var_os(SENTINEL_ROOT).expect("sentinel root"));
    let addr = std::env::var(SENTINEL_ADDR)
        .expect("sentinel address")
        .parse()
        .expect("sentinel address parses");
    let (ok, output) = cli(&root, addr, &["safe-delete", "project/main.go", "5", "6"]);
    assert!(ok && output.contains("compiler-verified"), "{output}");
    let written = std::fs::read_to_string(root.join("project/main.go")).expect("written source");
    assert!(!written.contains("func unused"), "{written}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn go_function_safe_delete_has_real_gateway_cli_and_mcp_proof() {
    if std::env::var_os(SENTINEL_CHILD).is_some() {
        sentinel_child().await;
        return;
    }
    let (gopls_ok, version) = run(Path::new("."), "gopls", &["version"]);
    assert!(gopls_ok, "gopls version: {version}");
    let gateway = Gateway::start();

    let (_cli_dir, cli_root) = checkout(SOURCE, TEST);
    let cli_project = cli_root.join("project");
    let before = behaviour(&cli_project);
    wait_for_gopls(gateway.addr, &cli_root, "unused", "func unused").await;

    let fake_bin = tempfile::tempdir().expect("fake client bin");
    let fake_go = fake_bin.path().join("go");
    std::fs::write(&fake_go, "#!/bin/sh\nexit 99\n").expect("fake go");
    let mut permissions = std::fs::metadata(&fake_go).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_go, permissions).expect("fake go executable");
    let path = std::env::join_paths(std::iter::once(fake_bin.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("sentinel PATH");
    let mut sentinel = Command::new(std::env::current_exe().expect("test executable"));
    sentinel
        .args([
            "--exact",
            "go_function_safe_delete_has_real_gateway_cli_and_mcp_proof",
            "--nocapture",
        ])
        .env(SENTINEL_CHILD, "1")
        .env(SENTINEL_ROOT, &cli_root)
        .env(SENTINEL_ADDR, gateway.addr.to_string())
        .env("PATH", path)
        .process_group(0);
    let mut sentinel = sentinel.spawn().expect("sentinel child starts");
    let status = wait_for_exit(&mut sentinel, Duration::from_secs(120))
        .expect("sentinel child can be inspected");
    if status.is_none() {
        retire_owned_group(&mut sentinel, None, Duration::from_secs(2));
        panic!("sentinel child did not exit within 120 seconds");
    }
    retire_owned_group(&mut sentinel, None, Duration::ZERO);
    let status = status.expect("checked sentinel status");
    assert!(status.success(), "sentinel child: {status}");
    assert_eq!(
        behaviour(&cli_project),
        before,
        "CLI deletion changed behavior"
    );

    let (_mcp_dir, mcp_root) = checkout(SOURCE, TEST);
    let mcp_project = mcp_root.join("project");
    let before = behaviour(&mcp_project);
    wait_for_gopls(gateway.addr, &mcp_root, "unused", "func unused").await;
    let (ok, output) = tool(
        gateway.addr,
        &mcp_root,
        "code_safe_delete",
        serde_json::json!({
            "path": "project/main.go",
            "line": 5,
            "character": 6
        }),
    )
    .await;
    assert!(ok && output.contains("compiler-verified"), "{output}");
    assert_eq!(
        behaviour(&mcp_project),
        before,
        "MCP deletion changed behavior"
    );

    let (_value_dir, value_root) = checkout(METHOD_VALUE_SOURCE, EMPTY_TEST);
    let value_project = value_root.join("project");
    let before = behaviour(&value_project);
    wait_for_gopls(
        gateway.addr,
        &value_root,
        "unusedValue",
        "func (value item) unusedValue",
    )
    .await;
    let (ok, output) = cli(
        &value_root,
        gateway.addr,
        &["safe-delete", "project/main.go", "5", "19"],
    );
    assert!(ok && output.contains("compiler-verified"), "{output}");
    assert!(!std::fs::read_to_string(value_project.join("main.go")).unwrap().contains("unusedValue"));
    assert_eq!(behaviour(&value_project), before, "value-method CLI deletion changed behavior");

    let (_pointer_dir, pointer_root) = checkout(METHOD_POINTER_SOURCE, EMPTY_TEST);
    let pointer_project = pointer_root.join("project");
    let before = behaviour(&pointer_project);
    wait_for_gopls(
        gateway.addr,
        &pointer_root,
        "unusedPointer",
        "func (value *item) unusedPointer",
    )
    .await;
    let (line, character) = tool_position(METHOD_POINTER_SOURCE, "unusedPointer");
    let (ok, output) = tool(
        gateway.addr,
        &pointer_root,
        "code_safe_delete",
        serde_json::json!({
            "path": "project/main.go",
            "line": line,
            "character": character
        }),
    )
    .await;
    assert!(ok && output.contains("compiler-verified"), "{output}");
    assert!(!std::fs::read_to_string(pointer_project.join("main.go")).unwrap().contains("unusedPointer"));
    assert_eq!(behaviour(&pointer_project), before, "pointer-method MCP deletion changed behavior");

    let (_uses_dir, uses_root) = checkout(METHOD_USES_SOURCE, EMPTY_TEST);
    wait_for_gopls(gateway.addr, &uses_root, "direct", "func (value item) direct").await;
    for method in ["direct", "recursive", "methodValue", "methodExpression"] {
        let untouched = snapshot(&uses_root);
        let (line, character) = tool_position(METHOD_USES_SOURCE, method);
        let (ok, output) = tool(
            gateway.addr,
            &uses_root,
            "code_safe_delete",
            serde_json::json!({
                "path": "project/main.go",
                "line": line,
                "character": character,
                "force": true
            }),
        )
        .await;
        assert!(!ok && output.contains("still referenced"), "{method}: {output}");
        assert_eq!(snapshot(&uses_root), untouched, "{method} refusal wrote");
    }

    let (_interfaces_dir, interfaces_root) = checkout(METHOD_INTERFACES_SOURCE, EMPTY_TEST);
    wait_for_gopls(
        gateway.addr,
        &interfaces_root,
        "localContract",
        "func (value item) localContract",
    )
    .await;
    for method in ["localContract", "asserted"] {
        let untouched = snapshot(&interfaces_root);
        let (line, character) = tool_position(METHOD_INTERFACES_SOURCE, method);
        let (ok, output) = tool(
            gateway.addr,
            &interfaces_root,
            "code_safe_delete",
            serde_json::json!({
                "path": "project/main.go",
                "line": line,
                "character": character,
                "force": true
            }),
        )
        .await;
        assert!(
            !ok && (output.contains("implementation evidence")
                || output.contains("local interface obligation")),
            "{method}: {output}"
        );
        assert_eq!(snapshot(&interfaces_root), untouched, "{method} refusal wrote");
    }

    let (_failure_dir, failure_root) = checkout(COMPILE_FAIL_SOURCE, TEST);
    let (baseline_compiles, baseline_output) = run(
        &failure_root.join("project"),
        "go",
        &["test", "-count=1", "./..."],
    );
    assert!(
        baseline_compiles,
        "refusal fixture must compile before deletion: {baseline_output}"
    );
    wait_for_gopls(gateway.addr, &failure_root, "unused", "func unused").await;
    let untouched = snapshot(&failure_root);
    let (ok, output) = cli(
        &failure_root,
        gateway.addr,
        &["safe-delete", "project/main.go", "5", "6"],
    );
    assert!(!ok && output.contains("does not compile"), "{output}");
    assert!(
        output.contains("imported and not used"),
        "compiler diagnostic missing: {output}"
    );
    assert_eq!(snapshot(&failure_root), untouched, "compiler refusal wrote");
}

#[test]
fn gateway_cleanup_retires_term_ignoring_owned_group_members() {
    if std::env::var_os(CLEANUP_CHILD).is_some() {
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
        let ready = std::env::var_os(CLEANUP_READY).expect("cleanup ready path");
        std::fs::write(ready, std::process::id().to_string()).expect("cleanup child ready");
        loop {
            std::thread::park_timeout(Duration::from_secs(60));
        }
    }

    let gateway = Gateway::start();
    let group = gateway.child.id() as i32;
    let state = tempfile::tempdir().expect("cleanup state");
    let ready = state.path().join("ready");
    let mut child = Command::new(std::env::current_exe().expect("test executable"));
    child
        .args([
            "--exact",
            "gateway_cleanup_retires_term_ignoring_owned_group_members",
            "--nocapture",
        ])
        .env(CLEANUP_CHILD, "1")
        .env(CLEANUP_READY, &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(group);
    let mut child = child
        .spawn()
        .expect("cleanup child starts in gateway group");
    let ready_deadline = Instant::now() + Duration::from_secs(10);
    while !ready.is_file() && Instant::now() < ready_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if !ready.is_file() {
        unsafe {
            libc::kill(child.id() as i32, libc::SIGKILL);
        }
        let _ = child.wait();
        panic!("cleanup child did not become ready");
    }

    drop(gateway);
    let exit_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < exit_deadline {
        if child.try_wait().expect("inspect cleanup child").is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGKILL);
    }
    let _ = child.wait();
    panic!("gateway cleanup left a TERM-ignoring owned group member alive");
}

#[test]
fn gateway_startup_failure_reaps_its_owned_process() {
    let storage = tempfile::tempdir().expect("startup storage");
    let child = Command::new("python3")
        .args(["-c", "import os,time; os.close(1); time.sleep(60)"])
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("owned silent startup child");
    let pid = child.id() as i32;
    let result = std::panic::catch_unwind(|| Gateway::from_child(child, storage));
    let survived = unsafe { libc::kill(pid, 0) == 0 };
    // A broken constructor still gets an exact owned-PID cleanup before asserting RED.
    if survived {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
            libc::waitpid(pid, std::ptr::null_mut(), 0);
        }
    }
    assert!(result.is_err(), "silent startup must refuse");
    assert!(!survived, "startup failure leaked its owned process");
}
