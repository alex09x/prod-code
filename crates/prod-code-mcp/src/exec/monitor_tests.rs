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

async fn pair() -> (
    Framed<AnyStream, ProdCodeCodec>,
    Framed<AnyStream, ProdCodeCodec>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    (
        Framed::new(AnyStream::from(client), ProdCodeCodec::new()),
        Framed::new(AnyStream::from(server), ProdCodeCodec::new()),
    )
}

#[tokio::test]
async fn exec_monitor_bounds_a_silent_gateway_and_sends_heartbeats() {
    let (mut client, mut server) = pair().await;
    let mut monitor = ExecMonitor::new(60);
    monitor.idle_deadline = Instant::now() + Duration::from_millis(120);
    monitor.reports = tokio::time::interval(Duration::from_millis(20));
    let mut output = Vec::new();
    let mut collect = |_, bytes: &[u8]| output.extend_from_slice(bytes);
    let wait = monitor.next(&mut client, &mut collect);
    let err = tokio::time::timeout(Duration::from_secs(2), wait)
        .await
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(err.contains("result is unknown"), "{err}");
    assert!(err.contains("prod-code status"), "{err}");
    assert!(String::from_utf8_lossy(&output).contains("waiting for command output or exit"));
    assert!(matches!(server.next().await, Some(Ok(WireMessage::Ping))));
}

#[tokio::test]
async fn exec_monitor_keepalives_do_not_extend_the_terminal_result_deadline() {
    let (mut client, mut server) = pair().await;
    let peer = tokio::spawn(async move {
        while let Some(Ok(WireMessage::Ping)) = server.next().await {
            if server.send(WireMessage::Pong).await.is_err() {
                break;
            }
        }
    });
    let mut monitor = ExecMonitor::new(1);
    monitor.deadline = Instant::now() + Duration::from_millis(120);
    monitor.reports = tokio::time::interval(Duration::from_millis(20));
    let outcome = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match monitor.next(&mut client, &mut |_, _| {}).await {
                Ok(Some(Ok(WireMessage::Pong))) => {}
                other => break other,
            }
        }
    })
    .await
    .unwrap();
    assert!(
        outcome
            .unwrap_err()
            .to_string()
            .contains("result is unknown")
    );
    drop(client);
    peer.await.unwrap();
}
