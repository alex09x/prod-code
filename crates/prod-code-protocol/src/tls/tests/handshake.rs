/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::client::{ClientTlsConfig, upgrade_client_stream};
use super::super::server::{ServerTlsConfig, upgrade_server_stream};
use super::super::test_helpers::{generate_test_ca_and_cert, generate_test_self_signed};
use super::super::verifier::cert_sha256_fingerprint;
use rustls::RootCertStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test]
async fn tls_handshake_with_ca_verification_succeeds() {
    let (ca_der, server_cert, server_key) = generate_test_ca_and_cert("localhost");

    // Configure server
    let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
        .build()
        .unwrap();

    // Configure client with CA
    let mut roots = RootCertStore::empty();
    roots.add(ca_der).unwrap();
    let (client_tls, server_name) = ClientTlsConfig::new()
        .with_ca(roots)
        .with_server_name("localhost")
        .build()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
        let mut buf = [0u8; 5];
        tls_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping!");
        tls_stream.write_all(b"pong!").await.unwrap();
        tls_stream.flush().await.unwrap();
    });

    let client_stream = TcpStream::connect(addr).await.unwrap();
    let mut tls_client = upgrade_client_stream(client_stream, client_tls, server_name)
        .await
        .unwrap();

    tls_client.write_all(b"ping!").await.unwrap();
    tls_client.flush().await.unwrap();

    let mut reply = [0u8; 5];
    tls_client.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply, b"pong!");

    server_task.await.unwrap();
}

#[tokio::test]
async fn tls_certificate_pinning_verification_succeeds() {
    let (server_cert, server_key) = generate_test_self_signed("node-1.code.internal");
    let pin = cert_sha256_fingerprint(&server_cert);

    let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
        .build()
        .unwrap();

    let (client_tls, server_name) = ClientTlsConfig::new()
        .with_pins(vec![pin])
        .with_server_name("node-1.code.internal")
        .build()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
        tls_stream.write_all(b"secure").await.unwrap();
        tls_stream.flush().await.unwrap();
    });

    let client_stream = TcpStream::connect(addr).await.unwrap();
    let mut tls_client = upgrade_client_stream(client_stream, client_tls, server_name)
        .await
        .unwrap();

    let mut buf = [0u8; 6];
    tls_client.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"secure");

    server_task.await.unwrap();
}

#[tokio::test]
async fn tls_invalid_pin_or_untrusted_ca_fails_handshake() {
    let (server_cert, server_key) = generate_test_self_signed("bad-node.internal");

    let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
        .build()
        .unwrap();

    // Pin is completely wrong
    let bogus_pin = "0000000000000000000000000000000000000000000000000000000000000000".to_string();
    let (client_tls, server_name) = ClientTlsConfig::new()
        .with_pins(vec![bogus_pin])
        .with_server_name("bad-node.internal")
        .build()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = upgrade_server_stream(stream, server_tls).await;
    });

    let client_stream = TcpStream::connect(addr).await.unwrap();
    let client_res = upgrade_client_stream(client_stream, client_tls, server_name).await;
    assert!(client_res.is_err(), "Handshake with wrong pin must fail");

    let _ = server_task.await;
}

#[tokio::test]
async fn mtls_mutual_authentication_succeeds() {
    let (ca_der, server_cert, server_key) = generate_test_ca_and_cert("cluster-peer");
    let (client_ca, client_cert, client_key) = generate_test_ca_and_cert("agent-worker");

    // Server requires client cert and trusts client_ca
    let mut client_roots = RootCertStore::empty();
    client_roots.add(client_ca).unwrap();

    let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
        .with_client_ca(client_roots, true)
        .build()
        .unwrap();

    // Client presents cert and trusts server CA
    let mut server_roots = RootCertStore::empty();
    server_roots.add(ca_der).unwrap();

    let (client_tls, server_name) = ClientTlsConfig::new()
        .with_ca(server_roots)
        .with_client_cert(vec![client_cert], client_key)
        .with_server_name("cluster-peer")
        .build()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
        tls_stream.write_all(b"mtls-ok").await.unwrap();
        tls_stream.flush().await.unwrap();
    });

    let client_stream = TcpStream::connect(addr).await.unwrap();
    let mut tls_client = upgrade_client_stream(client_stream, client_tls, server_name)
        .await
        .unwrap();

    let mut buf = [0u8; 7];
    tls_client.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"mtls-ok");

    server_task.await.unwrap();
}

#[tokio::test]
async fn mtls_untrusted_client_cert_is_rejected() {
    let (ca_der, server_cert, server_key) = generate_test_ca_and_cert("cluster-peer");
    let (_untrusted_ca, untrusted_client_cert, untrusted_client_key) =
        generate_test_ca_and_cert("untrusted-agent");

    let mut client_roots = RootCertStore::empty();
    client_roots.add(ca_der.clone()).unwrap();

    let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
        .with_client_ca(client_roots, true)
        .build()
        .unwrap();

    let mut server_roots = RootCertStore::empty();
    server_roots.add(ca_der).unwrap();

    let (client_tls, server_name) = ClientTlsConfig::new()
        .with_ca(server_roots)
        .with_client_cert(vec![untrusted_client_cert], untrusted_client_key)
        .with_server_name("cluster-peer")
        .build()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let res = upgrade_server_stream(stream, server_tls).await;
        assert!(
            res.is_err(),
            "Server must reject untrusted client cert: {res:?}"
        );
    });

    let client_stream = TcpStream::connect(addr).await.unwrap();
    match upgrade_client_stream(client_stream, client_tls, server_name).await {
        Ok(mut tls_client) => {
            let mut buf = [0u8; 1];
            let read_res = tls_client.read_exact(&mut buf).await;
            assert!(
                read_res.is_err(),
                "Read must fail because server rejected client cert"
            );
        }
        Err(_) => {
            // Client handshake failed directly
        }
    }

    server_task.await.unwrap();
}

#[tokio::test]
async fn only_tls13_is_negotiated() {
    let (ca_der, server_cert, server_key) = generate_test_ca_and_cert("tls13-only.internal");

    let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
        .build()
        .unwrap();

    let mut roots = RootCertStore::empty();
    roots.add(ca_der).unwrap();
    let (client_tls, server_name) = ClientTlsConfig::new()
        .with_ca(roots)
        .with_server_name("tls13-only.internal")
        .build()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
        let (_, server_conn) = tls_stream.get_ref();
        assert_eq!(
            server_conn.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
    });

    let client_stream = TcpStream::connect(addr).await.unwrap();
    let tls_client = upgrade_client_stream(client_stream, client_tls, server_name)
        .await
        .unwrap();
    let (_, client_conn) = tls_client.get_ref();
    assert_eq!(
        client_conn.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );

    server_task.await.unwrap();
}
