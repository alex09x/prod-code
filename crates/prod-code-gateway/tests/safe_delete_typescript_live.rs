/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![cfg(unix)]

//! Native TypeScript LSP/compiler proof through the source-built gateway, CLI and MCP boundary.

use prod_code_mcp::protocol::McpContentItem;
use std::collections::BTreeMap;
use std::io::{Read, Seek};
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
    process: OwnedProcess,
    addr: SocketAddr,
    stdout: std::fs::File,
    _storage: tempfile::TempDir,
}

struct OwnedProcess {
    child: Child,
    group: i32,
    deadline: Instant,
    retired: bool,
}

impl OwnedProcess {
    fn new(child: Child, deadline: Instant) -> Self {
        let group = -(child.id() as i32);
        Self {
            child,
            group,
            deadline,
            retired: false,
        }
    }

    fn wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        wait_for_exit(&mut self.child, self.deadline)
    }

    fn retire(&mut self) {
        if self.retired {
            return;
        }
        self.retired = true;
        unsafe {
            libc::kill(self.group, libc::SIGTERM);
        }
        let term_deadline = self.deadline.min(Instant::now() + Duration::from_secs(2));
        let reaped = wait_for_exit(&mut self.child, term_deadline)
            .ok()
            .flatten()
            .is_some();
        unsafe {
            libc::kill(self.group, libc::SIGKILL);
        }
        if !reaped {
            let _ = wait_for_exit(&mut self.child, self.deadline);
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.retire();
    }
}

