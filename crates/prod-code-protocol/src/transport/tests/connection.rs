/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::buffer::{
    KEEPALIVE_IDLE, KEEPALIVE_INTERVAL, KEEPALIVE_RETRIES, parse_buffer_size, tune,
    tune_with_buffer_sizes,
};
#[cfg(unix)]
use super::super::client::connect_unix_with;
use super::super::client::{
    clear_client_tls_cache, connect_raw_tcp, connect_raw_tcp_with, connect_stream_with,
    init_client_tls_from_env, resolve_token,
};
use super::super::stream::AnyStream;
use crate::messages::{AuthToken, WireMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Decoder, Encoder};

/// The first frame `stream` receives.
async fn first_frame(stream: &mut TcpStream) -> WireMessage {
    let mut codec = crate::ProdCodeCodec::new();
    let mut buffer = bytes::BytesMut::new();
    loop {
        if let Some(message) = codec.decode(&mut buffer).unwrap() {
            return message;
        }
        assert!(stream.read_buf(&mut buffer).await.unwrap() > 0, "closed");
    }
}

#[tokio::test]
async fn a_connection_has_keepalive_and_no_nagle_on_both_ends() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (client, accepted) = tokio::join!(connect_raw_tcp(addr), listener.accept());
    let client = client.unwrap();
    let (server, _) = accepted.unwrap();
    tune(&server);
    for stream in [&client, &server] {
        let socket = socket2::SockRef::from(stream);
        assert!(stream.nodelay().unwrap());
        assert!(socket.keepalive().unwrap());
        assert_eq!(socket.tcp_keepalive_time().unwrap(), KEEPALIVE_IDLE);
        assert_eq!(socket.tcp_keepalive_interval().unwrap(), KEEPALIVE_INTERVAL);
        assert_eq!(socket.tcp_keepalive_retries().unwrap(), KEEPALIVE_RETRIES);
    }
}

/// With a token the connection's first frame is the token; without one nothing is sent
/// before the caller's own frames (#402).
#[tokio::test]
async fn a_connection_opens_with_the_token_when_there_is_one() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (client, accepted) = tokio::join!(
        connect_raw_tcp_with(addr, Some("s3cret")),
        listener.accept()
    );
    let _client = client.unwrap();
    let mut server = accepted.unwrap().0;
    assert_eq!(
        first_frame(&mut server).await,
        WireMessage::Auth(AuthToken("s3cret".to_string()))
    );

    let (client, accepted) = tokio::join!(connect_raw_tcp_with(addr, None), listener.accept());
    let mut client = client.unwrap();
    let mut ping = bytes::BytesMut::new();
    crate::ProdCodeCodec::new()
        .encode(WireMessage::Ping, &mut ping)
        .unwrap();
    client.write_all(&ping).await.unwrap();
    let mut server = accepted.unwrap().0;
    assert_eq!(first_frame(&mut server).await, WireMessage::Ping);
}

