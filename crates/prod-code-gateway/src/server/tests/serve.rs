/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A gateway that really starts: it accepts on TCP and on a socket path, answers, and stops
//! on the signal a supervisor sends. Each test runs alone in a child process (`isolated!`),
//! because delivering a signal and setting the environment are process-wide.

use super::*;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{AuthToken, ProdCodeCodec, WireMessage};
use serde_json::{Value, json};
use std::net::SocketAddr;
use tokio_util::codec::Framed;

fn configure_environment(token: Option<&str>) {
    for var in prod_code_protocol::tls::TLS_ENV_VARS {
        unsafe {
            std::env::remove_var(var);
        }
    }
    unsafe {
        std::env::set_var("PROD_CODE_TLS_MODE", "disabled");
        std::env::remove_var("PROD_CODE_AUTH_TOKEN_FILE");
        match token {
            Some(token) => std::env::set_var("PROD_CODE_AUTH_TOKEN", token),
            None => std::env::remove_var("PROD_CODE_AUTH_TOKEN"),
        }
    }
}

async fn ask<T>(client: &mut Framed<T, ProdCodeCodec>, message: WireMessage) -> WireMessage
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    client.send(message).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), client.next())
        .await
        .expect("an answer")
        .expect("the connection stays open")
        .expect("a frame")
}

/// A plain HTTP request, the way a load balancer's health check makes it: the gateway answers
/// in HTTP and closes.
async fn http_get<T>(mut stream: T, path: &str) -> (u16, Value)
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let request = format!("GET {path} HTTP/1.1\r\nHost: sample\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_string(&mut raw))
        .await
        .expect("the gateway answers")
        .unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").expect("a complete answer");
    assert!(head.contains("Content-Type: application/json"), "{head}");
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}

async fn tcp_stream(addr: SocketAddr) -> tokio::net::TcpStream {
    let loopback = SocketAddr::from(([127, 0, 0, 1], addr.port()));
    tokio::net::TcpStream::connect(loopback).await.unwrap()
}

async fn tcp(addr: SocketAddr) -> Framed<tokio::net::TcpStream, ProdCodeCodec> {
    Framed::new(tcp_stream(addr).await, ProdCodeCodec::new())
}

/// Starts the gateway, runs `exercise` against the address it announces, then stops it with
/// `signal` and returns what `run` returned.
async fn serve_then_stop<F, Fut>(cli: ServerCli, signal: libc::c_int, exercise: F) -> Result<()>
where
    F: FnOnce(SocketAddr) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let (announce, announced) = tokio::sync::oneshot::channel();
    let server = run_announcing(cli, Some(announce));
    let driver = async {
        let addr = tokio::time::timeout(Duration::from_secs(20), announced)
            .await
            .expect("the gateway starts")
            .expect("the gateway announces its address");
        exercise(addr).await;
        assert_eq!(unsafe { libc::kill(libc::getpid(), signal) }, 0);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(server, driver)
    })
    .await
    .expect("the gateway stops on the signal");
    result
}

#[tokio::test]
async fn a_gateway_that_requires_tls_without_credentials_refuses_to_start() {
    crate::test_cache_support::isolated!();
    configure_environment(None);
    unsafe {
        std::env::set_var("PROD_CODE_TLS_MODE", "strict");
    }
    let temp = tempfile::tempdir().unwrap();
    let mut cli =
        ServerCli::try_parse_from(["gateway", "--bind", "127.0.0.1:0", "--no-admission"]).unwrap();
    cli.storage = temp.path().join("storage");
    let error = run(cli).await.unwrap_err().to_string();
    assert!(
        error.contains("is required on server")
            && error.contains("PROD_CODE_TLS_CERT")
            && error.contains("PROD_CODE_TLS_KEY"),
        "{error}"
    );
}

