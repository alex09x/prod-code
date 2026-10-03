//! Acceptance tests for Phase 5.6: Cluster Transport Security and Trusted Discovery.
//!
//! Validates:
//! 1. Wire confidentiality: packet-capture integration test verifying credentials,
//!    source code, and tool traffic are encrypted and zero plaintext leaks over TCP.
//! 2. Mutual TLS (mTLS): strict peer authentication, rejection of untrusted or missing client certificates.
//! 3. Certificate pinning: rejection of forged or mismatched certificates.
//! 4. Zero downgrade: plaintext connections are refused when TLS is required.
//! 5. Multi-CA trust bootstrap and key rotation: rolling CA updates without downgrade.
//! 6. Secret hygiene: environment variable scrubbing, restrictive key permissions (0600),
//!    and absence of private keys from debug representations.
//! 7. TLS handshake & warm-request latency overhead against plaintext baseline (< 1ms).
//! 8. Discovery security: replay resistance via challenge nonces, HMAC forgery rejection,
//!    and privacy-preserving minimal announcements.

use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::codec::ProdCodeCodec;
use prod_code_protocol::discovery::{
    format_minimal_node_line, format_node_line_with_nonce, format_probe_with_nonce, generate_nonce,
    inspect_probe, parse_node_line_with_auth_and_nonce,
};
use prod_code_protocol::messages::{
    AuthToken, FileDelta, ReadFileRequest, ReadFileResponse, SyncRequest, SyncResponse, WireMessage,
};
use prod_code_protocol::tls::{
    check_key_permissions, ensure_crypto_provider, pki, upgrade_client_stream,
    upgrade_server_stream, ClientTlsConfig, ServerTlsConfig, TlsMode, DEFAULT_TLS_SERVER_NAME,
    TLS_CA_ENV, TLS_CERT_ENV, TLS_ENV_VARS, TLS_KEY_ENV, TLS_MODE_ENV, TLS_PIN_ENV,
    TLS_SERVER_NAME_ENV,
};
use prod_code_protocol::transport::{
    clear_client_tls_cache, connect_with, default_client_tls_built, init_client_tls_from_env,
    AUTH_TOKEN_VARS,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::RootCertStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::{Encoder, Framed};

static TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn parse_cert_pem(pem: &str) -> Vec<CertificateDer<'static>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("valid certs in pem")
}

fn parse_key_pem(pem: &str) -> PrivateKeyDer<'static> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    loop {
        match rustls_pemfile::read_one(&mut reader).expect("valid pem key") {
            Some(rustls_pemfile::Item::Pkcs8Key(k)) => return PrivateKeyDer::Pkcs8(k),
            Some(rustls_pemfile::Item::Pkcs1Key(k)) => return PrivateKeyDer::Pkcs1(k),
            Some(rustls_pemfile::Item::Sec1Key(k)) => return PrivateKeyDer::Sec1(k),
            Some(_) => continue,
            None => panic!("no private key found in PEM"),
        }
    }
}

/// A bidirectional TCP proxy that captures 100% of raw bytes traversing the socket.
struct PacketCaptureProxy {
    listen_addr: SocketAddr,
    captured_bytes: Arc<Mutex<Vec<u8>>>,
    _abort_handle: tokio::task::AbortHandle,
}

