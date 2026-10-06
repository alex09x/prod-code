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

use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::shadow::{
    HYPOTHESIS_DIR_PREFIX, OWNERSHIP_LOCK_FILE, default_root, overlay_unavailable,
};
use prod_code_protocol::{
    FileDelta, ProdCodeCodec, ShadowHypothesis, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use std::fs::File;
use std::future::Future;
use std::io;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::codec::Framed;

const SERVER: &str = env!("CARGO_BIN_EXE_prod-code-server");
const START_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

struct OwnedProcessGroup {
    id: i32,
    retired: bool,
}

impl OwnedProcessGroup {
    fn for_child(child: &Child) -> Self {
        Self {
            id: i32::try_from(child.id()).expect("pid fits i32"),
            retired: false,
        }
    }

    fn signal(&self, signal: i32) -> io::Result<()> {
        // Every helper enters a new group before spawn returns, so a negative id is bounded to
        // exactly the gateway (or controlled fixture parent) and its descendants.
        let result = unsafe { libc::kill(-self.id, signal) };
        if result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }

    fn retire(&mut self) -> io::Result<()> {
        if self.retired {
            return Ok(());
        }
        self.signal(libc::SIGKILL)?;
        self.retired = true;
        Ok(())
    }
}

struct Gateway {
    child: Child,
    group: OwnedProcessGroup,
    addr: SocketAddr,
    log: PathBuf,
}

impl Gateway {
    async fn start(storage: &Path, shadow_root: Option<&Path>, fixture: &Path, tag: &str) -> Self {
        let mut gateway = Self::spawn(storage, shadow_root, fixture, tag);
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if status_query(gateway.addr, deadline).await.is_ok() {
                return gateway;
            }
            if let Some(status) = gateway.child.try_wait().expect("inspect gateway") {
                panic!(
                    "gateway {tag} exited before readiness with {status}: {}",
                    std::fs::read_to_string(&gateway.log).unwrap_or_default()
                );
            }
            assert!(
                Instant::now() < deadline,
                "gateway {tag} readiness timed out"
            );
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
        let group = OwnedProcessGroup::for_child(&child);
        Self {
            child,
            group,
            addr,
            log,
        }
    }

    async fn stop(&mut self) {
        let mut status = self.child.try_wait().expect("inspect gateway");
        if status.is_none() {
            self.group
                .signal(libc::SIGTERM)
                .expect("signal owned gateway process group");
            let deadline = Instant::now() + Duration::from_secs(10);
            while status.is_none() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(25)).await;
                status = self.child.try_wait().expect("inspect gateway");
            }
        }

        // A successful leader can leave descendants behind. Retire the exact group once,
        // independently of leader status, before the final wait.
        self.group
            .retire()
            .expect("force-cleanup owned gateway process group");
        let status = match status {
            Some(status) => status,
            None => self.child.wait().expect("reap gateway"),
        };
        assert!(status.success(), "gateway stopped with {status}");
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let cleanup = self.group.retire();
        let _ = self.child.wait();
        if let Err(error) = cleanup {
            if std::thread::panicking() {
                eprintln!("failed to force-clean owned gateway process group: {error}");
            } else {
                panic!("failed to force-clean owned gateway process group: {error}");
            }
        }
    }
}

struct AbortOnDrop<T> {
    task: Option<JoinHandle<T>>,
}

impl<T> AbortOnDrop<T> {
    fn new(task: JoinHandle<T>) -> Self {
        Self { task: Some(task) }
    }

