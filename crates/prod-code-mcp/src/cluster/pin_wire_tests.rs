/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A strict pin is confirmed by the gateway before anything else is sent (#1793).

use super::*;
use prod_code_protocol::NodePinResponse;
use std::sync::{Arc, Mutex as StdMutex};

/// What the fake gateway does with a pin request.
#[derive(Clone, Copy)]
enum Gateway {
    /// Confirms, naming itself.
    Current,
    /// Cannot decode the request and closes the connection, as a gateway older than pins does.
    Old,
    /// Refuses with a reason.
    Refusing,
    /// Never answers.
    Silent,
}

/// Starts `kind`; returns its address and every message it received.
async fn gateway(kind: Gateway) -> (SocketAddr, Arc<StdMutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        while let Some(Ok(message)) = framed.next().await {
            log.lock().unwrap().push(kind_of(&message));
            if !matches!(message, WireMessage::NodePinRequest(_)) {
                continue;
            }
            let answer = match kind {
                Gateway::Current => NodePinResponse {
                    accepted: true,
                    node: Some("build.example:9400".into()),
                    reason: None,
                },
                Gateway::Refusing => NodePinResponse {
                    accepted: false,
                    node: None,
                    reason: Some("draining".into()),
                },
                Gateway::Old => return,
                Gateway::Silent => {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    return;
                }
            };
            let _ = framed.send(WireMessage::NodePinResponse(answer)).await;
        }
    });
    (addr, seen)
}

fn kind_of(message: &WireMessage) -> String {
    kind(message)
}

async fn confirm(addr: SocketAddr, limit: Duration) -> anyhow::Result<String> {
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    confirm_pin_within(
        &mut framed,
        "build.example:9400",
        addr,
        Path::new("/work/app"),
        limit,
    )
    .await
}

#[tokio::test]
async fn issue_1793_a_current_gateway_confirms_and_names_the_executing_node() {
    let (addr, seen) = gateway(Gateway::Current).await;
    let node = confirm(addr, PIN_CONFIRM_TIMEOUT).await.unwrap();
    assert_eq!(node, "build.example:9400");
    assert_eq!(*seen.lock().unwrap(), ["NodePinRequest"]);
}

#[tokio::test]
async fn issue_1793_an_old_gateway_is_refused_before_any_command() {
    let (addr, seen) = gateway(Gateway::Old).await;
    let err = confirm(addr, PIN_CONFIRM_TIMEOUT)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("older than node pins"), "{err}");
    assert!(
        err.contains("no workspace sync or command was sent"),
        "{err}"
    );
    assert_eq!(*seen.lock().unwrap(), ["NodePinRequest"]);
}

#[tokio::test]
async fn issue_1793_a_refused_or_unanswered_pin_fails_closed() {
    let (addr, _) = gateway(Gateway::Refusing).await;
    let err = confirm(addr, PIN_CONFIRM_TIMEOUT)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("it refused (draining)"), "{err}");

    let (addr, _) = gateway(Gateway::Silent).await;
    let err = confirm(addr, Duration::from_millis(200))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no answer"), "{err}");
}
