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
use std::path::{Path, PathBuf};

/// Runs `cert init` into `dir/tls` and returns the output directory.
async fn init_pki(dir: &Path, json: bool) -> PathBuf {
    let out_dir = dir.join("tls");
    run_cert(CertCommands::Init {
        out_dir: out_dir.clone(),
        server_name: None,
        ips: vec![],
        json,
    })
    .await
    .unwrap();
    out_dir
}

fn verify_cmd(cert: &Path) -> CertCommands {
    CertCommands::Verify {
        cert_file: cert.to_path_buf(),
        key_file: None,
        ca_cert: None,
        server_name: None,
        pin: None,
    }
}

fn node_cmd(ca: &Path, out_dir: &Path, dns: Vec<String>, ips: Vec<String>) -> CertCommands {
    CertCommands::Node {
        ca_cert: ca.join("ca.crt"),
        ca_key: ca.join("ca.key"),
        out_dir: out_dir.to_path_buf(),
        prefix: "node".to_string(),
        dns,
        ips,
    }
}

#[tokio::test]
async fn init_text_mode_uses_default_server_name_and_writes_four_files() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = init_pki(temp.path(), false).await;
    for name in ["ca.crt", "ca.key", "node.crt", "node.key"] {
        assert!(out_dir.join(name).is_file(), "{name}");
    }
    // The default SAN is the protocol's default TLS server name.
    run_cert(CertCommands::Verify {
        cert_file: out_dir.join("node.crt"),
        key_file: Some(out_dir.join("node.key")),
        ca_cert: Some(out_dir.join("ca.crt")),
        server_name: Some(prod_code_protocol::tls::DEFAULT_TLS_SERVER_NAME.to_string()),
        pin: None,
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn invalid_ip_addresses_are_refused_by_init_and_node() {
    let temp = tempfile::tempdir().unwrap();
    let err = run_cert(CertCommands::Init {
        out_dir: temp.path().join("never"),
        server_name: None,
        ips: vec!["10.0.0.1".to_string(), "300.1.1.1".to_string()],
        json: false,
    })
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "invalid IP address '300.1.1.1'");
    assert!(!temp.path().join("never").exists());

    let ca = init_pki(temp.path(), true).await;
    let err = run_cert(node_cmd(
        &ca,
        &temp.path().join("n"),
        vec![],
        vec!["not-an-ip".to_string()],
    ))
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "invalid IP address 'not-an-ip'");
}

#[tokio::test]
async fn node_refuses_missing_ca_files_with_the_path_in_the_message() {
    let temp = tempfile::tempdir().unwrap();
    let ca = init_pki(temp.path(), true).await;
    let out = temp.path().join("n");

    let mut cmd = node_cmd(&ca, &out, vec![], vec![]);
    if let CertCommands::Node { ca_cert, .. } = &mut cmd {
        *ca_cert = ca.join("absent.crt");
    }
    let err = run_cert(cmd).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "failed reading CA certificate from {}",
            ca.join("absent.crt").display()
        )
    );

    let mut cmd = node_cmd(&ca, &out, vec![], vec![]);
    if let CertCommands::Node { ca_key, .. } = &mut cmd {
        *ca_key = ca.join("absent.key");
    }
    let err = run_cert(cmd).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "failed reading CA private key from {}",
            ca.join("absent.key").display()
        )
    );
    assert!(!out.exists());
}

#[tokio::test]
async fn node_without_dns_names_gets_default_name_and_localhost() {
    let temp = tempfile::tempdir().unwrap();
    let ca = init_pki(temp.path(), true).await;
    let out = temp.path().join("n");
    run_cert(node_cmd(&ca, &out, vec![], vec!["192.0.2.7".to_string()]))
        .await
        .unwrap();
    for name in [
        prod_code_protocol::tls::DEFAULT_TLS_SERVER_NAME,
        "localhost",
        "192.0.2.7",
    ] {
        let mut cmd = verify_cmd(&out.join("node.crt"));
        if let CertCommands::Verify {
            ca_cert,
            server_name,
            ..
        } = &mut cmd
        {
            *ca_cert = Some(ca.join("ca.crt"));
            *server_name = Some(name.to_string());
        }
        run_cert(cmd)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}

#[tokio::test]
async fn pin_prints_for_valid_certificates_and_refuses_bad_input() {
    let temp = tempfile::tempdir().unwrap();
    let ca = init_pki(temp.path(), true).await;
    run_cert(CertCommands::Pin {
        cert_file: ca.join("node.crt"),
    })
    .await
    .unwrap();

    let missing = temp.path().join("missing.crt");
    let err = run_cert(CertCommands::Pin {
        cert_file: missing.clone(),
    })
    .await
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("failed reading certificate from {}", missing.display())
    );

    let junk = temp.path().join("junk.crt");
    std::fs::write(&junk, "not a certificate").unwrap();
    let err = run_cert(CertCommands::Pin { cert_file: junk })
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "no certificate found in PEM data");
}

