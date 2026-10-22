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
