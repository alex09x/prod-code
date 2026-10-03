#![cfg(unix)]

use prod_code_engine_rust::proc_macro_farm::{
    prepare_sandboxed_srv, ProcMacroWorkerFarm,
};
use prod_code_engine_rust::{ProdCodeConfig, ProcMacroServerKind, RustEngine};
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn test_proc_macro_farm_shared_concurrency_and_permit_lifecycle() {
    let farm = Arc::new(ProcMacroWorkerFarm::new(10));
    assert_eq!(farm.capacity(), 10);
    assert_eq!(farm.active_workers(), 0);
    assert_eq!(farm.active_workspaces(), 0);

    let ws1 = PathBuf::from("/tmp/repo1");
    let ws2 = PathBuf::from("/tmp/repo2");
    let ws3 = PathBuf::from("/tmp/repo3");

    // Workspace 1 requests 4 workers
    let (w1, permit1) = farm.allocate_workers(&ws1, 4);
    assert_eq!(w1, 4);
    assert_eq!(permit1.worker_count(), 4);
    assert_eq!(farm.active_workers(), 4);
    assert_eq!(farm.active_workspaces(), 1);

    // Workspace 2 requests 5 workers
    let (w2, permit2) = farm.allocate_workers(&ws2, 5);
    assert_eq!(w2, 5);
    assert_eq!(permit2.worker_count(), 5);
    assert_eq!(farm.active_workers(), 9);
    assert_eq!(farm.active_workspaces(), 2);

    // Workspace 3 requests 4 workers, but only 1 remains in capacity (10 - 9 = 1)
    let (w3, permit3) = farm.allocate_workers(&ws3, 4);
    assert_eq!(w3, 1, "Must be bounded by remaining farm capacity");
    assert_eq!(permit3.worker_count(), 1);
    assert_eq!(farm.active_workers(), 10);
    assert_eq!(farm.active_workspaces(), 3);

    // When Workspace 1 finishes / is evicted, its 4 workers are returned
    drop(permit1);
    assert_eq!(farm.active_workers(), 6);
    assert_eq!(farm.active_workspaces(), 2);

    // Workspace 4 can now allocate workers freed by Workspace 1
    let ws4 = PathBuf::from("/tmp/repo4");
    let (w4, permit4) = farm.allocate_workers(&ws4, 3);
    assert_eq!(w4, 3);
    assert_eq!(farm.active_workers(), 9);
    assert_eq!(farm.active_workspaces(), 3);

    drop(permit2);
    drop(permit3);
    drop(permit4);
    assert_eq!(farm.active_workers(), 0);
    assert_eq!(farm.active_workspaces(), 0);
}