/// The variable wins over the file, the file's first line is the token, and blank ones do
/// not count; the token never shows in a message's `Debug` (#402).
#[test]
fn the_token_comes_from_the_variable_or_the_file_and_never_shows() {
    let dir = std::env::temp_dir().join(format!("prod-code-token-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("auth-token");
    std::fs::write(&file, "  from-file  \nsecond line\n").unwrap();
    assert_eq!(
        resolve_token(Some(" from-env "), Some(&file)).as_deref(),
        Some("from-env")
    );
    assert_eq!(
        resolve_token(Some("   "), Some(&file)).as_deref(),
        Some("from-file")
    );
    assert_eq!(resolve_token(None, Some(&dir.join("none"))), None);
    std::fs::write(&file, "\n").unwrap();
    assert_eq!(resolve_token(None, Some(&file)), None);
    let _ = std::fs::remove_dir_all(&dir);

    let message = WireMessage::Auth(AuthToken("s3cret".to_string()));
    assert!(!format!("{message:?}").contains("s3cret"));
    let token = AuthToken("s3cret".to_string());
    assert!(token.matches("s3cret"));
    assert!(!token.matches("s3creT"));
    assert!(!token.matches("s3cret2"));
}

#[cfg(unix)]
#[tokio::test]
async fn unix_socket_transport_supports_framing_and_auth_token() {
    let dir = std::env::temp_dir().join(format!("prod-code-test-sock-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock_path = dir.join("test.sock");
    let _ = std::fs::remove_file(&sock_path);
    let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

    let (client, accepted) = tokio::join!(
        connect_unix_with(&sock_path, Some("unix-secret")),
        listener.accept()
    );
    let client = client.unwrap();
    let mut server = accepted.unwrap().0;

    let mut codec = crate::ProdCodeCodec::new();
    let mut buffer = bytes::BytesMut::new();
    loop {
        if let Some(message) = codec.decode(&mut buffer).unwrap() {
            assert_eq!(
                message,
                WireMessage::Auth(AuthToken("unix-secret".to_string()))
            );
            break;
        }
        assert!(server.read_buf(&mut buffer).await.unwrap() > 0);
    }

    // Test AnyStream wrapping
    let mut any_client = AnyStream::Unix(client);
    let mut any_server = AnyStream::Unix(server);
    let mut ping = bytes::BytesMut::new();
    crate::ProdCodeCodec::new()
        .encode(WireMessage::Ping, &mut ping)
        .unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut any_client, &ping)
        .await
        .unwrap();

    let mut buf2 = bytes::BytesMut::new();
    loop {
        if let Some(msg) = codec.decode(&mut buf2).unwrap() {
            assert_eq!(msg, WireMessage::Ping);
            break;
        }
        assert!(
            tokio::io::AsyncReadExt::read_buf(&mut any_server, &mut buf2)
                .await
                .unwrap()
                > 0
        );
    }
}

#[tokio::test]
async fn any_stream_from_tcp() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = tokio::net::TcpStream::connect(addr).await.unwrap();
    let any: AnyStream = client.into();
    match any {
        AnyStream::Tcp(_) => {}
        _ => panic!("expected Tcp"),
    }
}

#[tokio::test]
async fn raw_tcp_with_fails_closed_when_tls_required_even_without_token() {
    let _lock = crate::tls::tests::TEST_ENV_LOCK.lock().await;
    unsafe {
        std::env::set_var(crate::tls::TLS_MODE_ENV, "strict");
    }
    let dummy_addr: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    let err = connect_raw_tcp_with(dummy_addr, None).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    unsafe {
        std::env::remove_var(crate::tls::TLS_MODE_ENV);
    }
}

#[tokio::test]
async fn raw_tcp_with_fails_on_invalid_mode() {
    let _lock = crate::tls::tests::TEST_ENV_LOCK.lock().await;
    unsafe {
        std::env::set_var(crate::tls::TLS_MODE_ENV, "stricts");
    }
    let dummy_addr: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
    let err = connect_raw_tcp_with(dummy_addr, None).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    unsafe {
        std::env::remove_var(crate::tls::TLS_MODE_ENV);
    }
}

#[tokio::test]
async fn connect_stream_with_succeeds_in_mutual_mode_after_caching_and_scrubbing_key_env() {
    use crate::tls::{ServerTlsConfig, upgrade_server_stream};
    use tokio::io::AsyncReadExt;

    let _lock = crate::tls::tests::TEST_ENV_LOCK.lock().await;
    clear_client_tls_cache();

    let mut client_cert_params =
        rcgen::CertificateParams::new(vec!["gateway-outbound".to_string()]).unwrap();
    client_cert_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "gateway-outbound");
    let client_key_pair = rcgen::KeyPair::generate().unwrap();
    let client_ca_key = rcgen::KeyPair::generate().unwrap();
    let client_ca_cert = rcgen::CertificateParams::new(vec!["client-ca".to_string()])
        .unwrap()
        .self_signed(&client_ca_key)
        .unwrap();
    let signed_client_cert = client_cert_params
        .signed_by(&client_key_pair, &client_ca_cert, &client_ca_key)
        .unwrap();

    let cert_file = tempfile::NamedTempFile::new().unwrap();
    let key_file = tempfile::NamedTempFile::new().unwrap();
    let ca_file = tempfile::NamedTempFile::new().unwrap();

    std::fs::write(cert_file.path(), signed_client_cert.pem()).unwrap();
    std::fs::write(key_file.path(), client_key_pair.serialize_pem()).unwrap();

    let mut ca_params = rcgen::CertificateParams::new(vec!["server-ca".to_string()]).unwrap();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "server-ca");
    let ca_key_pair = rcgen::KeyPair::generate().unwrap();
    let ca_cert_obj = ca_params.self_signed(&ca_key_pair).unwrap();
    std::fs::write(ca_file.path(), ca_cert_obj.pem()).unwrap();

    let mut server_params =
        rcgen::CertificateParams::new(vec!["cluster-peer".to_string()]).unwrap();
    server_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "cluster-peer");
    let s_key = rcgen::KeyPair::generate().unwrap();
    let s_cert = server_params
        .signed_by(&s_key, &ca_cert_obj, &ca_key_pair)
        .unwrap();

    let s_cert_der = rustls::pki_types::CertificateDer::from(s_cert.der().to_vec());
    let s_key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(s_key.serialize_der()),
    );

    let mut client_roots = rustls::RootCertStore::empty();
    client_roots
        .add(rustls::pki_types::CertificateDer::from(
            client_ca_cert.der().to_vec(),
        ))
        .unwrap();

    let server_tls = ServerTlsConfig::new(vec![s_cert_der], s_key_der)
        .with_client_ca(client_roots, true)
        .build()
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
        let mut token_buf = [0u8; 100];
        let n = tls_stream.read(&mut token_buf).await.unwrap();
        assert!(n > 0);
    });

    unsafe {
        std::env::set_var(crate::tls::TLS_MODE_ENV, "mutual");
        std::env::set_var(crate::tls::TLS_CERT_ENV, cert_file.path());
        std::env::set_var(crate::tls::TLS_KEY_ENV, key_file.path());
        std::env::set_var(crate::tls::TLS_CA_ENV, ca_file.path());
        std::env::set_var(crate::tls::TLS_SERVER_NAME_ENV, "cluster-peer");
    }

    init_client_tls_from_env().unwrap();

    // Simulate gateway scrubbing TLS_KEY_ENV at startup:
    unsafe {
        std::env::remove_var(crate::tls::TLS_KEY_ENV);
    }

    // Direct ClientTlsConfig::from_env() now fails:
    assert!(crate::tls::ClientTlsConfig::from_env().is_err());

    // But connect_stream_with uses cached default client TLS configuration and succeeds:
    let stream = connect_stream_with(addr, Some("cluster-token"))
        .await
        .unwrap();
    match stream {
        AnyStream::TlsClient(_) => {}
        _ => panic!("expected TlsClient stream"),
    }

    server_task.await.unwrap();

    clear_client_tls_cache();
    unsafe {
        std::env::remove_var(crate::tls::TLS_MODE_ENV);
        std::env::remove_var(crate::tls::TLS_CERT_ENV);
        std::env::remove_var(crate::tls::TLS_CA_ENV);
        std::env::remove_var(crate::tls::TLS_SERVER_NAME_ENV);
    }
}

