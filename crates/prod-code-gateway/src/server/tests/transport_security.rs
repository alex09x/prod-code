/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::serve::{configure_environment, serve_then_stop};
use crate::ServerCli;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::tls::{ClientTlsConfig, pki};
use prod_code_protocol::{AnyStream, AuthToken, ProdCodeCodec, WireMessage};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

struct CaptureProxy {
    listen_addr: SocketAddr,
    captured: Arc<Mutex<Vec<u8>>>,
    task: tokio::task::JoinHandle<()>,
}

impl CaptureProxy {
    async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let capture_client_bytes = Arc::clone(&captured);
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(target) = TcpStream::connect(target).await else {
                    break;
                };
                let (mut client_read, mut client_write) = client.into_split();
                let (mut target_read, mut target_write) = target.into_split();
                let capture = Arc::clone(&capture_client_bytes);
                let client_to_gateway = tokio::spawn(async move {
                    let mut bytes = [0u8; 8192];
                    loop {
                        let count = match client_read.read(&mut bytes).await {
                            Ok(0) | Err(_) => break,
                            Ok(count) => count,
                        };
                        capture.lock().unwrap().extend_from_slice(&bytes[..count]);
                        if target_write.write_all(&bytes[..count]).await.is_err() {
                            break;
                        }
                    }
                });
                let gateway_to_client = tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut target_read, &mut client_write).await;
                });
                let _ = tokio::join!(client_to_gateway, gateway_to_client);
            }
        });
        Self {
            listen_addr,
            captured,
            task,
        }
    }

    fn captured(&self) -> Vec<u8> {
        self.captured.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.captured.lock().unwrap().clear();
    }
}

impl Drop for CaptureProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[tokio::test]
async fn transport_security_production_wiring_uses_the_configured_client_and_gateway() {
    crate::test_cache_support::isolated!();
    configure_environment(Some("CANARY_AUTH_TOKEN_PRODUCTION_WIRING"));

    let temp = tempfile::tempdir().unwrap();
    let (ca_cert, ca_key) = pki::generate_ca("production-wiring-ca").unwrap();
    let (node_cert, node_key) = pki::generate_node_cert(
        &ca_cert,
        &ca_key,
        &["localhost".to_string()],
        &["127.0.0.1".parse().unwrap()],
    )
    .unwrap();
    let pki_dir = temp.path().join("pki");
    let (ca_path, _) = pki::write_cert_and_key(&pki_dir, "ca", &ca_cert, &ca_key).unwrap();
    let (cert_path, key_path) =
        pki::write_cert_and_key(&pki_dir, "node", &node_cert, &node_key).unwrap();
    unsafe {
        std::env::set_var("PROD_CODE_TLS_MODE", "strict");
        std::env::set_var("PROD_CODE_TLS_CERT", &cert_path);
        std::env::set_var("PROD_CODE_TLS_KEY", &key_path);
        std::env::set_var("PROD_CODE_TLS_CA", &ca_path);
        std::env::set_var("PROD_CODE_TLS_SERVER_NAME", "localhost");
    }

    let mut cli =
        ServerCli::try_parse_from(["gateway", "--bind", "127.0.0.1:0", "--no-admission"]).unwrap();
    cli.storage = temp.path().join("storage");
    let token = "CANARY_AUTH_TOKEN_PRODUCTION_WIRING";
    let result = serve_then_stop(cli, libc::SIGTERM, |gateway_addr| async move {
        let proxy = CaptureProxy::start(gateway_addr).await;

        // Negative control: without the production client's TLS upgrade, the real strict
        // gateway rejects the socket after the raw auth frame has crossed the proxy.
        let plain_socket = TcpStream::connect(proxy.listen_addr).await.unwrap();
        let mut plain = Framed::new(AnyStream::Tcp(plain_socket), ProdCodeCodec::new());
        plain
            .send(WireMessage::Auth(AuthToken(token.to_string())))
            .await
            .unwrap();
        plain.get_mut().shutdown().await.unwrap();
        let mut rejection = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            plain.get_mut().read_to_end(&mut rejection),
        )
        .await
        .expect("the plaintext connection is rejected")
        .unwrap();
        assert!(rejection.starts_with(b"HTTP/1.1 426 Upgrade Required"));
        assert!(contains_bytes(&proxy.captured(), token.as_bytes()));

        // The production protocol connection reads the configured TLS policy, performs
        // the client handshake, and sends the auth token after TLS is established.
        proxy.clear();
        let client_tls = ClientTlsConfig::from_env()
            .unwrap()
            .expect("the client TLS settings are configured")
            .build()
            .unwrap();
        let stream = prod_code_protocol::transport::connect_stream_with_client_config(
            proxy.listen_addr,
            Some(token),
            Some(client_tls),
        )
        .await
        .unwrap();
        let mut client = Framed::new(stream, ProdCodeCodec::new());
        client.send(WireMessage::StatusRequest).await.unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(10), client.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            WireMessage::StatusResponse(_)
        ));
        let message_canary = "CANARY_STATUS_DISCONNECT_REASON";
        let bytes_before_message = proxy.captured().len();
        client
            .send(WireMessage::Disconnect {
                reason: message_canary.into(),
            })
            .await
            .unwrap();
        let captured = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let captured = proxy.captured();
                if captured.len() > bytes_before_message {
                    return captured;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the application frame crosses the capture proxy");
        assert_eq!(captured.first(), Some(&0x16), "wire starts with a TLS handshake");
        assert!(
            !contains_bytes(&captured, token.as_bytes()),
            "the configured production connection must keep auth off the captured wire"
        );
        assert!(
            !contains_bytes(&captured, message_canary.as_bytes()),
            "the configured production connection must keep application frames off the captured wire"
        );
    })
    .await;
    assert!(result.is_ok(), "{result:?}");
}
