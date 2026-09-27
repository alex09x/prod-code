#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::shadow::{
    HYPOTHESIS_DIR_PREFIX, OWNERSHIP_LOCK_FILE, default_root, overlay_unavailable,
};
use prod_code_protocol::{
    FileDelta, ProdCodeCodec, ShadowHypothesis, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use std::fs::File;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

const SERVER: &str = env!("CARGO_BIN_EXE_prod-code-server");
const START_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

struct Gateway {
    child: Child,
    addr: SocketAddr,
    log: PathBuf,
}

impl Gateway {
    async fn start(storage: &Path, shadow_root: Option<&Path>, fixture: &Path, tag: &str) -> Self {
        let mut gateway = Self::spawn(storage, shadow_root, fixture, tag);
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if status_query(gateway.addr).await.is_ok() {
                return gateway;
            }
            if let Some(status) = gateway.child.try_wait().expect("inspect gateway") {
                panic!(
                    "gateway {tag} exited before readiness with {status}: {}",
                    std::fs::read_to_string(&gateway.log).unwrap_or_default()
                );
            }
            assert!(Instant::now() < deadline, "gateway {tag} readiness timed out");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn spawn(storage: &Path, shadow_root: Option<&Path>, fixture: &Path, tag: &str) -> Self {
        let addr = unused_addr();
        let log = fixture.join(format!("{tag}.log"));
        let stderr = File::create(&log).expect("create gateway log");
        let mut command = Command::new(SERVER);
        command
            .arg("--bind")
            .arg(addr.to_string())
            .arg("--advertise")
            .arg(addr.to_string())
            .arg("--storage")
            .arg(storage)
            .arg("--idle-evict-secs")
            .arg("0")
            .arg("--prune-worktree-days")
            .arg("0")
            .arg("--prune-below-free-percent")
            .arg("0")
            .arg("--engines")
            .arg("rust")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        if let Some(root) = shadow_root {
            command.arg("--shadow-dir").arg(root);
        }
        for variable in [
            "PROD_CODE_BIND",
            "PROD_CODE_STORAGE",
            "PROD_CODE_SHADOW_DIR",
            "PROD_CODE_PEERS",
            "PROD_CODE_ADVERTISE",
            "PROD_CODE_AUTH_TOKEN",
            "PROD_CODE_AUTH_TOKEN_FILE",
            "PROD_CODE_ENGINES",
        ] {
            command.env_remove(variable);
        }
        command.process_group(0);
        let child = command.spawn().expect("spawn source-built private gateway");
        Self { child, addr, log }
    }

    async fn stop(&mut self) {
        if self.child.try_wait().expect("inspect gateway").is_none() {
            signal_group(&self.child, libc::SIGTERM);
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.child.try_wait().expect("inspect gateway").is_none()
                && Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        if self.child.try_wait().expect("inspect gateway").is_none() {
            signal_group(&self.child, libc::SIGKILL);
        }
        let status = self.child.wait().expect("reap gateway");
        assert!(status.success(), "gateway stopped with {status}");
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            signal_group(&self.child, libc::SIGKILL);
            let _ = self.child.wait();
        }
    }
}

fn signal_group(child: &Child, signal: i32) {
    let pid = i32::try_from(child.id()).expect("pid fits i32");
    // Every helper is placed in its own process group before spawn returns, so this signal is
    // bounded to the exact gateway and its tasks.
    unsafe {
        libc::kill(-pid, signal);
    }
}

fn unused_addr() -> SocketAddr {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("reserve test port");
    listener.local_addr().expect("test port address")
}

async fn status_query(addr: SocketAddr) -> Result<(), String> {
    let stream = tokio::time::timeout(Duration::from_millis(250), TcpStream::connect(addr))
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|err| err.to_string())?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::StatusRequest)
        .await
        .map_err(|err| err.to_string())?;
    let message = tokio::time::timeout(Duration::from_secs(2), framed.next())
        .await
        .map_err(|_| "status timed out".to_string())?
        .ok_or_else(|| "gateway closed before status".to_string())?
        .map_err(|err| err.to_string())?;
    match message {
        WireMessage::StatusResponse(_) => Ok(()),
        other => Err(format!("unexpected status response: {other:?}")),
    }
}

async fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_failure(mut gateway: Gateway) -> String {
    let deadline = Instant::now() + START_TIMEOUT;
    let status = loop {
        if let Some(status) = gateway.child.try_wait().expect("inspect refused gateway") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "second gateway did not refuse shared ownership"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(!status.success(), "shared-root gateway unexpectedly started");
    std::fs::read_to_string(&gateway.log).unwrap_or_default()
}

async fn shadow_request(
    addr: SocketAddr,
    ready: PathBuf,
    release: PathBuf,
) -> ShadowRunResponse {
    let stream = TcpStream::connect(addr).await.expect("connect for shadow run");
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::ShadowRunRequest(ShadowRunRequest {
            client_workspace_root: "/fixture/client/workspace".to_string(),
            base_workspace_name: Some("workspace".to_string()),
            hypotheses: vec![ShadowHypothesis {
                name: "waiting".to_string(),
                files: vec![FileDelta {
                    relative_path: "value.txt".to_string(),
                    content: Some(b"staged\n".to_vec()),
                    is_executable: false,
                }],
            }],
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf ready > \"$READY\"; while [ ! -f \"$RELEASE\" ]; do sleep 0.05; done; test \"$(cat value.txt)\" = staged; cat value.txt".to_string(),
            ],
            env: vec![
                ("READY".to_string(), ready.to_string_lossy().into_owned()),
                ("RELEASE".to_string(), release.to_string_lossy().into_owned()),
            ],
            timeout_secs: 20,
            subdir: None,
            parallel: 1,
            tail_bytes: 4096,
            client_agent: Some("issue591-test".to_string()),
            client_host: Some("private-fixture".to_string()),
        }))
        .await
        .expect("send shadow request");
    let message = tokio::time::timeout(REQUEST_TIMEOUT, framed.next())
        .await
        .expect("shadow response timeout")
        .expect("gateway closed before shadow response")
        .expect("decode shadow response");
    match message {
        WireMessage::ShadowRunResponse(response) => response,
        other => panic!("unexpected shadow response: {other:?}"),
    }
}

fn assert_shadow_succeeded(response: &ShadowRunResponse) {
    assert_eq!(response.mode, "overlay", "overlay coverage is required");
    assert_eq!(response.error, None);
    assert_eq!(response.results.len(), 1);
    let result = &response.results[0];
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert_eq!(result.error, None, "{result:?}");
    assert!(
        String::from_utf8_lossy(result.output_tail.as_deref().unwrap_or_default())
            .contains("staged")
    );
}

fn active_hypothesis(root: &Path) -> PathBuf {
    let entries: Vec<_> = std::fs::read_dir(root)
        .expect("read shadow root")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(HYPOTHESIS_DIR_PREFIX)
        })
        .map(|entry| entry.path())
        .collect();
    assert_eq!(entries.len(), 1, "one active hypothesis in {root:?}");
    entries[0].clone()
}

fn require_overlay() {
    if let Some(reason) = overlay_unavailable() {
        panic!("required overlay lifecycle coverage is unavailable: {reason}");
    }
}

