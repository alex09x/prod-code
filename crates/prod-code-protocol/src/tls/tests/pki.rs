/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::pki;
use super::super::types::check_key_permissions;
use std::io::{Error, ErrorKind};

fn load_certs_from_pem(
    pem: &str,
) -> std::io::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn load_private_key_from_pem(
    pem: &str,
) -> std::io::Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    loop {
        match rustls_pemfile::read_one(&mut reader)
            .map_err(|e| Error::new(ErrorKind::InvalidData, e))?
        {
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => {
                return Ok(rustls::pki_types::PrivateKeyDer::Pkcs8(key));
            }
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => {
                return Ok(rustls::pki_types::PrivateKeyDer::Pkcs1(key));
            }
            Some(rustls_pemfile::Item::Sec1Key(key)) => {
                return Ok(rustls::pki_types::PrivateKeyDer::Sec1(key));
            }
            Some(_) => continue,
            None => break,
        }
    }
    Err(Error::new(
        ErrorKind::InvalidData,
        "no private key found in PEM",
    ))
}

#[test]
fn pki_init_creates_valid_pki_and_sets_safe_permissions() {
    let temp = tempfile::tempdir().unwrap();
    let report = pki::init_cluster_pki(
        temp.path(),
        Some("prod-code.test.internal"),
        &["192.168.2.50".parse().unwrap()],
    )
    .unwrap();

    assert!(report.ca_cert_path.exists());
    assert!(report.ca_key_path.exists());
    assert!(report.node_cert_path.exists());
    assert!(report.node_key_path.exists());
    assert_eq!(report.cert_pin.len(), 64);

    // Check key permissions
    check_key_permissions(&report.ca_key_path).unwrap();
    check_key_permissions(&report.node_key_path).unwrap();

    // Pin calculation matches cert
    let node_pem = std::fs::read(&report.node_cert_path).unwrap();
    let pin = pki::compute_cert_pin(&node_pem).unwrap();
    assert_eq!(pin, report.cert_pin);
}

#[test]
fn pki_write_overwrites_insecure_existing_key_with_strict_mode() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("pki_sec");
    std::fs::create_dir_all(&out_dir).unwrap();
    let key_path = out_dir.join("test.key");

    // Pre-create an insecure key file with world-readable permissions (0666)
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).mode(0o666);
        let mut f = opts.open(&key_path).unwrap();
        let mut perms = f.metadata().unwrap().permissions();
        perms.set_mode(0o666);
        let _ = f.set_permissions(perms);
        let _ = writeln!(f, "insecure-preexisting-content");
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&key_path, "insecure-preexisting-content").unwrap();
    }

    // Call write_cert_and_key
    let (_ca_cert, ca_key) = pki::generate_ca("Test Insecure Overwrite CA").unwrap();
    let (written_cert, written_key) =
        pki::write_cert_and_key(&out_dir, "test", "cert-pem", &ca_key).unwrap();

    assert_eq!(written_key, key_path);
    assert!(written_cert.exists());
    assert!(written_key.exists());

    // Verify the key file permissions are now strictly 0600
    check_key_permissions(&written_key).expect("permissions must be tightened to 0600");
}

#[test]
fn verify_cert_against_ca_validates_chain_and_rejects_untrusted() {
    let (ca_cert_pem, ca_key_pem) = pki::generate_ca("Trusted Test CA").unwrap();
    let (node_cert_pem, _node_key_pem) = pki::generate_node_cert(
        &ca_cert_pem,
        &ca_key_pem,
        &["node.trusted.internal".into()],
        &[],
    )
    .unwrap();

    let ca_certs = load_certs_from_pem(&ca_cert_pem).unwrap();
    let node_certs = load_certs_from_pem(&node_cert_pem).unwrap();
    let node_leaf = &node_certs[0];

    // 1. Verification succeeds against the issuing CA
    pki::verify_cert_against_ca(node_leaf, &[], &ca_certs, Some("node.trusted.internal")).unwrap();

    // 2. Verification fails when checked against an unrelated CA
    let (unrelated_ca_pem, _) = pki::generate_ca("Unrelated Untrusted CA").unwrap();
    let unrelated_ca_certs = load_certs_from_pem(&unrelated_ca_pem).unwrap();
    let err = pki::verify_cert_against_ca(
        node_leaf,
        &[],
        &unrelated_ca_certs,
        Some("node.trusted.internal"),
    )
    .expect_err("must reject certificate signed by different untrusted CA");
    assert!(err.to_string().contains("UnknownIssuer") || err.to_string().contains("failed"));

    // 3. Verification fails when checked with wrong hostname
    let name_err =
        pki::verify_cert_against_ca(node_leaf, &[], &ca_certs, Some("wrong.attacker.internal"))
            .expect_err("must reject certificate with mismatched server name");
    assert!(
        name_err.to_string().contains("validation failed")
            || name_err.to_string().contains("NotValidForName")
    );
}

#[test]
fn verify_cert_matches_key_rejects_mismatched_key_and_accepts_matching() {
    let (ca_cert_pem, ca_key_pem) = pki::generate_ca("Match Test CA").unwrap();
    let (node1_cert_pem, node1_key_pem) =
        pki::generate_node_cert(&ca_cert_pem, &ca_key_pem, &["node1.internal".into()], &[])
            .unwrap();
    let (_node2_cert_pem, node2_key_pem) =
        pki::generate_node_cert(&ca_cert_pem, &ca_key_pem, &["node2.internal".into()], &[])
            .unwrap();

    let cert1 = &load_certs_from_pem(&node1_cert_pem).unwrap()[0];
    let key1 = load_private_key_from_pem(&node1_key_pem).unwrap();
    let key2 = load_private_key_from_pem(&node2_key_pem).unwrap();

    // 1. Matching cert and key succeed
    pki::verify_cert_matches_key(cert1, &key1).expect("matching key must verify successfully");

    // 2. Mismatched cert and key fail
    let err = pki::verify_cert_matches_key(cert1, &key2)
        .expect_err("mismatched key must fail verification");
    assert!(err.to_string().contains("does not match") || err.to_string().contains("BadSignature"));
}
