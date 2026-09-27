#![cfg(unix)]

//! Native TypeScript LSP/compiler proof through the source-built gateway, CLI and MCP boundary.

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

const CHILD: &str = "PROD_CODE_SAFE_DELETE_TYPESCRIPT_CHILD";
const CHILD_ROOT: &str = "PROD_CODE_SAFE_DELETE_TYPESCRIPT_ROOT";
const CHILD_ADDR: &str = "PROD_CODE_SAFE_DELETE_TYPESCRIPT_ADDR";

const CONFIG: &str = r#"{"compilerOptions":{"target":"ES2022","module":"ESNext","strict":true,"noEmit":true},"include":["src/**/*.ts"]}"#;
const UNUSED: &str = "export {};\r\n\r\nconst emoji = \"🙂\";\r\nfunction hidden(): number {\r\n  return 7;\r\n}\r\nvoid emoji;\r\n";
const USED: &str =
    "export {};\nfunction hidden(): number { return 7; }\nconst kept = hidden();\nvoid kept;\n";

struct Gateway {
    child: Child,
    addr: SocketAddr,
    reader: Option<std::thread::JoinHandle<()>>,
    _storage: tempfile::TempDir,
}

impl Gateway {
    fn start() -> Self {
        let storage = tempfile::Builder::new()
            .prefix("typescript-safe-delete-gateway")
            .tempdir()
            .expect("storage");
        let mut command = Command::new(env!("CARGO_BIN_EXE_prod-code-server"));
        command
            .env("PROD_CODE_STORAGE", storage.path())
            .env(
                "PROD_CODE_SHADOW_ROOT",
                storage.path().join("typescript-safe-delete-owned-shadow"),
            )
            .env("PROD_CODE_PEERS", "")
            .env("PROD_CODE_BIND", "127.0.0.1:0")
            .env_remove("RUST_LOG")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().expect("gateway starts");
        let mut gateway = Self {
            child,
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            reader: None,
            _storage: storage,
        };
        gateway.addr = bound_address(&mut gateway);
        gateway
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        retire_group(&mut self.child, self.reader.take(), Duration::from_secs(20));
    }
}