    async fn complete(&mut self, deadline: Instant) -> T {
        let joined = before(
            deadline,
            self.task.as_mut().expect("request task"),
            "join request",
        )
        .await
        .expect("request task panicked");
        self.task.take();
        joined
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn before<T>(deadline: Instant, future: impl Future<Output = T>, operation: &str) -> T {
    tokio::time::timeout_at(deadline.into(), future)
        .await
        .unwrap_or_else(|_| panic!("{operation} timed out"))
}

fn unused_addr() -> SocketAddr {
    let listener = StdTcpListener::bind("127.0.0.1:0").expect("reserve test port");
    listener.local_addr().expect("test port address")
}

async fn status_query(addr: SocketAddr, deadline: Instant) -> Result<(), String> {
    tokio::time::timeout_at(deadline.into(), async {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|err| err.to_string())?;
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        framed
            .send(WireMessage::StatusRequest)
            .await
            .map_err(|err| err.to_string())?;
        let message = framed
            .next()
            .await
            .ok_or_else(|| "gateway closed before status".to_string())?
            .map_err(|err| err.to_string())?;
        match message {
            WireMessage::StatusResponse(_) => Ok(()),
            other => Err(format!("unexpected status response: {other:?}")),
        }
    })
    .await
    .map_err(|_| "status flow timed out".to_string())?
}

async fn wait_for_path(path: &Path, deadline: Instant) {
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
    assert!(
        !status.success(),
        "shared-root gateway unexpectedly started"
    );
    std::fs::read_to_string(&gateway.log).unwrap_or_default()
}

async fn shadow_request(
    addr: SocketAddr,
    ready: PathBuf,
    release: PathBuf,
    deadline: Instant,
) -> ShadowRunResponse {
    before(
        deadline,
        async {
            let stream = TcpStream::connect(addr)
                .await
                .expect("connect for shadow run");
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
                    in_memory: false,
                    client_agent: Some("issue591-test".to_string()),
                    client_host: Some("private-fixture".to_string()),
                }))
                .await
                .expect("send shadow request");
            let message = framed
                .next()
                .await
                .expect("gateway closed before shadow response")
                .expect("decode shadow response");
            match message {
                WireMessage::ShadowRunResponse(response) => response,
                other => panic!("unexpected shadow response: {other:?}"),
            }
        },
        "shadow request flow",
    )
    .await
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
async fn gateway_drop_retires_descendants_after_the_leader_exits() {
    let fixture = TempDir::new().expect("fixture");
    let ready = fixture.path().join("held-child.ready");
    let child_pid = fixture.path().join("held-child.pid");
    let mut command = Command::new("sh");
    command
        .args([
            "-c",
            r#"sh -c 'trap "" TERM; printf ready > "$READY"; while :; do sleep 1; done' & echo $! > "$CHILD_PID""#,
        ])
        .env("READY", &ready)
        .env("CHILD_PID", &child_pid)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.process_group(0);
    let child = command.spawn().expect("spawn controlled process group");
    let group = OwnedProcessGroup::for_child(&child);
    let mut gateway = Gateway {
        child,
        group,
        addr: unused_addr(),
        log: fixture.path().join("held-child.log"),
    };
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    wait_for_path(&ready, deadline).await;
    before(
        deadline,
        async {
            loop {
                if gateway
                    .child
                    .try_wait()
                    .expect("inspect exited parent")
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        },
        "fixture parent exit",
    )
    .await;
    let pid: i32 = std::fs::read_to_string(&child_pid)
        .expect("child pid")
        .trim()
        .parse()
        .expect("numeric child pid");
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "held child must be alive");

    drop(gateway);

    before(
        deadline,
        async {
            loop {
                let exists = unsafe { libc::kill(pid, 0) } == 0
                    || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
                if !exists {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        },
        "held child cleanup",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_shadow_request_cleans_its_active_hypothesis() {
    require_overlay();
    let fixture = TempDir::new().expect("fixture");
    let storage = fixture.path().join("storage");
    let workspace = storage.join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("value.txt"), "original\n").expect("original");

    let mut gateway = Gateway::start(&storage, None, fixture.path(), "cancel").await;
    let ready = fixture.path().join("cancel.ready");
    let release = fixture.path().join("cancel.release");
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let request = AbortOnDrop::new(tokio::spawn(shadow_request(
        gateway.addr,
        ready.clone(),
        release,
        deadline,
    )));
    wait_for_path(&ready, deadline).await;
    let active = active_hypothesis(&default_root(&storage));

    drop(request);

    before(
        deadline,
        async {
            while active.exists() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        },
        "cancelled shadow cleanup",
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(workspace.join("value.txt")).expect("original value"),
        "original\n"
    );
    gateway.stop().await;
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
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut request = AbortOnDrop::new(tokio::spawn(shadow_request(
        first.addr,
        ready.clone(),
        release.clone(),
        deadline,
    )));
    wait_for_path(&ready, deadline).await;
    let active = active_hypothesis(&default_root(&storage_a));
    assert_eq!(
        std::fs::read_to_string(active.join("upper/value.txt")).expect("staged value"),
        "staged\n"
    );

    let mut second = Gateway::start(&storage_b, None, fixture.path(), "default-second").await;
    status_query(first.addr, Instant::now() + REQUEST_TIMEOUT)
        .await
        .expect("first gateway still answers while its hypothesis is active");
    std::fs::write(&release, "go\n").expect("release shadow");
    let response = request.complete(deadline).await;
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

    let mut first =
        Gateway::start(&storage_a, Some(&shared), fixture.path(), "explicit-first").await;
    let lock = shared.join(OWNERSHIP_LOCK_FILE);
    let lock_inode = std::fs::metadata(&lock).expect("ownership lock").ino();
    let ready = fixture.path().join("explicit.ready");
    let release = fixture.path().join("explicit.release");
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut request = AbortOnDrop::new(tokio::spawn(shadow_request(
        first.addr,
        ready.clone(),
        release.clone(),
        deadline,
    )));
    wait_for_path(&ready, deadline).await;
    let active = active_hypothesis(&shared);
    assert_eq!(
        std::fs::read_to_string(active.join("upper/value.txt")).expect("staged value"),
        "staged\n"
    );

    let alias = fixture.path().join("explicit-shadow-alias");
    std::os::unix::fs::symlink(&shared, &alias).expect("shadow-root alias");
    let refused = Gateway::spawn(&storage_b, Some(&alias), fixture.path(), "explicit-refused");
    let error = wait_for_failure(refused).await;
    assert!(
        error.contains("already owned by another gateway"),
        "{error}"
    );
    assert!(error.contains("--shadow-dir"), "{error}");
    assert_eq!(
        std::fs::read_to_string(active.join("upper/value.txt")).expect("undamaged staged value"),
        "staged\n"
    );
    status_query(first.addr, Instant::now() + REQUEST_TIMEOUT)
        .await
        .expect("first gateway still answers after refusal");

    std::fs::write(&release, "go\n").expect("release shadow");
    let response = request.complete(deadline).await;
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
    let mut later =
        Gateway::start(&storage_b, Some(&shared), fixture.path(), "explicit-later").await;
    assert!(!stale.exists(), "abandoned hypothesis was swept");
    assert!(unrelated.exists(), "unrelated root entry is preserved");
    assert_eq!(
        std::fs::metadata(&lock)
            .expect("stable ownership lock")
            .ino(),
        lock_inode,
        "startup must keep the same lock inode"
    );
    later.stop().await;
}