#[test]
fn test_proc_macro_sandboxed_wrapper_execution_and_secret_scrubbing() {
    let temp = tempfile::tempdir().unwrap();
    let mock_proc_macro_srv = temp.path().join("mock-proc-macro-srv.sh");
    std::fs::write(
        &mock_proc_macro_srv,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "rust-analyzer-proc-macro-srv 1.95.0-mock"
    exit 0
fi

echo "TOKEN='$PROD_CODE_TOKEN'"
echo "SECRET='$PROD_CODE_SECRET'"
echo "TLS_KEY='$PROD_CODE_TLS_KEY'"
echo "AWS='$AWS_SECRET_ACCESS_KEY'"
echo "GITHUB='$GITHUB_TOKEN'"
echo "SSH='$SSH_AUTH_SOCK'"
echo "INTERNAL='$RUST_ANALYZER_INTERNALS_DO_NOT_USE'"
echo "TMPDIR='$TMPDIR'"
"#,
    )
    .unwrap();

    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&mock_proc_macro_srv).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&mock_proc_macro_srv, perms).unwrap();

    let wrapper_path = prepare_sandboxed_srv(&mock_proc_macro_srv, 2048).unwrap();
    assert!(wrapper_path.exists());

    // 1. Verify --version returns expected version string
    let out = std::process::Command::new(&wrapper_path)
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("rust-analyzer-proc-macro-srv 1.95.0-mock"));

    // 2. Verify sensitive credentials are scrubbed from environment
    let out = std::process::Command::new(&wrapper_path)
        .env("PROD_CODE_TOKEN", "prod-token-12345")
        .env("PROD_CODE_SECRET", "super-secret-hex")
        .env("PROD_CODE_TLS_KEY", "mTLS-private-key-material")
        .env("AWS_SECRET_ACCESS_KEY", "aws-secret-access-key")
        .env("GITHUB_TOKEN", "ghp_1234567890abcdef")
        .env("SSH_AUTH_SOCK", "/tmp/ssh-agent.sock")
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("TOKEN=''"), "PROD_CODE_TOKEN must be scrubbed: {stdout}");
    assert!(stdout.contains("SECRET=''"), "PROD_CODE_SECRET must be scrubbed: {stdout}");
    assert!(stdout.contains("TLS_KEY=''"), "PROD_CODE_TLS_KEY must be scrubbed: {stdout}");
    assert!(stdout.contains("AWS=''"), "AWS_SECRET_ACCESS_KEY must be scrubbed: {stdout}");
    assert!(stdout.contains("GITHUB=''"), "GITHUB_TOKEN must be scrubbed: {stdout}");
    assert!(stdout.contains("SSH=''"), "SSH_AUTH_SOCK must be scrubbed: {stdout}");
    assert!(stdout.contains("INTERNAL='this is unstable'"), "Internal authorization must be set: {stdout}");
    assert!(stdout.contains("prod-code-proc-macro-farm/scratch"), "Isolated scratch dir must be set: {stdout}");
}

#[test]
fn test_proc_macro_config_options_parsing() {
    let toml = r#"
[rust]
build_scripts = true
proc_macro_srv = "sandboxed"
proc_macro_workers = 4
proc_macro_memory_limit_mb = 1024
"#;
    let config: ProdCodeConfig = toml::from_str(toml).unwrap();
    assert!(config.rust.build_scripts);
    assert_eq!(config.rust.proc_macro_srv, ProcMacroServerKind::Sandboxed);
    assert_eq!(config.rust.proc_macro_workers, Some(4));
    assert_eq!(config.rust.proc_macro_memory_limit_mb, Some(1024));
}

#[test]
fn test_rust_engine_load_with_sandboxed_proc_macro_farm() {
    let temp = tempfile::tempdir().unwrap();
    let src_dir = temp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    std::fs::write(
        temp.path().join("Cargo.toml"),
        r#"[package]
name = "farm-fixture"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    std::fs::write(
        temp.path().join("prod-code.toml"),
        r#"[rust]
build_scripts = true
proc_macro_srv = "sandboxed"
proc_macro_workers = 2
proc_macro_memory_limit_mb = 2048
"#,
    )
    .unwrap();

    let lib_rs = src_dir.join("lib.rs");
    std::fs::write(
        &lib_rs,
        r#"#[derive(Debug, Clone, Default)]
pub struct WorkerItem {
    pub id: u64,
    pub name: String,
}

impl WorkerItem {
    pub fn new(id: u64, name: &str) -> Self {
        Self { id, name: name.to_string() }
    }
}
"#,
    )
    .unwrap();

    let engine = RustEngine::load(temp.path()).expect("Must load workspace with sandboxed farm");

    let metrics = RustEngine::proc_macro_farm_metrics();
    assert!(metrics.active_workers >= 1, "Farm must track active worker allocation");
    assert!(metrics.active_workspaces >= 1, "Farm must track active workspace");

    let syms = engine.document_symbols(&lib_rs).expect("Must get document symbols");
    assert!(syms.iter().any(|s| s.name == "WorkerItem"));
    assert!(syms.iter().any(|s| s.name == "new"));

    let hover = engine.hover(&lib_rs, 2, 12).expect("Hover query must succeed");
    assert!(hover.is_some(), "Must resolve hover for WorkerItem");

    drop(engine);
}