fn wait_for_exit(
    child: &mut Child,
    deadline: Instant,
) -> std::io::Result<Option<std::process::ExitStatus>> {
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

fn retire_group(child: &mut Child, reader: Option<std::thread::JoinHandle<()>>, grace: Duration) {
    let group = -(child.id() as i32);
    unsafe {
        libc::kill(group, libc::SIGTERM);
    }
    let reaped = wait_for_exit(child, Instant::now() + grace)
        .ok()
        .flatten()
        .is_some();
    unsafe {
        libc::kill(group, libc::SIGKILL);
    }
    if !reaped {
        let _ = child.wait();
    }
    if let Some(reader) = reader {
        let _ = reader.join();
    }
}

fn bound_address(gateway: &mut Gateway) -> SocketAddr {
    let stdout = gateway.child.stdout.take().expect("gateway stdout");
    let (send, receive) = std::sync::mpsc::channel();
    gateway.reader = Some(std::thread::spawn(move || {
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
    match receive.recv_timeout(Duration::from_secs(60)) {
        Ok(addr) => addr,
        Err(error) => {
            retire_group(
                &mut gateway.child,
                gateway.reader.take(),
                Duration::from_secs(2),
            );
            panic!("gateway did not report readiness: {error}");
        }
    }
}

fn checkout(source: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("typescript-safe-delete-fixture")
        .tempdir()
        .expect("fixture");
    for (relative, text) in [("tsconfig.json", CONFIG), ("src/main.ts", source)] {
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
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
                    format!("{}/", path.strip_prefix(root).unwrap().to_string_lossy()),
                    Vec::new(),
                );
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .unwrap()
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
            .expect("workspace")
            .to_path_buf();
        let status = Command::new("cargo")
            .args(["build", "-p", "prod-code-client", "--bin", "prod-code"])
            .current_dir(&workspace)
            .status()
            .expect("source client build");
        assert!(status.success(), "source client build: {status}");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join("target"));
        let target = if target.is_absolute() {
            target
        } else {
            workspace.join(target)
        };
        target
            .join("debug")
            .join(format!("prod-code{}", std::env::consts::EXE_SUFFIX))
    })
    .clone()
}

fn cli(root: &Path, addr: SocketAddr, args: &[&str]) -> (bool, String) {
    let output = Command::new(source_cli())
        .args(["--remote", &addr.to_string()])
        .args(args)
        .current_dir(root)
        .output()
        .expect("source client");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
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

fn compile(root: &Path) -> (bool, String) {
    let output = Command::new("tsc")
        .args([
            "--noEmit",
            "--pretty",
            "false",
            "--incremental",
            "false",
            "--project",
            "tsconfig.json",
        ])
        .current_dir(root)
        .output()
        .expect("installed TypeScript compiler");
    (
        output.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn position(source: &str, needle: &str) -> (u32, u32) {
    let offset = source.find(needle).expect("needle");
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

async fn wait_for_typescript(addr: SocketAddr, root: &Path, source: &str) {
    let (line, character) = position(source, "hidden");
    let mut answer = String::new();
    for _ in 0..60 {
        let (_, text) = tool(
            addr,
            root,
            "code_hover",
            serde_json::json!({
                "path": "src/main.ts",
                "line": line,
                "character": character
            }),
        )
        .await;
        answer = text;
        if answer.contains("hidden") {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("TypeScript language server did not load fixture: {answer}");
}

async fn child_run() {
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT).expect("child root"));
    let addr = std::env::var(CHILD_ADDR)
        .expect("child addr")
        .parse()
        .expect("address");
    let (line, character) = position(UNUSED, "hidden");
    let (ok, output) = cli(
        &root,
        addr,
        &[
            "safe-delete",
            "src/main.ts",
            &line.to_string(),
            &character.to_string(),
        ],
    );
    assert!(ok && output.contains("compiler-verified"), "{output}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typescript_safe_delete_has_native_gateway_cli_mcp_and_compiler_proof() {
    if std::env::var_os(CHILD).is_some() {
        child_run().await;
        return;
    }
    let version = Command::new("tsc")
        .arg("--version")
        .output()
        .expect("tsc must be installed on the build node");
    assert!(version.status.success(), "tsc --version");
    let gateway = Gateway::start();

    let (_cli_dir, cli_root) = checkout(UNUSED);
    let (baseline, baseline_output) = compile(&cli_root);
    assert!(baseline, "baseline compile: {baseline_output}");
    wait_for_typescript(gateway.addr, &cli_root, UNUSED).await;
    let before = snapshot(&cli_root);
    let mut expected = before.clone();
    expected.insert(
        "src/main.ts".into(),
        UNUSED
            .replace("function hidden(): number {\r\n  return 7;\r\n}", "")
            .into_bytes(),
    );

    let fake = tempfile::tempdir().expect("fake compiler directory");
    let fake_tsc = fake.path().join("tsc");
    std::fs::write(&fake_tsc, "#!/bin/sh\nexit 99\n").expect("fake tsc");
    let mut permissions = std::fs::metadata(&fake_tsc).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_tsc, permissions).unwrap();
    let path = std::env::join_paths(std::iter::once(fake.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("child PATH");
    let mut child = Command::new(std::env::current_exe().expect("test executable"));
    child
        .args([
            "--exact",
            "typescript_safe_delete_has_native_gateway_cli_mcp_and_compiler_proof",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env(CHILD_ROOT, &cli_root)
        .env(CHILD_ADDR, gateway.addr.to_string())
        .env("PATH", path)
        .process_group(0);
    let mut child = child.spawn().expect("isolated CLI child");
    let status = wait_for_exit(&mut child, Instant::now() + Duration::from_secs(180))
        .expect("inspect CLI child");
    if status.is_none() {
        retire_group(&mut child, None, Duration::from_secs(2));
        panic!("CLI child timed out");
    }
    retire_group(&mut child, None, Duration::ZERO);
    assert!(status.unwrap().success(), "CLI child failed");
    assert_eq!(snapshot(&cli_root), expected, "CLI exact fixture snapshot");
    let (compiled, output) = compile(&cli_root);
    assert!(compiled, "post-deletion compile: {output}");

    let (_used_dir, used_root) = checkout(USED);
    let (compiled, output) = compile(&used_root);
    assert!(compiled, "used baseline compile: {output}");
    wait_for_typescript(gateway.addr, &used_root, USED).await;
    let untouched = snapshot(&used_root);
    let (line, character) = position(USED, "hidden");
    let (ok, output) = tool(
        gateway.addr,
        &used_root,
        "code_safe_delete",
        serde_json::json!({
            "path": "src/main.ts",
            "line": line,
            "character": character,
            "force": true
        }),
    )
    .await;
    assert!(!ok && output.contains("still referenced"), "{output}");
    assert_eq!(
        snapshot(&used_root),
        untouched,
        "force refusal changed bytes"
    );

    let exported = "export function hidden(): number { return 7; }\n";
    let (_export_dir, export_root) = checkout(exported);
    wait_for_typescript(gateway.addr, &export_root, exported).await;
    let untouched = snapshot(&export_root);
    let (line, character) = position(exported, "hidden");
    let (ok, output) = tool(
        gateway.addr,
        &export_root,
        "code_safe_delete",
        serde_json::json!({
            "path": "src/main.ts",
            "line": line,
            "character": character,
            "force": true
        }),
    )
    .await;
    assert!(
        !ok && (output.contains("ordinary function") || output.contains("exported")),
        "{output}"
    );
    assert_eq!(
        snapshot(&export_root),
        untouched,
        "export refusal changed bytes"
    );
}