#[test]
fn test_parse_buffer_size() {
    assert_eq!(parse_buffer_size(""), None);
    assert_eq!(parse_buffer_size("   "), None);
    assert_eq!(parse_buffer_size("4194304"), Some(4_194_304));
    assert_eq!(parse_buffer_size("64K"), Some(64 * 1024));
    assert_eq!(parse_buffer_size("512KiB"), Some(512 * 1024));
    assert_eq!(parse_buffer_size("4M"), Some(4 * 1024 * 1024));
    assert_eq!(parse_buffer_size("8MB"), Some(8 * 1024 * 1024));
    assert_eq!(parse_buffer_size("16MiB"), Some(16 * 1024 * 1024));
    assert_eq!(parse_buffer_size("1G"), Some(1024 * 1024 * 1024));
    assert_eq!(parse_buffer_size("invalid"), None);
}

#[tokio::test]
async fn test_tune_socket_buffers_applied() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let sock = socket2::SockRef::from(&stream);
    let recv_before = sock.recv_buffer_size().unwrap();
    let send_before = sock.send_buffer_size().unwrap();
    // Request a size above ordinary defaults. The OS may clamp it, so assert that each
    // resulting buffer increased rather than expecting the exact requested size.
    tune_with_buffer_sizes(&stream, Some(16 * 1024 * 1024), Some(16 * 1024 * 1024));
    let recv_buf = sock.recv_buffer_size().unwrap();
    let send_buf = sock.send_buffer_size().unwrap();
    assert!(
        recv_buf > recv_before,
        "recv buffer should increase from {recv_before} after tuning, got {recv_buf}"
    );
    assert!(
        send_buf > send_before,
        "send buffer should increase from {send_before} after tuning, got {send_buf}"
    );
}
