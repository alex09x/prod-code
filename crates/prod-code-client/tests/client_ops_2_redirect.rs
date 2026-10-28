/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_client::editor_files::{read_frame, write_frame};
use prod_code_protocol::{ProdCodeCodec, ServerTlsConfig, SyncProbeResponse, WireMessage};
use prod_code_testkit::Workspace;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::BufReader;
use tokio_util::codec::Framed;

#[tokio::test]
async fn authenticated_handshake_redirect_loop_is_refused_after_three_hops() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn sample() {}\n")]);
    let cache = tempfile::tempdir().unwrap();
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .signed_by(&key, &ca, &ca_key)
        .unwrap();
    let ca_path = cache.path().join("ca.pem");
    std::fs::write(&ca_path, ca.pem()).unwrap();
    let server_config = ServerTlsConfig::new(
        vec![rustls_pki_types::CertificateDer::from(cert.der().to_vec())],
        rustls_pki_types::PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(
            key.serialize_der(),
        )),
    )
    .build()
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let mut hops = vec![];
        for hop in 0..4 {
            let (socket, _) = listener.accept().await.unwrap();
            let tls = prod_code_protocol::tls::upgrade_server_stream(socket, server_config.clone())
                .await
                .unwrap();
            let mut framed = Framed::new(
                prod_code_protocol::AnyStream::TlsServer(tls),
                ProdCodeCodec::new(),
            );
            loop {
                match framed.next().await.unwrap().unwrap() {
                    WireMessage::SyncProbeRequest(req) => {
                        assert_eq!(
                            hop, 0,
                            "redirected workspaces must not sync before handshake"
                        );
                        framed
                            .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                                server_workspace_root: req.client_workspace_root,
                                seeded: false,
                                files_deleted: 0,
                                missing: vec![],
                            }))
                            .await
                            .unwrap();
                    }
                    WireMessage::HandshakeRequest(req) => {
                        hops.push((req.redirect_count, req.refused_by));
                        framed
                            .send(WireMessage::Redirect {
                                target_addr: addr.to_string(),
                                reason: Some("fixture loop".into()),
                            })
                            .await
                            .unwrap();
                        break;
                    }
                    other => panic!("unexpected handshake message {other:?}"),
                }
            }
        }
        hops
    });
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(addr.to_string())
        .args(["lsp", "--watchdog-secs", "0"])
        .env("HOME", cache.path())
        .env("XDG_CACHE_HOME", cache.path())
        .env("PROD_CODE_TLS_MODE", "auto")
        .env("PROD_CODE_TLS_CA", &ca_path)
        .env("PROD_CODE_TLS_SERVER_NAME", "localhost")
        .env_remove("PROD_CODE_TLS_PIN")
        .env_remove("PROD_CODE_TLS_CERT")
        .env_remove("PROD_CODE_TLS_KEY")
        .env_remove("PROD_CODE_AUTH_TOKEN")
        .env_remove("PROD_CODE_AUTH_TOKEN_FILE")
        .env_remove("PROD_CODE_CLUSTER")
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    write_frame(
        &mut stdin,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}).to_string(),
    )
    .await
    .unwrap();
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut stdout))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&reply).unwrap(),
        json!({"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"prod-code lsp could not start: too many gateway redirects: Some(\"fixture loop\")"}})
    );
    drop(stdin);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
            .await
            .unwrap()
            .unwrap()
            .code(),
        Some(0)
    );
    assert_eq!(
        peer.await.unwrap(),
        vec![
            (0, vec![]),
            (1, vec![addr.to_string()]),
            (2, vec![addr.to_string(), addr.to_string()]),
            (
                3,
                vec![addr.to_string(), addr.to_string(), addr.to_string()]
            ),
        ]
    );
}
