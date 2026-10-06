/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::runner::run_cert;
use super::types::CertCommands;

#[tokio::test]
async fn cert_cli_init_ca_node_pin_and_verify_cycle() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("tls");

    // 1. Run Init
    run_cert(CertCommands::Init {
        out_dir: out_dir.clone(),
        server_name: Some("cluster.internal".to_string()),
        ips: vec!["10.0.0.5".to_string()],
        json: true,
    })
    .await
    .unwrap();

    let ca_crt = out_dir.join("ca.crt");
    let ca_key = out_dir.join("ca.key");
    let node_crt = out_dir.join("node.crt");
    let node_key = out_dir.join("node.key");

    assert!(ca_crt.exists());
    assert!(ca_key.exists());
    assert!(node_crt.exists());
    assert!(node_key.exists());

    // 2. Read node pin
    let node_bytes = std::fs::read(&node_crt).unwrap();
    let pin = prod_code_protocol::tls::pki::compute_cert_pin(&node_bytes).unwrap();

    // 3. Verify valid
    run_cert(CertCommands::Verify {
        cert_file: node_crt.clone(),
        key_file: Some(node_key.clone()),
        ca_cert: Some(ca_crt.clone()),
        server_name: Some("cluster.internal".to_string()),
        pin: Some(pin.clone()),
    })
    .await
    .unwrap();

    // 4. Verify SAN IP match
    run_cert(CertCommands::Verify {
        cert_file: node_crt.clone(),
        key_file: None,
        ca_cert: Some(ca_crt.clone()),
        server_name: Some("10.0.0.5".to_string()),
        pin: None,
    })
    .await
    .unwrap();

    // 5. Verify SAN mismatch fails
    let err_san = run_cert(CertCommands::Verify {
        cert_file: node_crt.clone(),
        key_file: None,
        ca_cert: Some(ca_crt.clone()),
        server_name: Some("wrong.internal".to_string()),
        pin: None,
    })
    .await;
    assert!(err_san.is_err());

    // 6. Verify pin mismatch fails
    let err_pin = run_cert(CertCommands::Verify {
        cert_file: node_crt.clone(),
        key_file: None,
        ca_cert: None,
        server_name: None,
        pin: Some("0000000000000000000000000000000000000000000000000000000000000000".to_string()),
    })
    .await;
    assert!(err_pin.is_err());

    // 7. Verify standalone CA generation
    let ca_only_dir = temp.path().join("ca_only");
    run_cert(CertCommands::Ca {
        out_dir: ca_only_dir.clone(),
        common_name: "Standalone Root".to_string(),
    })
    .await
    .unwrap();
    assert!(ca_only_dir.join("ca.crt").exists());
    assert!(ca_only_dir.join("ca.key").exists());

    // 8. Verify standalone Node generation from standalone CA
    let node_only_dir = temp.path().join("node_only");
    run_cert(CertCommands::Node {
        ca_cert: ca_only_dir.join("ca.crt"),
        ca_key: ca_only_dir.join("ca.key"),
        out_dir: node_only_dir.clone(),
        prefix: "gateway".to_string(),
        dns: vec!["gateway.internal".to_string()],
        ips: vec!["127.0.0.1".to_string()],
    })
    .await
    .unwrap();
    assert!(node_only_dir.join("gateway.crt").exists());
    assert!(node_only_dir.join("gateway.key").exists());
}