#[test]
fn sibling_storage_roots_get_distinct_namespaces_and_aliases_do_not() {
    let fixture = TempDir::new().expect("fixture");
    let first = fixture.path().join("storage-a");
    let second = fixture.path().join("storage-b");
    std::fs::create_dir_all(&first).expect("first storage");
    std::fs::create_dir_all(&second).expect("second storage");
    let alias = fixture.path().join("storage-alias");
    std::os::unix::fs::symlink(&first, &alias).expect("storage alias");

    let first_root = default_root(&first);
    let second_root = default_root(&second);
    assert_ne!(first_root, second_root);
    assert_eq!(first_root.parent(), second_root.parent());
    assert_eq!(first_root, default_root(&alias));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sibling_default_gateways_cannot_sweep_each_others_active_hypotheses() {
    require_overlay();
    let fixture = TempDir::new().expect("fixture");
    let storage_a = fixture.path().join("storage-a");
    let storage_b = fixture.path().join("storage-b");
    let workspace_a = storage_a.join("workspace");
    let workspace_b = storage_b.join("workspace");
    std::fs::create_dir_all(&workspace_a).expect("workspace a");
    std::fs::create_dir_all(&workspace_b).expect("workspace b");
    std::fs::write(workspace_a.join("value.txt"), "original\n").expect("original a");
    std::fs::write(workspace_b.join("value.txt"), "other\n").expect("original b");

    let mut first = Gateway::start(&storage_a, None, fixture.path(), "default-first").await;
    let ready = fixture.path().join("default.ready");
    let release = fixture.path().join("default.release");
    let request = tokio::spawn(shadow_request(first.addr, ready.clone(), release.clone()));
    wait_for_path(&ready).await;
    let active = active_hypothesis(&default_root(&storage_a));
    assert_eq!(
        std::fs::read_to_string(active.join("upper/value.txt")).expect("staged value"),
        "staged\n"
    );

    let mut second = Gateway::start(&storage_b, None, fixture.path(), "default-second").await;
    status_query(first.addr)
        .await
        .expect("first gateway still answers while its hypothesis is active");
    std::fs::write(&release, "go\n").expect("release shadow");
    let response = request.await.expect("join shadow request");
    assert_shadow_succeeded(&response);
    assert_eq!(
        std::fs::read_to_string(workspace_a.join("value.txt")).expect("original value"),
        "original\n"
    );

    second.stop().await;
    first.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_root_owner_refuses_alias_then_releases_for_stale_cleanup() {
    require_overlay();
    let fixture = TempDir::new().expect("fixture");
    let storage_a = fixture.path().join("storage-a");
    let storage_b = fixture.path().join("storage-b");
    let workspace = storage_a.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&storage_b).expect("second storage");
    std::fs::write(workspace.join("value.txt"), "original\n").expect("original");
    let shared = fixture.path().join("explicit-shadow");

    let mut first = Gateway::start(
        &storage_a,
        Some(&shared),
        fixture.path(),
        "explicit-first",
    )
    .await;
    let lock = shared.join(OWNERSHIP_LOCK_FILE);
    let lock_inode = std::fs::metadata(&lock).expect("ownership lock").ino();
    let ready = fixture.path().join("explicit.ready");
    let release = fixture.path().join("explicit.release");
    let request = tokio::spawn(shadow_request(first.addr, ready.clone(), release.clone()));
    wait_for_path(&ready).await;
    let active = active_hypothesis(&shared);
    assert_eq!(
        std::fs::read_to_string(active.join("upper/value.txt")).expect("staged value"),
        "staged\n"
    );

    let alias = fixture.path().join("explicit-shadow-alias");
    std::os::unix::fs::symlink(&shared, &alias).expect("shadow-root alias");
    let refused = Gateway::spawn(
        &storage_b,
        Some(&alias),
        fixture.path(),
        "explicit-refused",
    );
    let error = wait_for_failure(refused).await;
    assert!(error.contains("already owned by another gateway"), "{error}");
    assert!(error.contains("--shadow-dir"), "{error}");
    assert_eq!(
        std::fs::read_to_string(active.join("upper/value.txt")).expect("undamaged staged value"),
        "staged\n"
    );
    status_query(first.addr)
        .await
        .expect("first gateway still answers after refusal");

    std::fs::write(&release, "go\n").expect("release shadow");
    let response = request.await.expect("join shadow request");
    assert_shadow_succeeded(&response);
    assert_eq!(
        std::fs::read_to_string(workspace.join("value.txt")).expect("original value"),
        "original\n"
    );
    first.stop().await;

    let stale = shared.join(format!("{HYPOTHESIS_DIR_PREFIX}abandoned"));
    std::fs::create_dir_all(stale.join("upper")).expect("stale hypothesis");
    std::fs::write(stale.join("upper/output"), "stale\n").expect("stale output");
    let unrelated = shared.join("unrelated");
    std::fs::create_dir_all(&unrelated).expect("unrelated entry");
    let mut later = Gateway::start(
        &storage_b,
        Some(&shared),
        fixture.path(),
        "explicit-later",
    )
    .await;
    assert!(!stale.exists(), "abandoned hypothesis was swept");
    assert!(unrelated.exists(), "unrelated root entry is preserved");
    assert_eq!(
        std::fs::metadata(&lock).expect("stable ownership lock").ino(),
        lock_inode,
        "startup must keep the same lock inode"
    );
    later.stop().await;
}
