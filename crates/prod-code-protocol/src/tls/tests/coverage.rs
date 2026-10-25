/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::tls::{client::ClientTlsConfig, server::ServerTlsConfig, types::*};
use std::process::Command;

#[test]
fn isolated_environment_configuration_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let report = crate::tls::pki::init_cluster_pki(dir.path(), Some("example.test"), &[]).unwrap();
    for (mode, cert, key, ca, server, client) in [
        ("disabled", false, false, false, "none", "none"),
        ("0", false, false, false, "none", "none"),
        (" FALSE ", false, false, false, "none", "none"),
        ("auto", false, false, false, "none", "none"),
        ("auto", true, false, false, "key_missing", "none"),
        ("auto", false, true, false, "none", "none"),
        ("strict", false, false, false, "required", "required"),
        ("1", false, false, false, "required", "required"),
        ("TRUE", false, false, false, "required", "required"),
        ("mutual", true, true, false, "ca_missing", "some"),
        ("mtls", false, false, false, "required", "mutual_missing"),
        ("strict", true, true, false, "some", "some"),
        ("mutual", true, true, true, "some", "some"),
        ("auto", true, true, true, "some", "some"),
        ("unset", false, false, false, "none", "none"),
        ("unset", true, true, true, "some", "some"),
    ] {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "tls::tests::coverage::environment_child",
            "--nocapture",
        ])
        .env("COVERAGE_TLS_CHILD", "1")
        .env("EXPECT_SERVER", server)
        .env("EXPECT_CLIENT", client)
        .env("HOME", dir.path().join("empty-home"))
        .env_remove("USERPROFILE");
        for name in TLS_ENV_VARS {
            cmd.env_remove(name);
        }
        if mode != "unset" {
            cmd.env(TLS_MODE_ENV, mode);
        }
        if cert {
            cmd.env(TLS_CERT_ENV, &report.node_cert_path);
        }
        if key {
            cmd.env(TLS_KEY_ENV, &report.node_key_path);
        }
        if ca {
            cmd.env(TLS_CA_ENV, &report.ca_cert_path);
        }
        cmd.env(TLS_SERVER_NAME_ENV, " example.test ");
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{mode}/{cert}/{key}/{ca}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn environment_child() {
    if std::env::var_os("COVERAGE_TLS_CHILD").is_none() {
        return;
    }
    let mode = TlsMode::from_env().unwrap();
    assert_eq!(
        mode.is_required(),
        matches!(mode, TlsMode::Strict | TlsMode::Mutual)
    );
    assert_eq!(mode.requires_client_cert(), mode == TlsMode::Mutual);
    let server = ServerTlsConfig::from_env();
    match std::env::var("EXPECT_SERVER").unwrap().as_str() {
        "none" => assert!(server.unwrap().is_none()),
        "some" => {
            let config = server.unwrap().unwrap();
            assert_eq!(config.certs.len(), 1);
            assert_eq!(config.require_client_auth, mode == TlsMode::Mutual);
            let cloned = config.clone();
            assert_eq!(cloned.certs, config.certs);
            assert_eq!(cloned.key.secret_der(), config.key.secret_der());
            let debug = format!("{config:?}");
            assert!(debug.contains("key: \"[REDACTED]\""), "{debug}");
            assert!(debug.contains("certs_count: 1"), "{debug}");
            let built = cloned.build().unwrap();
            assert_eq!(built.max_early_data_size, 0);
            let _acceptor = config.build_acceptor().unwrap();
        }
        expected => {
            let err = server.unwrap_err();
            let message = match expected {
                "key_missing" => "'PROD_CODE_TLS_CERT' is set but 'PROD_CODE_TLS_KEY' is missing".to_string(),
                "ca_missing" => "Mutual TLS mode ('Mutual') requires 'PROD_CODE_TLS_CA' for verifying client certificates".to_string(),
                "required" => format!("TLS mode '{mode:?}' is required on server, but 'PROD_CODE_TLS_CERT' and/or 'PROD_CODE_TLS_KEY' are missing"),
                _ => panic!("unknown expectation"),
            };
            assert_eq!(err.to_string(), message);
            assert_eq!(
                err.kind(),
                if expected == "key_missing" {
                    std::io::ErrorKind::NotFound
                } else {
                    std::io::ErrorKind::InvalidInput
                }
            );
        }
    }
    let client = ClientTlsConfig::from_env();
    match std::env::var("EXPECT_CLIENT").unwrap().as_str() {
        "none" => assert!(client.unwrap().is_none()),
        "some" => {
            let config = client.unwrap().unwrap();
            assert_eq!(config.server_name, "example.test");
            assert_eq!(config.client_cert.as_ref().unwrap().0.len(), 1);
            let cloned = config.clone();
            assert_eq!(cloned.server_name, config.server_name);
            assert!(format!("{cloned:?}").contains("[1 certificate(s), key: [REDACTED]]"));
            let (_, name) = cloned.build().unwrap();
            assert_eq!(name.to_str(), "example.test");
        }
        expected => {
            let err = client.unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                if expected == "mutual_missing" {
                    "Mutual TLS mode ('Mutual') requires client 'PROD_CODE_TLS_CERT' and 'PROD_CODE_TLS_KEY'".to_string()
                } else {
                    format!(
                        "TLS mode '{mode:?}' is required, but neither 'PROD_CODE_TLS_CA', 'PROD_CODE_TLS_PIN', nor client credentials are configured"
                    )
                }
            );
        }
    }
}

#[test]
fn empty_client_roots_are_rejected_for_both_auth_modes() {
    for require in [false, true] {
        let (cert, key) = crate::tls::test_helpers::generate_test_self_signed("example.test");
        let error = ServerTlsConfig::new(vec![cert], key)
            .with_client_ca(rustls::RootCertStore::empty(), require)
            .build()
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "Invalid client verifier: no root trust anchors were provided"
        );
    }
}