#[tokio::test]
async fn a_running_gateway_serves_tcp_and_a_socket_then_stops_on_sigterm() {
    crate::test_cache_support::isolated!();
    configure_environment(None);
    let temp = tempfile::tempdir().unwrap();
    let shadow = temp.path().join("shadow");
    std::fs::create_dir_all(shadow.join(".prod-code-hypothesis-abandoned")).unwrap();
    std::fs::write(shadow.join("keep.txt"), "kept").unwrap();
    let build_cache = temp.path().join("build-cache");
    std::fs::create_dir_all(build_cache.join("idle")).unwrap();
    let socket = temp.path().join("gateway.sock");
    let mut cli =
        ServerCli::try_parse_from(["gateway", "--bind", "127.0.0.1:0", "--no-admission"]).unwrap();
    cli.storage = temp.path().join("storage");
    cli.shadow_dir = Some(shadow.clone());
    cli.socket_path = Some(socket.clone());
    cli.build_cache_ram = true;
    cli.build_cache_dir = Some(build_cache);
    cli.engines = vec![" UNAVAILABLE-TEST-ENGINE ".into(), " ".into()];
    cli.advertise = Some("sample-node".into());
    cli.peers = "sample-node, 127.0.0.1:1, ".into();

    let socket_path = socket.clone();
    let leftover = shadow.join(".prod-code-hypothesis-abandoned");
    let result = serve_then_stop(cli, libc::SIGTERM, |addr| async move {
        let (status, body) = http_get(tcp_stream(addr).await, "/health").await;
        assert_eq!(status, 200);
        assert_eq!(
            body,
            json!({ "status": "ok", "service": "prod-code-gateway" })
        );
        let mut over_tcp = tcp(addr).await;
        match ask(&mut over_tcp, WireMessage::StatusRequest).await {
            WireMessage::StatusResponse(status) => {
                assert_eq!(status.server_pid, std::process::id());
            }
            other => panic!("expected a status, got {other:?}"),
        }
        match ask(&mut over_tcp, WireMessage::ClusterRequest).await {
            WireMessage::ClusterResponse(view) => assert_eq!(view.this_node, "sample-node"),
            other => panic!("expected a cluster view, got {other:?}"),
        }

        let stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
        let (status, body) = http_get(stream, "/elsewhere").await;
        assert_eq!(status, 426);
        assert_eq!(body["error"], "protocol_mismatch");
    })
    .await;
    result.unwrap();
    assert!(!leftover.exists(), "an abandoned hypothesis is swept");
    assert_eq!(
        std::fs::read_to_string(shadow.join("keep.txt")).unwrap(),
        "kept"
    );
    assert!(!socket.exists(), "the socket path is removed on exit");
}

#[tokio::test]
async fn a_gateway_with_a_token_serves_only_connections_that_open_with_it_and_stops_on_sigint() {
    crate::test_cache_support::isolated!();
    configure_environment(Some("sample-token"));
    let temp = tempfile::tempdir().unwrap();
    let mut cli =
        ServerCli::try_parse_from(["gateway", "--bind", "0.0.0.0:0", "--no-admission"]).unwrap();
    cli.storage = temp.path().join("storage");
    cli.shadow_dir = Some(temp.path().join("shadow"));

    let result = serve_then_stop(cli, libc::SIGINT, |addr| async move {
        let (status, body) = http_get(tcp_stream(addr).await, "/healthz").await;
        assert_eq!(status, 200);
        assert_eq!(body["service"], "prod-code-gateway");
        for first in [
            WireMessage::StatusRequest,
            WireMessage::Auth(AuthToken("another-token".into())),
        ] {
            match ask(&mut tcp(addr).await, first).await {
                WireMessage::Disconnect { reason } => assert_eq!(reason, AUTH_REFUSED),
                other => panic!("expected a refusal, got {other:?}"),
            }
        }
        let mut client = tcp(addr).await;
        client
            .send(WireMessage::Auth(AuthToken("sample-token".into())))
            .await
            .unwrap();
        // With no advertised address the gateway names itself by the bound port.
        match ask(&mut client, WireMessage::ClusterRequest).await {
            WireMessage::ClusterResponse(view) => {
                assert!(
                    view.this_node.ends_with(&format!(":{}", addr.port())),
                    "{view:?}"
                );
            }
            other => panic!("expected a cluster view, got {other:?}"),
        }
    })
    .await;
    result.unwrap();
}
