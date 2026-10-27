/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[tokio::test]
async fn a_silent_gateway_times_out_naming_the_node_and_the_next_step() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accept = tokio::spawn(async move { listener.accept().await.unwrap().0 });
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    // Accepted and never answered, like the stuck sync on the node in #1143.
    let _silent = accept.await.unwrap();
    let mut framed = Framed::new(AnyStream::Tcp(stream), ProdCodeCodec::new());
    let err =
        wait_for_message_within(
            &mut framed,
            "sync response",
            Duration::from_secs(1),
            |m| match m {
                WireMessage::SyncResponse(r) => Some(r),
                _ => None,
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("after 1s waiting for sync response"), "{err}");
    assert!(err.contains(&addr.to_string()), "{err}");
    assert!(err.contains("may already be on that node"), "{err}");
    assert!(err.contains("prod-code status"), "{err}");
}

/// A gateway that sends keepalives every 300 ms; with `answer_after`, it then answers.
async fn keepalive_gateway(answer_after: Option<Duration>) -> Framed<AnyStream, ProdCodeCodec> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut gateway = Framed::new(socket, ProdCodeCodec::new());
        let started = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if answer_after.is_some_and(|after| started.elapsed() >= after) {
                let _ = gateway
                    .send(WireMessage::SyncProbeResponse(
                        prod_code_protocol::SyncProbeResponse {
                            server_workspace_root: "/srv/ws".to_string(),
                            seeded: true,
                            files_deleted: 0,
                            missing: Vec::new(),
                        },
                    ))
                    .await;
                return;
            }
            if gateway.send(WireMessage::Pong).await.is_err() {
                return;
            }
        }
    });
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    Framed::new(AnyStream::Tcp(stream), ProdCodeCodec::new())
}

fn probe_response(m: WireMessage) -> Option<prod_code_protocol::SyncProbeResponse> {
    match m {
        WireMessage::SyncProbeResponse(r) => Some(r),
        _ => None,
    }
}

#[tokio::test]
async fn keepalives_extend_the_wait_past_the_idle_limit_up_to_the_cap() {
    let mut framed = keepalive_gateway(Some(Duration::from_millis(1500))).await;
    let probe = wait_for_message_capped(
        &mut framed,
        "sync probe response",
        Duration::from_secs(1),
        Duration::from_secs(10),
        probe_response,
    )
    .await
    .expect("keepalives keep the probe alive");
    assert!(probe.seeded);
    let mut framed = keepalive_gateway(None).await;
    let started = tokio::time::Instant::now();
    let err = wait_for_message_capped(
        &mut framed,
        "sync probe response",
        Duration::from_secs(1),
        Duration::from_secs(2),
        probe_response,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(started.elapsed() < Duration::from_secs(4), "{err}");
    assert!(err.contains("waiting for sync probe response"), "{err}");
}

/// A gateway that answers the first-contact probe asking for `missing`, then each sync request
/// with the stale set `stale(n, missing, sent)` gives for the `n`th request, recording what each one carried.
async fn stale_gateway(
    missing: Vec<String>,
    stale: fn(usize, &[String], &[String]) -> Vec<String>,
) -> (
    Framed<AnyStream, ProdCodeCodec>,
    std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut gateway = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(message)) = gateway.next().await {
            let answer = match message {
                WireMessage::SyncProbeRequest(_) => {
                    WireMessage::SyncProbeResponse(prod_code_protocol::SyncProbeResponse {
                        server_workspace_root: "/srv/ws".to_string(),
                        seeded: true,
                        files_deleted: 0,
                        missing: missing.clone(),
                    })
                }
                WireMessage::SyncRequest(req) => {
                    let sent: Vec<String> =
                        req.files.iter().map(|f| f.relative_path.clone()).collect();
                    let mut log = log.lock().unwrap();
                    log.push(sent.clone());
                    WireMessage::SyncResponse(prod_code_protocol::SyncResponse {
                        server_workspace_root: "/srv/ws".to_string(),
                        files_updated: req.files.len(),
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 1,
                        workspace_was_fresh: false,
                        stale_paths: stale(log.len() - 1, &missing, &sent),
                    })
                }
                _ => continue,
            };
            if gateway.send(answer).await.is_err() {
                return;
            }
        }
    });
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    (
        Framed::new(AnyStream::Tcp(stream), ProdCodeCodec::new()),
        seen,
    )
}

/// A committed git checkout of `files` in a fresh directory, with no sync cache yet.
fn checkout<const N: usize>(files: &[(&str, Vec<u8>); N]) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    crate::sync::cache::clear_sync_cache(root);
    for (path, bytes) in files {
        std::fs::write(root.join(path), bytes).unwrap();
    }
    for args in [
        &["init", "-q"][..],
        &["add", "."],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "initial",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .unwrap()
                .success()
        );
    }
    temp
}

fn identity() -> WorkspaceIdentity {
    WorkspaceIdentity {
        name: "ws".to_string(),
        base: None,
    }
}

/// #1475: each sync response carries the gateway's whole stale set. A file the first batch's
/// answer named stale and the second batch delivered is not stale any more, and is not sent a
/// third time.
#[tokio::test]
async fn a_file_a_later_batch_delivered_is_not_resent() {
    // Three files of 9 MiB: more than one 24 MiB batch, each under the 10 MiB file limit.
    let big = vec![b'x'; 9 << 20];
    let names = ["a.rs", "b.rs", "c.rs"];
    let temp = checkout(&names.map(|n| (n, big.clone())));
    let missing = names.map(str::to_string).to_vec();
    // The first answer names stale what is still to come; the last, nothing.
    let (mut framed, seen) = stale_gateway(missing, |n, missing, sent| match n {
        0 => missing
            .iter()
            .filter(|m| !sent.contains(m))
            .cloned()
            .collect(),
        _ => Vec::new(),
    })
    .await;
    let outcome = push_workspace_sync(&mut framed, temp.path(), &identity(), None)
        .await
        .unwrap();
    assert!(outcome.stale_paths.is_empty(), "{outcome:?}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "two batches and no resend round: {seen:?}");
    crate::sync::cache::clear_sync_cache_for(temp.path(), &gateway_node(&framed));
}

/// #1475: a file the gateway still could not write after it was sent again fails the sync,
/// naming it, so that no command runs against the node's old copy; the next sync sends it again.
#[tokio::test]
async fn a_file_the_gateway_cannot_write_fails_the_sync_and_is_sent_again() {
    let temp = checkout(&[("a.rs", b"pub fn a() {}".to_vec())]);
    let (mut framed, seen) =
        stale_gateway(vec!["a.rs".to_string()], |_, _, _| vec!["a.rs".to_string()]).await;
    let err = push_workspace_sync(&mut framed, temp.path(), &identity(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("a.rs"), "{err}");
    assert!(err.contains("could not write"), "{err}");
    assert_eq!(seen.lock().unwrap().len(), 2, "sent, then sent once more");
    let node = gateway_node(&framed);
    let next = crate::sync::plan::prepare_workspace_sync_for(temp.path(), &node, None).unwrap();
    assert!(
        next.files.iter().any(|f| f.relative_path == "a.rs"),
        "the next sync sends it again: {:?}",
        next.files
            .iter()
            .map(|f| &f.relative_path)
            .collect::<Vec<_>>()
    );
    crate::sync::cache::clear_sync_cache_for(temp.path(), &node);
}