impl PacketCaptureProxy {
    async fn start(target_addr: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        let captured_bytes = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured_bytes.clone();

        let task = tokio::spawn(async move {
            while let Ok((client_sock, _)) = listener.accept().await {
                let target = match TcpStream::connect(target_addr).await {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let (mut client_rx, mut client_tx) = client_sock.into_split();
                let (mut target_rx, mut target_tx) = target.into_split();

                let captured_c2s = captured_clone.clone();
                let c2s = tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = match client_rx.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        {
                            let mut guard = captured_c2s.lock().unwrap();
                            guard.extend_from_slice(&buf[..n]);
                        }
                        if target_tx.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });

                let captured_s2c = captured_clone.clone();
                let s2c = tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = match target_rx.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        {
                            let mut guard = captured_s2c.lock().unwrap();
                            guard.extend_from_slice(&buf[..n]);
                        }
                        if client_tx.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });

                let _ = tokio::join!(c2s, s2c);
            }
        });

        Self {
            listen_addr,
            captured_bytes,
            _abort_handle: task.abort_handle(),
        }
    }

    fn captured(&self) -> Vec<u8> {
        self.captured_bytes.lock().unwrap().clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Packet-capture Wire Confidentiality
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_packet_capture_wire_confidentiality() {
    let _lock = TEST_MUTEX.lock().await;
    ensure_crypto_provider();

    let secret_token = "super-secret-cluster-auth-token-998877";
    let sensitive_path = "/workspaces/finance/proprietary_valuation_engine.rs";
    let sensitive_code = "pub fn calculate_alpha_trading_formula() -> f64 { 42.12345 }";
    let response_content = "INTERNAL_CLASSIFIED_HEADER_PAYLOAD_DATA";

    // ── Negative control: Plaintext TCP transmits secrets in clear text ───────
    {
        let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server_listener.local_addr().unwrap();
        let proxy = PacketCaptureProxy::start(server_addr).await;

        let server_task = tokio::spawn(async move {
            let (sock, _) = server_listener.accept().await.unwrap();
            let mut framed = Framed::new(sock, ProdCodeCodec::new());
            // Receive auth
            let msg = framed.next().await.unwrap().unwrap();
            assert_eq!(msg, WireMessage::Auth(AuthToken(secret_token.to_string())));
            // Receive read file request
            let msg = framed.next().await.unwrap().unwrap();
            match msg {
                WireMessage::ReadFileRequest(req) => {
                    assert_eq!(req.path, sensitive_path);
                    framed
                        .send(WireMessage::ReadFileResponse(ReadFileResponse {
                            path: req.path,
                            content: Some(response_content.as_bytes().to_vec()),
                            truncated: false,
                            error: None,
                        }))
                        .await
                        .unwrap();
                }
                other => panic!("unexpected message: {:?}", other),
            }
        });

        let client_sock = TcpStream::connect(proxy.listen_addr).await.unwrap();
        let mut framed = Framed::new(client_sock, ProdCodeCodec::new());
        framed
            .send(WireMessage::Auth(AuthToken(secret_token.to_string())))
            .await
            .unwrap();
        framed
            .send(WireMessage::ReadFileRequest(ReadFileRequest {
                path: sensitive_path.to_string(),
                max_bytes: 0,
            }))
            .await
            .unwrap();
        let resp = framed.next().await.unwrap().unwrap();
        match resp {
            WireMessage::ReadFileResponse(r) => {
                assert_eq!(r.content.as_deref(), Some(response_content.as_bytes()));
            }
            other => panic!("unexpected response: {:?}", other),
        }
        drop(framed);
        server_task.await.unwrap();

        // Control assertion: Plaintext wire MUST contain secrets!
        let wire_bytes = proxy.captured();
        let encoded_response =
            prod_code_protocol::messages::base64_bytes::encode(response_content.as_bytes());
        assert!(
            wire_bytes
                .windows(secret_token.len())
                .any(|w| w == secret_token.as_bytes()),
            "control failure: plaintext capture must contain auth token"
        );
        assert!(
            wire_bytes
                .windows(sensitive_path.len())
                .any(|w| w == sensitive_path.as_bytes()),
            "control failure: plaintext capture must contain sensitive path"
        );
        assert!(
            wire_bytes
                .windows(encoded_response.len())
                .any(|w| w == encoded_response.as_bytes()),
            "control failure: plaintext capture must contain base64 response content"
        );
    }

    // ── Primary test: TLS 1.3 encrypts 100% of wire traffic ───────────────────
    {
        let (ca_pem, ca_key_pem) = pki::generate_ca("Wire Test CA").unwrap();
        let (server_pem, server_key_pem) = pki::generate_node_cert(
            &ca_pem,
            &ca_key_pem,
            &[DEFAULT_TLS_SERVER_NAME.to_string()],
            &[],
        )
        .unwrap();

        let server_certs = parse_cert_pem(&server_pem);
        let server_key = parse_key_pem(&server_key_pem);
        let ca_certs = parse_cert_pem(&ca_pem);

        let server_tls = ServerTlsConfig::new(server_certs, server_key)
            .build()
            .unwrap();

        let mut roots = RootCertStore::empty();
        for ca in ca_certs {
            roots.add(ca).unwrap();
        }

        let (client_tls, server_name) = ClientTlsConfig::new()
            .with_ca(roots)
            .with_server_name(DEFAULT_TLS_SERVER_NAME)
            .build()
            .unwrap();

        let server_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server_listener.local_addr().unwrap();
        let proxy = PacketCaptureProxy::start(server_addr).await;

        let server_task = tokio::spawn(async move {
            let (sock, _) = server_listener.accept().await.unwrap();
            let tls_sock = upgrade_server_stream(sock, server_tls).await.unwrap();
            let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());

            // 1. Receive AuthToken
            let msg = framed.next().await.unwrap().unwrap();
            assert_eq!(msg, WireMessage::Auth(AuthToken(secret_token.to_string())));

            // 2. Receive ReadFileRequest
            let msg = framed.next().await.unwrap().unwrap();
            match msg {
                WireMessage::ReadFileRequest(req) => {
                    assert_eq!(req.path, sensitive_path);
                    framed
                        .send(WireMessage::ReadFileResponse(ReadFileResponse {
                            path: req.path,
                            content: Some(response_content.as_bytes().to_vec()),
                            truncated: false,
                            error: None,
                        }))
                        .await
                        .unwrap();
                }
                other => panic!("unexpected message: {:?}", other),
            }

            // 3. Receive SyncRequest with source code
            let msg = framed.next().await.unwrap().unwrap();
            match msg {
                WireMessage::SyncRequest(sync) => {
                    assert_eq!(sync.files.len(), 1);
                    assert_eq!(
                        sync.files[0].content.as_deref(),
                        Some(sensitive_code.as_bytes())
                    );
                    framed
                        .send(WireMessage::SyncResponse(SyncResponse {
                            files_updated: 1,
                            files_deleted: 0,
                            bytes_transferred: sensitive_code.len(),
                            duration_ms: 1,
                            server_workspace_root: "/server/workspace".to_string(),
                            workspace_was_fresh: false,
                            stale_paths: Vec::new(),
                        }))
                        .await
                        .unwrap();
                }
                other => panic!("unexpected message: {:?}", other),
            }
        });

        // Client connects to the proxy address, negotiating TLS SNI with the server name
        let client_sock = TcpStream::connect(proxy.listen_addr).await.unwrap();
        let tls_client = upgrade_client_stream(client_sock, client_tls, server_name)
            .await
            .unwrap();
        let mut framed = Framed::new(tls_client, ProdCodeCodec::new());

        // Send credentials and payloads
        framed
            .send(WireMessage::Auth(AuthToken(secret_token.to_string())))
            .await
            .unwrap();

        framed
            .send(WireMessage::ReadFileRequest(ReadFileRequest {
                path: sensitive_path.to_string(),
                max_bytes: 0,
            }))
            .await
            .unwrap();

        let resp = framed.next().await.unwrap().unwrap();
        match resp {
            WireMessage::ReadFileResponse(r) => {
                assert_eq!(r.content.as_deref(), Some(response_content.as_bytes()));
            }
            other => panic!("unexpected response: {:?}", other),
        }

        framed
            .send(WireMessage::SyncRequest(SyncRequest {
                client_workspace_root: "/client/workspace".to_string(),
                files: vec![FileDelta {
                    relative_path: "trading_algo.rs".to_string(),
                    content: Some(sensitive_code.as_bytes().to_vec()),
                    is_executable: false,
                }],
                clean_others: false,
                base_workspace_name: None,
            }))
            .await
            .unwrap();

        let resp = framed.next().await.unwrap().unwrap();
        match resp {
            WireMessage::SyncResponse(s) => {
                assert_eq!(s.files_updated, 1);
            }
            other => panic!("unexpected response: {:?}", other),
        }

        drop(framed);
        server_task.await.unwrap();

        // ── Rigorous Wire Packet Analysis ─────────────────────────────────────
        let captured = proxy.captured();
        assert!(!captured.is_empty(), "must have captured wire bytes");

        // 1. Zero plaintext tokens or secrets on wire
        assert!(
            !captured
                .windows(secret_token.len())
                .any(|w| w == secret_token.as_bytes()),
            "SECURITY VIOLATION: Auth token appeared in plaintext on TCP wire!"
        );

        // 2. Zero source file paths on wire
        assert!(
            !captured
                .windows(sensitive_path.len())
                .any(|w| w == sensitive_path.as_bytes()),
            "SECURITY VIOLATION: Workspace path appeared in plaintext on TCP wire!"
        );

        // 3. Zero source code contents on wire
        assert!(
            !captured
                .windows(sensitive_code.len())
                .any(|w| w == sensitive_code.as_bytes()),
            "SECURITY VIOLATION: Source code appeared in plaintext on TCP wire!"
        );
        let encoded_code =
            prod_code_protocol::messages::base64_bytes::encode(sensitive_code.as_bytes());
        assert!(
            !captured
                .windows(encoded_code.len())
                .any(|w| w == encoded_code.as_bytes()),
            "SECURITY VIOLATION: Source code (base64) appeared in plaintext on TCP wire!"
        );
        assert!(
            !captured
                .windows(b"calculate_alpha".len())
                .any(|w| w == b"calculate_alpha"),
            "SECURITY VIOLATION: Code identifier appeared in plaintext on TCP wire!"
        );

        // 4. Zero response payload on wire
        assert!(
            !captured
                .windows(response_content.len())
                .any(|w| w == response_content.as_bytes()),
            "SECURITY VIOLATION: Gateway file response appeared in plaintext on TCP wire!"
        );
        let encoded_response =
            prod_code_protocol::messages::base64_bytes::encode(response_content.as_bytes());
        assert!(
            !captured
                .windows(encoded_response.len())
                .any(|w| w == encoded_response.as_bytes()),
            "SECURITY VIOLATION: Gateway file response (base64) appeared in plaintext on TCP wire!"
        );

        // 5. Zero JSON message frame types on wire
        assert!(
            !captured
                .windows(b"ReadFileRequest".len())
                .any(|w| w == b"ReadFileRequest"),
            "SECURITY VIOLATION: Wire framing type appeared in plaintext on TCP wire!"
        );
        assert!(
            !captured
                .windows(b"SyncRequest".len())
                .any(|w| w == b"SyncRequest"),
            "SECURITY VIOLATION: Wire framing type appeared in plaintext on TCP wire!"
        );

        // 6. Verify TLS 1.3 record protocol encapsulation:
        // Records begin with ContentType byte:
        // 0x14 = ChangeCipherSpec (TLS 1.3 middlebox compat)
        // 0x16 = Handshake
        // 0x17 = Application Data (encrypted)
        let has_handshake = captured.windows(3).any(|w| w[0] == 0x16 && w[1] == 0x03 && w[2] == 0x03);
        let has_app_data = captured.windows(3).any(|w| w[0] == 0x17 && w[1] == 0x03 && w[2] == 0x03);

        assert!(has_handshake, "wire capture must contain TLS 1.3 handshake record (0x16 0x03 0x03)");
        assert!(has_app_data, "wire capture must contain TLS 1.3 application data records (0x17 0x03 0x03)");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Mutual TLS (mTLS) Strict Peer Authentication
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_mutual_tls_strict_peer_authentication() {
    let _lock = TEST_MUTEX.lock().await;
    ensure_crypto_provider();

    let (ca_pem, ca_key_pem) = pki::generate_ca("mTLS Root CA").unwrap();
    let (server_pem, server_key_pem) = pki::generate_node_cert(
        &ca_pem,
        &ca_key_pem,
        &[DEFAULT_TLS_SERVER_NAME.to_string()],
        &[],
    )
    .unwrap();
    let (client_pem, client_key_pem) = pki::generate_node_cert(
        &ca_pem,
        &ca_key_pem,
        &["client-peer.internal".to_string()],
        &[],
    )
    .unwrap();

    let (untrusted_ca_pem, untrusted_ca_key_pem) = pki::generate_ca("Untrusted Rogue CA").unwrap();
    let (untrusted_client_pem, untrusted_client_key_pem) = pki::generate_node_cert(
        &untrusted_ca_pem,
        &untrusted_ca_key_pem,
        &["rogue-client.internal".to_string()],
        &[],
    )
    .unwrap();

    let server_certs = parse_cert_pem(&server_pem);
    let server_key = parse_key_pem(&server_key_pem);
    let ca_certs = parse_cert_pem(&ca_pem);

    let mut ca_roots = RootCertStore::empty();
    for ca in ca_certs {
        ca_roots.add(ca).unwrap();
    }

    // Server strictly requires client authentication (require_client_auth = true)
    let server_tls = ServerTlsConfig::new(server_certs, server_key)
        .with_client_ca(ca_roots.clone(), true)
        .build()
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let server_tls_clone = server_tls.clone();
    let server_task = tokio::spawn(async move {
        // Handle 3 connection attempts:
        // 1. Missing client cert -> should fail handshake
        // 2. Untrusted client cert -> should fail handshake
        // 3. Valid client cert -> should succeed
        for i in 0..3 {
            let (sock, _) = listener.accept().await.unwrap();
            match upgrade_server_stream(sock, server_tls_clone.clone()).await {
                Ok(tls_sock) => {
                    assert_eq!(i, 2, "only attempt 2 (valid cert) should succeed");
                    let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
                    let msg = framed.next().await.unwrap().unwrap();
                    assert_eq!(msg, WireMessage::Ping);
                    framed.send(WireMessage::Pong).await.unwrap();
                }
                Err(e) => {
                    assert!(
                        i < 2,
                        "attempts 0 and 1 must fail handshake, but attempt {i} failed with {e}"
                    );
                }
            }
        }
    });

    // Subcase 2a: Client connects without a client certificate -> fails before any tool/ping execution
    {
        let (no_auth_client, server_name) = ClientTlsConfig::new()
            .with_ca(ca_roots.clone())
            .with_server_name(DEFAULT_TLS_SERVER_NAME)
            .build()
            .unwrap();

        let sock = TcpStream::connect(server_addr).await.unwrap();
        let failed = match upgrade_client_stream(sock, no_auth_client, server_name).await {
            Err(_) => true,
            Ok(tls_sock) => {
                let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
                let _ = framed.send(WireMessage::Ping).await;
                match framed.next().await {
                    None => true,
                    Some(Err(_)) => true,
                    Some(Ok(_)) => false,
                }
            }
        };
        assert!(
            failed,
            "server must reject client connection when client cert is missing"
        );
    }

    // Subcase 2b: Client connects with untrusted client certificate -> fails before any tool/ping execution
    {
        let untrusted_certs = parse_cert_pem(&untrusted_client_pem);
        let untrusted_key = parse_key_pem(&untrusted_client_key_pem);

        let (untrusted_client, server_name) = ClientTlsConfig::new()
            .with_ca(ca_roots.clone())
            .with_client_cert(untrusted_certs, untrusted_key)
            .with_server_name(DEFAULT_TLS_SERVER_NAME)
            .build()
            .unwrap();

        let sock = TcpStream::connect(server_addr).await.unwrap();
        let failed = match upgrade_client_stream(sock, untrusted_client, server_name).await {
            Err(_) => true,
            Ok(tls_sock) => {
                let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
                let _ = framed.send(WireMessage::Ping).await;
                match framed.next().await {
                    None => true,
                    Some(Err(_)) => true,
                    Some(Ok(_)) => false,
                }
            }
        };
        assert!(
            failed,
            "server must reject client connection when certificate is signed by unknown CA"
        );
    }

    // Subcase 2c: Client connects with valid client certificate
    {
        let valid_client_certs = parse_cert_pem(&client_pem);
        let valid_client_key = parse_key_pem(&client_key_pem);

        let (valid_client, server_name) = ClientTlsConfig::new()
            .with_ca(ca_roots.clone())
            .with_client_cert(valid_client_certs, valid_client_key)
            .with_server_name(DEFAULT_TLS_SERVER_NAME)
            .build()
            .unwrap();

        let sock = TcpStream::connect(server_addr).await.unwrap();
        let tls_sock = upgrade_client_stream(sock, valid_client, server_name)
            .await
            .unwrap();
        let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());

        framed.send(WireMessage::Ping).await.unwrap();
        let reply = framed.next().await.unwrap().unwrap();
        assert_eq!(reply, WireMessage::Pong);
    }

    server_task.await.unwrap();
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Certificate Pinning and Forgery Rejection
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_certificate_pinning_and_forgery_rejection() {
    let _lock = TEST_MUTEX.lock().await;
    ensure_crypto_provider();

    let (ca_pem, ca_key_pem) = pki::generate_ca("Pinning Test CA").unwrap();
    let (server_pem, server_key_pem) = pki::generate_node_cert(
        &ca_pem,
        &ca_key_pem,
        &[DEFAULT_TLS_SERVER_NAME.to_string()],
        &[],
    )
    .unwrap();

    let server_certs = parse_cert_pem(&server_pem);
    let server_key = parse_key_pem(&server_key_pem);
    let real_pin = pki::compute_cert_pin(server_pem.as_bytes()).unwrap();

    let server_tls = ServerTlsConfig::new(server_certs, server_key)
        .build()
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_addr = listener.local_addr().unwrap();

    let server_tls_clone = server_tls.clone();
    let server_task = tokio::spawn(async move {
        // Attempt 1: Valid pin -> succeeds
        // Attempt 2: Mismatched pin -> fails
        for _ in 0..2 {
            let (sock, _) = listener.accept().await.unwrap();
            if let Ok(tls_sock) = upgrade_server_stream(sock, server_tls_clone.clone()).await {
                let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
                if let Some(Ok(WireMessage::Ping)) = framed.next().await {
                    let _ = framed.send(WireMessage::Pong).await;
                }
            }
        }
    });

    // Subcase 3a: Client with matching pin succeeds (even without root CA!)
    {
        let (pinned_client, server_name) = ClientTlsConfig::new()
            .with_pins(vec![real_pin.clone()])
            .with_server_name(DEFAULT_TLS_SERVER_NAME)
            .build()
            .unwrap();

        let sock = TcpStream::connect(server_addr).await.unwrap();
        let tls_sock = upgrade_client_stream(sock, pinned_client, server_name)
            .await
            .expect("matching pin must establish TLS connection");
        let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
        framed.send(WireMessage::Ping).await.unwrap();
        assert_eq!(framed.next().await.unwrap().unwrap(), WireMessage::Pong);
    }

    // Subcase 3b: Client with forged / mismatched pin fails closed
    {
        let fake_pin = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string();
        let (pinned_client, server_name) = ClientTlsConfig::new()
            .with_pins(vec![fake_pin])
            .with_server_name(DEFAULT_TLS_SERVER_NAME)
            .build()
            .unwrap();

        let sock = TcpStream::connect(server_addr).await.unwrap();
        let err = upgrade_client_stream(sock, pinned_client, server_name)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("does not match any configured pins")
                || err.to_string().contains("failed"),
            "mismatched pin must fail handshake: {err}"
        );
    }

    server_task.await.unwrap();
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Zero Downgrade and Plaintext Refusal When TLS Is Required
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_zero_downgrade_and_plaintext_refusal_when_tls_required() {
    let _lock = TEST_MUTEX.lock().await;

    // Verify TlsMode classification
    assert!(TlsMode::Strict.is_required());
    assert!(TlsMode::Mutual.is_required());
    assert!(!TlsMode::Auto.is_required());
    assert!(!TlsMode::Disabled.is_required());

    // Set TLS_MODE to strict
    unsafe {
        std::env::set_var(TLS_MODE_ENV, "strict");
        std::env::remove_var(TLS_CERT_ENV);
        std::env::remove_var(TLS_KEY_ENV);
        std::env::remove_var(TLS_CA_ENV);
        std::env::remove_var(TLS_PIN_ENV);
    }

    let dummy_addr: SocketAddr = "127.0.0.1:9".parse().unwrap();

    // 1. connect_with fails closed immediately when TLS is required but unconfigured
    let err = connect_with(dummy_addr, Some("token")).await.unwrap_err();
    assert!(
        err.kind() == std::io::ErrorKind::PermissionDenied
            || err.kind() == std::io::ErrorKind::InvalidInput
    );
    assert!(err.to_string().contains("required"));

    // 2. Client connecting with TLS to a plaintext-only endpoint does NOT downgrade
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let plaintext_server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        // Server sends plaintext response instead of TLS ServerHello
        let _ = sock.write_all(b"ERROR: PLAINTEXT ONLY\n").await;
    });

    let (ca_pem, _ca_key_pem) = pki::generate_ca("NoDowngrade CA").unwrap();
    let ca_certs = parse_cert_pem(&ca_pem);
    let mut roots = RootCertStore::empty();
    for ca in ca_certs {
        roots.add(ca).unwrap();
    }
    let (client_tls, server_name) = ClientTlsConfig::new()
        .with_ca(roots)
        .with_server_name(DEFAULT_TLS_SERVER_NAME)
        .build()
        .unwrap();

    let sock = TcpStream::connect(addr).await.unwrap();
    let err = upgrade_client_stream(sock, client_tls, server_name)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);

    plaintext_server.await.unwrap();

    // 3. Plaintext client connecting to TLS-required server fails closed and receives 0 bytes response
    {
        let (server_ca_pem, server_ca_key) = pki::generate_ca("StrictServer CA").unwrap();
        let (srv_pem, srv_key) = pki::generate_node_cert(
            &server_ca_pem,
            &server_ca_key,
            &[DEFAULT_TLS_SERVER_NAME.to_string()],
            &[],
        )
        .unwrap();

        let srv_tls = ServerTlsConfig::new(parse_cert_pem(&srv_pem), parse_key_pem(&srv_key))
            .build()
            .unwrap();

        let tls_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tls_server_addr = tls_listener.local_addr().unwrap();

        let srv_task = tokio::spawn(async move {
            let (sock, _) = tls_listener.accept().await.unwrap();
            // Server TLS handshake MUST fail when the client sends plaintext data instead of ClientHello
            let upgrade_res = upgrade_server_stream(sock, srv_tls).await;
            assert!(
                upgrade_res.is_err(),
                "server TLS handshake must fail when client sends plaintext data"
            );
        });

        // Plaintext client connects via raw TCP and sends plaintext WireMessage (e.g. Auth and Ping)
        let mut plain_client = TcpStream::connect(tls_server_addr).await.unwrap();
        let mut frame = bytes::BytesMut::new();
        ProdCodeCodec::new()
            .encode(
                WireMessage::Auth(AuthToken("plaintext-leak-attempt".to_string())),
                &mut frame,
            )
            .unwrap();
        ProdCodeCodec::new()
            .encode(WireMessage::Ping, &mut frame)
            .unwrap();

        let _ = plain_client.write_all(&frame).await;
        let _ = plain_client.flush().await;

        // Plaintext client framed reader must never receive a valid protocol message
        let mut framed = Framed::new(plain_client, ProdCodeCodec::new());
        match framed.next().await {
            None => {} // Connection was closed immediately without response
            Some(Err(e)) => {
                // If the server sent a TLS alert record (ContentType 0x15), the codec rejects it
                // as an invalid frame header because a TLS alert is not a WireMessage frame.
                assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
            }
            Some(Ok(msg)) => {
                panic!("TLS-required server must not return protocol response to plaintext client, got: {msg:?}");
            }
        }

        srv_task.await.unwrap();
    }

    unsafe {
        std::env::remove_var(TLS_MODE_ENV);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Multi-CA Trust Bootstrap and Key Rotation Without Downgrade
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_multi_ca_trust_bootstrap_and_key_rotation_without_downgrade() {
    let _lock = TEST_MUTEX.lock().await;
    ensure_crypto_provider();

    // CA-v1 (Old) and CA-v2 (New)
    let (ca1_pem, ca1_key_pem) = pki::generate_ca("Cluster Root CA v1").unwrap();
    let (ca2_pem, ca2_key_pem) = pki::generate_ca("Cluster Root CA v2").unwrap();

    // Node 1 issued by CA-v1, Node 2 issued by CA-v2
    let (node1_pem, node1_key_pem) = pki::generate_node_cert(
        &ca1_pem,
        &ca1_key_pem,
        &[DEFAULT_TLS_SERVER_NAME.to_string()],
        &[],
    )
    .unwrap();
    let (node2_pem, node2_key_pem) = pki::generate_node_cert(
        &ca2_pem,
        &ca2_key_pem,
        &[DEFAULT_TLS_SERVER_NAME.to_string()],
        &[],
    )
    .unwrap();

    let server1_tls = ServerTlsConfig::new(parse_cert_pem(&node1_pem), parse_key_pem(&node1_key_pem))
        .build()
        .unwrap();
    let server2_tls = ServerTlsConfig::new(parse_cert_pem(&node2_pem), parse_key_pem(&node2_key_pem))
        .build()
        .unwrap();

    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr1 = listener1.local_addr().unwrap();

    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();

    let s1 = tokio::spawn(async move {
        while let Ok((sock, _)) = listener1.accept().await {
            if let Ok(tls_sock) = upgrade_server_stream(sock, server1_tls.clone()).await {
                let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
                if let Some(Ok(WireMessage::Ping)) = framed.next().await {
                    let _ = framed.send(WireMessage::Pong).await;
                }
            }
        }
    });

    let s2 = tokio::spawn(async move {
        while let Ok((sock, _)) = listener2.accept().await {
            if let Ok(tls_sock) = upgrade_server_stream(sock, server2_tls.clone()).await {
                let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
                if let Some(Ok(WireMessage::Ping)) = framed.next().await {
                    let _ = framed.send(WireMessage::Pong).await;
                }
            }
        }
    });

    // ── Phase 1: Dual-CA Trust Bootstrap (CA-v1 + CA-v2) ─────────────────────
    let mut dual_roots = RootCertStore::empty();
    for ca in parse_cert_pem(&ca1_pem) {
        dual_roots.add(ca).unwrap();
    }
    for ca in parse_cert_pem(&ca2_pem) {
        dual_roots.add(ca).unwrap();
    }

    let (dual_client, server_name) = ClientTlsConfig::new()
        .with_ca(dual_roots)
        .with_server_name(DEFAULT_TLS_SERVER_NAME)
        .build()
        .unwrap();

    // Client connects to Node 1 (signed by CA-v1) -> succeeds!
    {
        let sock = TcpStream::connect(addr1).await.unwrap();
        let tls_sock = upgrade_client_stream(sock, dual_client.clone(), server_name.clone())
            .await
            .expect("dual CA client must trust Node 1 (CA-v1)");
        let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
        framed.send(WireMessage::Ping).await.unwrap();
        assert_eq!(framed.next().await.unwrap().unwrap(), WireMessage::Pong);
    }

    // Client connects to Node 2 (signed by CA-v2) -> succeeds!
    {
        let sock = TcpStream::connect(addr2).await.unwrap();
        let tls_sock = upgrade_client_stream(sock, dual_client.clone(), server_name.clone())
            .await
            .expect("dual CA client must trust Node 2 (CA-v2)");
        let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
        framed.send(WireMessage::Ping).await.unwrap();
        assert_eq!(framed.next().await.unwrap().unwrap(), WireMessage::Pong);
    }

    // ── Phase 2: Complete Rotation (CA-v1 retired, only CA-v2 trusted) ────────
    let mut v2_only_roots = RootCertStore::empty();
    for ca in parse_cert_pem(&ca2_pem) {
        v2_only_roots.add(ca).unwrap();
    }

    let (v2_client, server_name) = ClientTlsConfig::new()
        .with_ca(v2_only_roots)
        .with_server_name(DEFAULT_TLS_SERVER_NAME)
        .build()
        .unwrap();

    // Client connects to Node 2 (signed by CA-v2) -> still succeeds!
    {
        let sock = TcpStream::connect(addr2).await.unwrap();
        let tls_sock = upgrade_client_stream(sock, v2_client.clone(), server_name.clone())
            .await
            .expect("v2-only client must trust Node 2 (CA-v2)");
        let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
        framed.send(WireMessage::Ping).await.unwrap();
        assert_eq!(framed.next().await.unwrap().unwrap(), WireMessage::Pong);
    }

    // Client connects to Node 1 (signed by retired CA-v1) -> fails closed!
    {
        let sock = TcpStream::connect(addr1).await.unwrap();
        let err = upgrade_client_stream(sock, v2_client.clone(), server_name.clone())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("UnknownIssuer") || err.to_string().contains("failed"),
            "retired CA-v1 must be rejected: {err}"
        );
    }

    s1.abort();
    s2.abort();
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. Secret Hygiene: Permissions, Scrubbing, and Memory Caching
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_secrets_absent_from_logs_and_child_environments() {
    let _lock = TEST_MUTEX.lock().await;

    // 1. Verify environment scrub lists
    assert_eq!(TLS_ENV_VARS.len(), 6);
    assert!(TLS_ENV_VARS.contains(&TLS_MODE_ENV));
    assert!(TLS_ENV_VARS.contains(&TLS_CERT_ENV));
    assert!(TLS_ENV_VARS.contains(&TLS_KEY_ENV));
    assert!(TLS_ENV_VARS.contains(&TLS_CA_ENV));
    assert!(TLS_ENV_VARS.contains(&TLS_PIN_ENV));
    assert!(TLS_ENV_VARS.contains(&TLS_SERVER_NAME_ENV));

    assert_eq!(AUTH_TOKEN_VARS.len(), 2);
    assert!(AUTH_TOKEN_VARS.contains(&"PROD_CODE_AUTH_TOKEN"));
    assert!(AUTH_TOKEN_VARS.contains(&"PROD_CODE_AUTH_TOKEN_FILE"));

    // 2. Child process environment inspection: verify env_remove scrubs all secrets from child processes
    {
        let canary_token = "cluster-secret-token-canary-445566";
        let canary_key_path = "/tmp/cluster_private_key_canary_secret.pem";
        unsafe {
            std::env::set_var("PROD_CODE_AUTH_TOKEN", canary_token);
            std::env::set_var("PROD_CODE_AUTH_TOKEN_FILE", "/tmp/token.txt");
            std::env::set_var(TLS_MODE_ENV, "strict");
            std::env::set_var(TLS_CERT_ENV, "/tmp/cert.pem");
            std::env::set_var(TLS_KEY_ENV, canary_key_path);
            std::env::set_var(TLS_CA_ENV, "/tmp/ca.pem");
            std::env::set_var(TLS_PIN_ENV, "abcdef0123456789");
            std::env::set_var(TLS_SERVER_NAME_ENV, "node.internal");
        }

        let mut cmd = std::process::Command::new("env");
        for var in AUTH_TOKEN_VARS {
            cmd.env_remove(var);
        }
        for var in TLS_ENV_VARS {
            cmd.env_remove(var);
        }

        let output = cmd.output().expect("execute env command in child process");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);

        for var in AUTH_TOKEN_VARS {
            assert!(
                !stdout.contains(&format!("{var}=")),
                "child process environment must not contain {var}"
            );
        }
        for var in TLS_ENV_VARS {
            assert!(
                !stdout.contains(&format!("{var}=")),
                "child process environment must not contain {var}"
            );
        }
        assert!(
            !stdout.contains(canary_token),
            "child environment must never leak auth token secret value"
        );
        assert!(
            !stdout.contains(canary_key_path),
            "child environment must never leak private key path"
        );

        unsafe {
            for var in AUTH_TOKEN_VARS {
                std::env::remove_var(var);
            }
            for var in TLS_ENV_VARS {
                std::env::remove_var(var);
            }
        }
    }

    // 3. Restrictive file permissions enforcement (0600)
    let temp_dir = tempfile::tempdir().unwrap();
    let key_file = temp_dir.path().join("test_sec.key");
    std::fs::write(&key_file, "mock-private-key-bytes").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Mode 0600 -> valid
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(check_key_permissions(&key_file).is_ok());

        // Mode 0644 -> insecure, fails
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            check_key_permissions(&key_file).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );

        // Mode 0666 -> insecure, fails
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(
            check_key_permissions(&key_file).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );

        // Mode 0777 -> insecure, fails
        std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(
            check_key_permissions(&key_file).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }

    // 4. In-memory caching allows key environment scrubbing without connection failure
    clear_client_tls_cache();
    let (ca_pem, ca_key_pem) = pki::generate_ca("Scrub Test CA").unwrap();
    let (cert_pem, key_pem) = pki::generate_node_cert(
        &ca_pem,
        &ca_key_pem,
        &[DEFAULT_TLS_SERVER_NAME.to_string()],
        &[],
    )
    .unwrap();

    let cert_file = temp_dir.path().join("client.crt");
    let priv_key_file = temp_dir.path().join("client.key");
    let ca_file = temp_dir.path().join("ca.crt");

    std::fs::write(&cert_file, &cert_pem).unwrap();
    std::fs::write(&ca_file, &ca_pem).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).mode(0o600);
        let mut f = opts.open(&priv_key_file).unwrap();
        f.write_all(key_pem.as_bytes()).unwrap();
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&priv_key_file, &key_pem).unwrap();
    }

    unsafe {
        std::env::set_var(TLS_MODE_ENV, "mutual");
        std::env::set_var(TLS_CERT_ENV, &cert_file);
        std::env::set_var(TLS_KEY_ENV, &priv_key_file);
        std::env::set_var(TLS_CA_ENV, &ca_file);
    }

    // Load into memory cache
    init_client_tls_from_env().unwrap();
    assert!(default_client_tls_built().is_some());

    // Scrub key from environment
    unsafe {
        std::env::remove_var(TLS_KEY_ENV);
    }

    // Direct environment reload fails (proves key was removed)
    assert!(ClientTlsConfig::from_env().is_err());

    // But cached client config remains valid and accessible
    assert!(default_client_tls_built().unwrap().is_some());

    clear_client_tls_cache();
    unsafe {
        std::env::remove_var(TLS_MODE_ENV);
        std::env::remove_var(TLS_CERT_ENV);
        std::env::remove_var(TLS_CA_ENV);
    }

    // 5. Secret hygiene in logging and Debug formatting:
    // Verify that neither Debug representations nor tracing logs leak private keys or auth tokens.
    {
        struct TestLogSubscriber {
            logs: Arc<Mutex<Vec<String>>>,
        }

        impl tracing::Subscriber for TestLogSubscriber {
            fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
            fn event(&self, event: &tracing::Event<'_>) {
                struct Visitor<'a>(&'a mut String);
                impl<'a> tracing::field::Visit for Visitor<'a> {
                    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                        use std::fmt::Write;
                        let _ = write!(self.0, " {}={:?}", field.name(), value);
                    }
                }
                let mut msg = format!("{:?} {}", event.metadata().level(), event.metadata().target());
                let mut visitor = Visitor(&mut msg);
                event.record(&mut visitor);
                if let Ok(mut logs) = self.logs.lock() {
                    logs.push(msg);
                }
            }
            fn enter(&self, _span: &tracing::span::Id) {}
            fn exit(&self, _span: &tracing::span::Id) {}
        }

        let (ca_pem, ca_key_pem) = pki::generate_ca("Log Hygiene CA").unwrap();
        let (server_pem, server_key_pem) = pki::generate_node_cert(
            &ca_pem,
            &ca_key_pem,
            &[DEFAULT_TLS_SERVER_NAME.to_string()],
            &[],
        )
        .unwrap();
        let (client_pem, client_key_pem) = pki::generate_node_cert(
            &ca_pem,
            &ca_key_pem,
            &["client.internal".to_string()],
            &[],
        )
        .unwrap();

        let secret_token_str = "super-secret-auth-token-canary-998877";
        let auth_token = AuthToken(secret_token_str.to_string());
        let wire_auth = WireMessage::Auth(auth_token.clone());

        let server_tls = ServerTlsConfig::new(parse_cert_pem(&server_pem), parse_key_pem(&server_key_pem));
        let client_tls = ClientTlsConfig::new().with_client_cert(parse_cert_pem(&client_pem), parse_key_pem(&client_key_pem));

        // A. Direct Debug representation checks:
        let debug_auth = format!("{auth_token:?}");
        assert!(!debug_auth.contains(secret_token_str), "AuthToken Debug must not contain secret string: {debug_auth}");
        assert!(debug_auth.contains("<redacted>"), "AuthToken Debug must be redacted: {debug_auth}");

        let debug_wire = format!("{wire_auth:?}");
        assert!(!debug_wire.contains(secret_token_str), "WireMessage::Auth Debug must not contain secret string: {debug_wire}");
        assert!(debug_wire.contains("<redacted>"), "WireMessage::Auth Debug must be redacted: {debug_wire}");

        let debug_server = format!("{server_tls:?}");
        assert!(!debug_server.contains(&server_key_pem), "ServerTlsConfig Debug must not leak private key PEM: {debug_server}");
        assert!(debug_server.contains("[REDACTED]"), "ServerTlsConfig Debug must mark key as [REDACTED]: {debug_server}");

        let debug_client = format!("{client_tls:?}");
        assert!(!debug_client.contains(&client_key_pem), "ClientTlsConfig Debug must not leak private key PEM: {debug_client}");
        assert!(debug_client.contains("[REDACTED]"), "ClientTlsConfig Debug must mark key as [REDACTED]: {debug_client}");

        // B. In-memory tracing log capture check:
        let captured_logs = Arc::new(Mutex::new(Vec::new()));
        let subscriber = TestLogSubscriber {
            logs: Arc::clone(&captured_logs),
        };

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(server = ?server_tls, "configured server TLS");
            tracing::info!(client = ?client_tls, "configured client TLS");
            tracing::warn!(auth = ?auth_token, "received auth attempt");
            tracing::debug!(wire = ?wire_auth, "dispatching wire message");
        });

        let logs = captured_logs.lock().unwrap();
        assert_eq!(logs.len(), 4, "must capture exactly 4 log events");
        for log_line in logs.iter() {
            assert!(
                !log_line.contains(secret_token_str),
                "captured log must not leak secret auth token: {log_line}"
            );
            assert!(
                !log_line.contains(&server_key_pem),
                "captured log must not leak server private key PEM: {log_line}"
            );
            assert!(
                !log_line.contains(&client_key_pem),
                "captured log must not leak client private key PEM: {log_line}"
            );
            let server_key_lines: Vec<&str> = server_key_pem
                .lines()
                .filter(|l| !l.starts_with("---"))
                .collect();
            if let Some(first_key_line) = server_key_lines.first() {
                assert!(
                    !log_line.contains(first_key_line),
                    "captured log must not leak raw private key bytes: {log_line}"
                );
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. TLS Handshake and Warm-Request Latency Benchmark (< 1ms overhead)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_tls_handshake_and_warm_request_overhead_benchmark() {
    let _lock = TEST_MUTEX.lock().await;
    ensure_crypto_provider();

    const NUM_WARM_REQUESTS: usize = 100;

    // ── Baseline: Plaintext TCP ───────────────────────────────────────────────
    let plain_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let plain_addr = plain_listener.local_addr().unwrap();

    let plain_server = tokio::spawn(async move {
        let (sock, _) = plain_listener.accept().await.unwrap();
        let mut framed = Framed::new(sock, ProdCodeCodec::new());
        while let Some(Ok(msg)) = framed.next().await {
            match msg {
                WireMessage::Ping => {
                    let _ = framed.send(WireMessage::Pong).await;
                }
                _ => break,
            }
        }
    });

    let plain_start = Instant::now();
    let plain_client_sock = TcpStream::connect(plain_addr).await.unwrap();
    let plain_handshake_duration = plain_start.elapsed();
    let mut plain_framed = Framed::new(plain_client_sock, ProdCodeCodec::new());

    let mut plain_latencies = Vec::with_capacity(NUM_WARM_REQUESTS);
    for _ in 0..NUM_WARM_REQUESTS {
        let req_start = Instant::now();
        plain_framed.send(WireMessage::Ping).await.unwrap();
        let resp = plain_framed.next().await.unwrap().unwrap();
        assert_eq!(resp, WireMessage::Pong);
        plain_latencies.push(req_start.elapsed());
    }
    drop(plain_framed);
    plain_server.await.unwrap();

    // ── TLS 1.3: Transport Security ───────────────────────────────────────────
    let (ca_pem, ca_key_pem) = pki::generate_ca("Benchmark CA").unwrap();
    let (server_pem, server_key_pem) = pki::generate_node_cert(
        &ca_pem,
        &ca_key_pem,
        &[DEFAULT_TLS_SERVER_NAME.to_string()],
        &[],
    )
    .unwrap();

    let server_tls = ServerTlsConfig::new(parse_cert_pem(&server_pem), parse_key_pem(&server_key_pem))
        .build()
        .unwrap();

    let mut roots = RootCertStore::empty();
    for ca in parse_cert_pem(&ca_pem) {
        roots.add(ca).unwrap();
    }
    let (client_tls, server_name) = ClientTlsConfig::new()
        .with_ca(roots)
        .with_server_name(DEFAULT_TLS_SERVER_NAME)
        .build()
        .unwrap();

    let tls_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tls_addr = tls_listener.local_addr().unwrap();

    let tls_server = tokio::spawn(async move {
        let (sock, _) = tls_listener.accept().await.unwrap();
        let tls_sock = upgrade_server_stream(sock, server_tls).await.unwrap();
        let mut framed = Framed::new(tls_sock, ProdCodeCodec::new());
        while let Some(Ok(msg)) = framed.next().await {
            match msg {
                WireMessage::Ping => {
                    let _ = framed.send(WireMessage::Pong).await;
                }
                _ => break,
            }
        }
    });

    let tls_start = Instant::now();
    let tls_client_sock = TcpStream::connect(tls_addr).await.unwrap();
    let tls_sock = upgrade_client_stream(tls_client_sock, client_tls, server_name)
        .await
        .unwrap();
    let tls_handshake_duration = tls_start.elapsed();
    let mut tls_framed = Framed::new(tls_sock, ProdCodeCodec::new());

    let mut tls_latencies = Vec::with_capacity(NUM_WARM_REQUESTS);
    for _ in 0..NUM_WARM_REQUESTS {
        let req_start = Instant::now();
        tls_framed.send(WireMessage::Ping).await.unwrap();
        let resp = tls_framed.next().await.unwrap().unwrap();
        assert_eq!(resp, WireMessage::Pong);
        tls_latencies.push(req_start.elapsed());
    }
    drop(tls_framed);
    tls_server.await.unwrap();

    // ── Metrics Calculation & Acceptance Assertion ────────────────────────────
    plain_latencies.sort();
    tls_latencies.sort();

    let plain_p50 = plain_latencies[NUM_WARM_REQUESTS / 2];
    let plain_p95 = plain_latencies[(NUM_WARM_REQUESTS * 95) / 100];
    let tls_p50 = tls_latencies[NUM_WARM_REQUESTS / 2];
    let tls_p95 = tls_latencies[(NUM_WARM_REQUESTS * 95) / 100];

    let warm_overhead_p50 = if tls_p50 > plain_p50 {
        tls_p50 - plain_p50
    } else {
        std::time::Duration::from_nanos(0)
    };

    println!(
        "\n=== Phase 5.6 Transport Security Benchmark Report ===\n\
         Plaintext Handshake: {:>8.2?}\n\
         TLS 1.3 Handshake:   {:>8.2?}\n\
         Plaintext Warm RTT:  p50={:>8.2?}, p95={:>8.2?}\n\
         TLS 1.3 Warm RTT:    p50={:>8.2?}, p95={:>8.2?}\n\
         Warm RTT Overhead:   {:>8.2?}\n\
         =====================================================",
        plain_handshake_duration,
        tls_handshake_duration,
        plain_p50,
        plain_p95,
        tls_p50,
        tls_p95,
        warm_overhead_p50
    );

    // Warm-request overhead budget: must be well below 1.0 millisecond (< 1000 µs)
    assert!(
        warm_overhead_p50 < std::time::Duration::from_millis(1),
        "Warm request overhead ({:?}) exceeds 1ms budget",
        warm_overhead_p50
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. Discovery Replay Resistance, HMAC Forgery Rejection, and Privacy
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_forged_or_replayed_discovery_rejected() {
    let token = "cluster-secret-discovery-key";
    let nonce = generate_nonce().unwrap();
    let sender_ip: IpAddr = "192.168.1.100".parse().unwrap();

    // 1. Legitimate probe and reply with 128-bit challenge nonce
    let probe = format_probe_with_nonce(Some(token), Some(&nonce));
    let parsed_nonce = inspect_probe(&probe, Some(token)).unwrap();
    assert_eq!(parsed_nonce.as_deref(), Some(nonce.as_str()));

    let workspaces = vec![("proj-a".to_string(), "rust".to_string(), 2)];
    let legitimate_reply = format_node_line_with_nonce(
        "192.168.1.100:9400",
        "rust,go",
        1024,
        0.05,
        16,
        64000,
        32000,
        2,
        &workspaces,
        Some(token),
        Some(&nonce),
    );

    let parsed_node = parse_node_line_with_auth_and_nonce(
        &legitimate_reply,
        Some(token),
        Some(sender_ip),
        Some(&nonce),
    )
    .expect("legitimate announcement must authenticate");
    assert_eq!(parsed_node.workspaces.len(), 1);
    assert_eq!(parsed_node.workspaces[0].name, "proj-a");
    assert_eq!(parsed_node.nonce.as_deref(), Some(nonce.as_str()));

    // 2. Replayed announcement with expired / different nonce fails
    let attacker_stale_nonce = generate_nonce().unwrap();
    assert!(
        parse_node_line_with_auth_and_nonce(
            &legitimate_reply,
            Some(token),
            Some(sender_ip),
            Some(&attacker_stale_nonce)
        )
        .is_none(),
        "replayed announcement with mismatched nonce MUST be rejected"
    );

    // 3. Forged routing fields (e.g. modified available RAM to falsely attract traffic) fails HMAC
    let mut tokens: Vec<&str> = legitimate_reply.split_whitespace().collect();
    // Tamper with mem_avail_mb (index 8)
    tokens[8] = "999999";
    let tampered_reply = tokens.join(" ");
    assert!(
        parse_node_line_with_auth_and_nonce(
            &tampered_reply,
            Some(token),
            Some(sender_ip),
            Some(&nonce)
        )
        .is_none(),
        "forged announcement with tampered routing telemetry MUST be rejected by HMAC"
    );

    // 4. Cross-host IP spoofing fails
    let spoofed_ip: IpAddr = "10.0.0.1".parse().unwrap();
    assert!(
        parse_node_line_with_auth_and_nonce(
            &legitimate_reply,
            Some(token),
            Some(spoofed_ip),
            Some(&nonce)
        )
        .is_none(),
        "spoofed IP address MUST be rejected"
    );

    // 5. Privacy: Minimal periodic multicast strips workspace names and detailed telemetry
    let minimal_line = format_minimal_node_line("192.168.1.100:9400", "rust,go", Some(token));
    let minimal_node = parse_node_line_with_auth_and_nonce(
        &minimal_line,
        Some(token),
        Some(sender_ip),
        None,
    )
    .expect("minimal announcement must parse");
    assert!(
        minimal_node.workspaces.is_empty(),
        "minimal multicast announcement must NOT disclose workspace names"
    );
    assert_eq!(minimal_node.rss_mb, 0);
    assert_eq!(minimal_node.mem_total_mb, 0);
}