#[tokio::test]
async fn verify_pin_accepts_colons_spaces_and_uppercase_and_names_both_on_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let ca = init_pki(temp.path(), true).await;
    let cert = ca.join("node.crt");
    let pin =
        prod_code_protocol::tls::pki::compute_cert_pin(&std::fs::read(&cert).unwrap()).unwrap();
    let pretty = pin
        .to_ascii_uppercase()
        .as_bytes()
        .chunks(2)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join(":");
    for expected in [pretty.clone(), pretty.replace(':', " "), pin.clone()] {
        let mut cmd = verify_cmd(&cert);
        if let CertCommands::Verify { pin, .. } = &mut cmd {
            *pin = Some(expected);
        }
        run_cert(cmd).await.unwrap();
    }

    let wrong = "AB:CD";
    let mut cmd = verify_cmd(&cert);
    if let CertCommands::Verify { pin, .. } = &mut cmd {
        *pin = Some(wrong.to_string());
    }
    let err = run_cert(cmd).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("SHA-256 pin mismatch: expected abcd, got {pin}")
    );
}

#[tokio::test]
async fn verify_refuses_unreadable_unparseable_and_untrusted_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let ca = init_pki(temp.path(), true).await;
    let other = {
        let dir = temp.path().join("other");
        std::fs::create_dir_all(&dir).unwrap();
        init_pki(&dir, true).await
    };

    let missing = temp.path().join("missing.crt");
    let err = run_cert(verify_cmd(&missing)).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("failed reading certificate from {}", missing.display())
    );

    let junk = temp.path().join("junk.crt");
    std::fs::write(&junk, "garbage").unwrap();
    let err = run_cert(verify_cmd(&junk)).await.unwrap_err();
    assert_eq!(err.to_string(), "no certificate found in PEM data");

    let mut cmd = verify_cmd(&ca.join("node.crt"));
    if let CertCommands::Verify { ca_cert, .. } = &mut cmd {
        *ca_cert = Some(temp.path().join("no-ca.crt"));
    }
    let err = run_cert(cmd).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "failed loading CA certs from {}",
            temp.path().join("no-ca.crt").display()
        )
    );

    // A certificate issued by a different CA is not trusted.
    let mut cmd = verify_cmd(&ca.join("node.crt"));
    if let CertCommands::Verify { ca_cert, .. } = &mut cmd {
        *ca_cert = Some(other.join("ca.crt"));
    }
    let err = run_cert(cmd).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "certificate trust verification failed against CA {}",
            other.join("ca.crt").display()
        )
    );
}

#[cfg(unix)]
#[tokio::test]
async fn verify_key_checks_permissions_then_certificate_match() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let ca = init_pki(temp.path(), true).await;
    let other_dir = temp.path().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other = init_pki(&other_dir, true).await;
    let cert = ca.join("node.crt");

    let with_key = |key: &Path| {
        let mut cmd = verify_cmd(&cert);
        if let CertCommands::Verify { key_file, .. } = &mut cmd {
            *key_file = Some(key.to_path_buf());
        }
        cmd
    };

    // The key of another PKI does not belong to this certificate.
    let err = run_cert(with_key(&other.join("node.key")))
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "private key at {} does not match certificate at {}",
            other.join("node.key").display(),
            cert.display()
        )
    );

    // A group/world readable key is refused before it is parsed.
    let loose = temp.path().join("loose.key");
    std::fs::copy(ca.join("node.key"), &loose).unwrap();
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = run_cert(with_key(&loose)).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("insecure private key permissions on {}", loose.display())
    );

    // Owner-only read (0400) is accepted.
    std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o400)).unwrap();
    run_cert(with_key(&loose)).await.unwrap();

    // A missing key file is reported as a permission-check failure with its path.
    let absent = temp.path().join("absent.key");
    let err = run_cert(with_key(&absent)).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("insecure private key permissions on {}", absent.display())
    );
}
