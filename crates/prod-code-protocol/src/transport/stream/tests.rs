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
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn exchange(mut left: AnyStream, mut right: AnyStream) {
    left.write_all(b"request").await.unwrap();
    left.flush().await.unwrap();
    let mut request = [0; 7];
    right.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"request");
    right.write_all(b"reply").await.unwrap();
    right.flush().await.unwrap();
    let mut reply = [0; 5];
    left.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"reply");
    left.shutdown().await.unwrap();
    assert_eq!(right.read(&mut request).await.unwrap(), 0);
    right.shutdown().await.unwrap();
}

#[tokio::test]
async fn tcp_and_tls_delegate_bidirectional_io() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let (peer, _) = listener.accept().await.unwrap();
    let tcp: AnyStream = tcp.into();
    assert_eq!(tcp.peer_addr().unwrap(), address);
    assert!(format!("{tcp:?}").starts_with("Tcp("));
    exchange(tcp, peer.into()).await;

    let (ca, cert, key) = crate::tls::test_helpers::generate_test_ca_and_cert("example.test");
    let server = crate::tls::ServerTlsConfig::new(vec![cert], key)
        .build()
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(ca).unwrap();
    let (client, name) = crate::tls::ClientTlsConfig::new()
        .with_ca(roots)
        .with_server_name("example.test")
        .build()
        .unwrap();
    let tcp = TcpStream::connect(address).await.unwrap();
    let client_address = tcp.local_addr().unwrap();
    let (peer, _) = listener.accept().await.unwrap();
    let (left, right) = tokio::join!(
        crate::tls::upgrade_client_stream(tcp, client, name),
        crate::tls::upgrade_server_stream(peer, server)
    );
    let left: AnyStream = left.unwrap().into();
    let right: AnyStream = right.unwrap().into();
    assert_eq!(left.peer_addr().unwrap(), address);
    assert_eq!(right.peer_addr().unwrap(), client_address);
    assert_eq!(format!("{left:?}"), format!("TlsClient(Some({address}))"));
    assert_eq!(
        format!("{right:?}"),
        format!("TlsServer(Some({client_address}))")
    );
    exchange(left, right).await;
}

#[cfg(unix)]
#[tokio::test]
async fn unix_stream_io_and_address_error() {
    let (left, right) = tokio::net::UnixStream::pair().unwrap();
    let left: AnyStream = left.into();
    let err = left.peer_addr().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AddrNotAvailable);
    assert_eq!(err.to_string(), "unix domain socket has no IP peer_addr");
    assert!(format!("{left:?}").starts_with("Unix("));
    exchange(left, right.into()).await;
}

#[test]
fn command_scrubbers_explicitly_remove_all_secrets() {
    let mut command = std::process::Command::new("unused");
    let vars: Vec<_> = AUTH_TOKEN_VARS
        .into_iter()
        .chain(crate::tls::TLS_ENV_VARS)
        .collect();
    for var in &vars {
        command.env(var, "test-value");
    }
    command
        .env("NEUTRAL_SETTING", "preserved")
        .scrub_cluster_secrets();
    for var in &vars {
        assert_eq!(
            command.get_envs().find(|(key, _)| *key == *var).unwrap().1,
            None
        );
    }
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == "NEUTRAL_SETTING")
            .unwrap()
            .1
            .unwrap(),
        "preserved"
    );
    let mut command = tokio::process::Command::new("unused");
    for var in &vars {
        command.env(var, "test-value");
    }
    command.scrub_cluster_secrets();
    for var in vars {
        assert_eq!(
            command
                .as_std()
                .get_envs()
                .find(|(key, _)| *key == var)
                .unwrap()
                .1,
            None
        );
    }
}