impl Gateway {
    fn start(deadline: Instant) -> Self {
        let storage = tempfile::Builder::new()
            .prefix("typescript-safe-delete-gateway")
            .tempdir()
            .expect("storage");
        let mut command = Command::new(env!("CARGO_BIN_EXE_prod-code-server"));
        let stdout = tempfile::tempfile().expect("gateway stdout file");
        command
            .env("PROD_CODE_STORAGE", storage.path())
            .env(
                "PROD_CODE_SHADOW_ROOT",
                storage.path().join("typescript-safe-delete-owned-shadow"),
            )
            .env("PROD_CODE_PEERS", "")
            .env("PROD_CODE_BIND", "127.0.0.1:0")
            .env_remove("RUST_LOG")
            .stdout(Stdio::from(
                stdout.try_clone().expect("clone gateway stdout file"),
            ))
            .stderr(Stdio::null())
            .process_group(0);
        let child = command.spawn().expect("gateway starts");
        let mut gateway = Self {
            process: OwnedProcess::new(child, deadline),
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            stdout,
            _storage: storage,
        };
        gateway.addr = bound_address(&mut gateway);
        gateway
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.process.retire();
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

fn bound_address(gateway: &mut Gateway) -> SocketAddr {
    let mut output = String::new();
    loop {
        gateway.stdout.rewind().expect("rewind gateway stdout");
        output.clear();
        gateway
            .stdout
            .read_to_string(&mut output)
            .expect("read gateway stdout");
        if let Some(addr) = output.lines().find_map(|line| {
            line.split("listening on ")
                .nth(1)
                .and_then(|value| value.trim().parse().ok())
        }) {
            return addr;
        }
        if gateway
            .process
            .child
            .try_wait()
            .expect("inspect gateway readiness")
            .is_some()
        {
            panic!("gateway exited before readiness: {output}");
        }
        if Instant::now() >= gateway.process.deadline {
            gateway.process.retire();
            panic!("gateway did not report readiness before deadline: {output}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_command(command: &mut Command, deadline: Instant, label: &str) -> (bool, String) {
    let mut stdout = tempfile::tempfile().expect("command stdout file");
    let mut stderr = tempfile::tempfile().expect("command stderr file");
    command
        .stdout(Stdio::from(stdout.try_clone().expect("clone stdout file")))
        .stderr(Stdio::from(stderr.try_clone().expect("clone stderr file")))
        .process_group(0);
    let child = command
        .spawn()
        .unwrap_or_else(|error| panic!("{label} spawn failed: {error}"));
    let mut process = OwnedProcess::new(child, deadline);
    let status = process
        .wait()
        .unwrap_or_else(|error| panic!("{label} wait failed: {error}"));
    if status.is_none() {
        process.retire();
        panic!("{label} timed out");
    }
    drop(process);
    let mut output = String::new();
    stdout.rewind().expect("rewind stdout");
    stderr.rewind().expect("rewind stderr");
    stdout.read_to_string(&mut output).expect("read stdout");
    stderr.read_to_string(&mut output).expect("read stderr");
    (status.expect("status").success(), output)
}

fn checkout(source: &str, deadline: Instant) -> (tempfile::TempDir, PathBuf) {
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
        let (ok, output) = run_command(
            Command::new("git").args(args).current_dir(dir.path()),
            deadline,
            "git fixture command",
        );
        assert!(ok, "git {args:?}: {output}");
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

fn source_cli(deadline: Instant) -> PathBuf {
    static CLI: OnceLock<PathBuf> = OnceLock::new();
    CLI.get_or_init(|| {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace")
            .to_path_buf();
        let (ok, output) = run_command(
            Command::new("cargo")
                .args(["build", "-p", "prod-code-client", "--bin", "prod-code"])
                .current_dir(&workspace),
            deadline,
            "source client build",
        );
        assert!(ok, "source client build: {output}");
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

fn cli(root: &Path, addr: SocketAddr, args: &[&str], deadline: Instant) -> (bool, String) {
    run_command(
        Command::new(source_cli(deadline))
            .args(["--remote", &addr.to_string()])
            .args(args)
            .current_dir(root),
        deadline,
        "source client",
    )
}

async fn tool(
    addr: SocketAddr,
    root: &Path,
    name: &str,
    args: serde_json::Value,
    deadline: Instant,
) -> (bool, String) {
    let request = tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline),
        prod_code_mcp::tools::execute_tool(addr, root, name, args),
    )
    .await
    .expect("tool request timed out");
    match request {
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

fn compile(root: &Path, deadline: Instant) -> (bool, String) {
    run_command(
        Command::new("tsc")
            .args([
                "--noEmit",
                "--pretty",
                "false",
                "--incremental",
                "false",
                "--project",
                "tsconfig.json",
            ])
            .current_dir(root),
        deadline,
        "installed TypeScript compiler",
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

async fn wait_for_typescript(addr: SocketAddr, root: &Path, source: &str, deadline: Instant) {
    let (line, character) = position(source, "hidden");
    let mut answer = String::new();
    while Instant::now() < deadline {
        let (_, text) = tool(
            addr,
            root,
            "code_hover",
            serde_json::json!({
                "path": "src/main.ts",
                "line": line,
                "character": character
            }),
            deadline,
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

async fn assert_declaration_only_reference(
    addr: SocketAddr,
    root: &Path,
    source: &str,
    deadline: Instant,
) {
    let file = std::fs::canonicalize(root.join("src/main.ts")).expect("source path");
    let (line, character) = position(source, "hidden");
    let uri = format!("file://{}", file.display());
    let answer = tokio::time::timeout_at(
        tokio::time::Instant::from_std(deadline),
        prod_code_mcp::tools::execute_lsp_query(
            addr,
            root,
            &file,
            "textDocument/references",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": line - 1, "character": character - 1 },
                "context": { "includeDeclaration": true }
            }),
        ),
    )
    .await
    .expect("native reference request timed out")
    .expect("native reference request");
    let locations = answer.as_array().expect("native reference list");
    assert_eq!(
        locations.len(),
        1,
        "references were not declaration-only: {answer}"
    );
    assert_eq!(
        locations[0].get("uri").and_then(serde_json::Value::as_str),
        Some(uri.as_str())
    );
    assert_eq!(
        locations[0]
            .pointer("/range/start/line")
            .and_then(serde_json::Value::as_u64),
        Some(u64::from(line - 1))
    );
    assert_eq!(
        locations[0]
            .pointer("/range/start/character")
            .and_then(serde_json::Value::as_u64),
        Some(u64::from(character - 1))
    );
}

async fn child_run() {
    let deadline = Instant::now() + Duration::from_secs(180);
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
        deadline,
    );
    let unchanged =
        std::fs::read_to_string(root.join("src/main.ts")).expect("child source") == UNUSED;
    assert!(
        ok && output.contains("compiler-verified") && !unchanged,
        "private TypeScript deletion was unsupported: ok={ok}, unchanged={unchanged}, output={output:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn typescript_safe_delete_has_native_gateway_cli_mcp_and_compiler_proof() {
    if std::env::var_os(CHILD).is_some() {
        child_run().await;
        return;
    }
    let deadline = Instant::now() + Duration::from_secs(900);
    let (version_ok, version_output) = run_command(
        Command::new("tsc").arg("--version"),
        deadline,
        "tsc --version",
    );
    assert!(version_ok, "tsc --version: {version_output}");
    let gateway = Gateway::start(deadline);

    let (_cli_dir, cli_root) = checkout(UNUSED, deadline);
    let (baseline, baseline_output) = compile(&cli_root, deadline);
    assert!(baseline, "baseline compile: {baseline_output}");
    wait_for_typescript(gateway.addr, &cli_root, UNUSED, deadline).await;
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
    let child = child.spawn().expect("isolated CLI child");
    let mut child = OwnedProcess::new(child, deadline);
    let status = child.wait().expect("inspect CLI child");
    if status.is_none() {
        child.retire();
        panic!("CLI child timed out");
    }
    child.retire();
    assert!(status.unwrap().success(), "CLI child failed");
    assert_eq!(snapshot(&cli_root), expected, "CLI exact fixture snapshot");
    let (compiled, output) = compile(&cli_root, deadline);
    assert!(compiled, "post-deletion compile: {output}");

    let (_used_dir, used_root) = checkout(USED, deadline);
    let (compiled, output) = compile(&used_root, deadline);
    assert!(compiled, "used baseline compile: {output}");
    wait_for_typescript(gateway.addr, &used_root, USED, deadline).await;
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
        deadline,
    )
    .await;
    assert!(!ok && output.contains("still referenced"), "{output}");
    assert_eq!(
        snapshot(&used_root),
        untouched,
        "force refusal changed bytes"
    );

    let exported = "export function hidden(): number { return 7; }\n";
    let (_export_dir, export_root) = checkout(exported, deadline);
    wait_for_typescript(gateway.addr, &export_root, exported, deadline).await;
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
        deadline,
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

    for source in [
        "export {};\nfunction hidden(): number { return 7; }\nexport const value = (eval)(\"hidden()\");\n",
        "export {};\nfunction hidden(): number { return 7; }\nexport const value = eval!(\"hidden()\");\n",
        "export {};\nfunction hidden(): number { return 7; }\nexport const value = \\u0065val(\"hidden()\");\n",
    ] {
        let (_dynamic_dir, dynamic_root) = checkout(source, deadline);
        let (compiled, output) = compile(&dynamic_root, deadline);
        assert!(compiled, "dynamic baseline compile: {output}");
        wait_for_typescript(gateway.addr, &dynamic_root, source, deadline).await;
        assert_declaration_only_reference(gateway.addr, &dynamic_root, source, deadline).await;
        let untouched = snapshot(&dynamic_root);
        let (line, character) = position(source, "hidden");
        let (ok, output) = tool(
            gateway.addr,
            &dynamic_root,
            "code_safe_delete",
            serde_json::json!({
                "path": "src/main.ts",
                "line": line,
                "character": character,
                "force": true
            }),
            deadline,
        )
        .await;
        assert!(
            !ok && (output.contains("dynamic") || output.contains("escaped identifiers")),
            "unsafe dynamic deletion was not refused: {output}"
        );
        assert_eq!(
            snapshot(&dynamic_root),
            untouched,
            "dynamic refusal changed bytes"
        );
    }

    let source = "export type Marker = number;\nfunction hidden(): number { return 7; }\n(globalThis as any)[\"output\"] = (globalThis as any)[\"hidden\"]();\n";
    let (_preserve_dir, preserve_root) = checkout(source, deadline);
    let config_path = preserve_root.join("tsconfig.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).expect("preserve config")).unwrap();
    config["compilerOptions"]["module"] = serde_json::json!("preserve");
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap())
        .expect("write preserve config");
    let (compiled, output) = compile(&preserve_root, deadline);
    assert!(compiled, "preserve baseline compile: {output}");
    wait_for_typescript(gateway.addr, &preserve_root, source, deadline).await;
    assert_declaration_only_reference(gateway.addr, &preserve_root, source, deadline).await;
    let untouched = snapshot(&preserve_root);
    let (line, character) = position(source, "hidden");
    let (ok, output) = tool(
        gateway.addr,
        &preserve_root,
        "code_safe_delete",
        serde_json::json!({
            "path": "src/main.ts",
            "line": line,
            "character": character,
            "force": true
        }),
        deadline,
    )
    .await;
    assert!(
        !ok && output.contains("module mode"),
        "unsafe preserve-mode deletion: {output}"
    );
    assert_eq!(
        snapshot(&preserve_root),
        untouched,
        "preserve refusal changed bytes"
    );
}
