/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::tls::pki::*;
use crate::tls::types::{load_certs, load_private_key};
use rustls::pki_types::CertificateDer;
use std::io::ErrorKind;

#[test]
fn ca_validation_names_chains_and_invalid_der() {
    let temp = tempfile::tempdir().unwrap();
    let (ca, ca_key) = generate_ca("Test CA").unwrap();
    let (leaf, leaf_key) = generate_node_cert(
        &ca,
        &ca_key,
        &["example.test".into()],
        &["::1".parse().unwrap()],
    )
    .unwrap();
    let (ca_path, _) = write_cert_and_key(temp.path(), "ca", &ca, &ca_key).unwrap();
    let (leaf_path, key_path) = write_cert_and_key(temp.path(), "leaf", &leaf, &leaf_key).unwrap();
    let roots = load_certs(ca_path).unwrap();
    let cert = load_certs(leaf_path).unwrap().remove(0);
    assert_eq!(
        verify_cert_against_ca(&cert, &[], &roots, Some("example.test")).unwrap(),
        ()
    );
    assert_eq!(
        verify_cert_against_ca(&cert, &[], &roots, Some("::1")).unwrap(),
        ()
    );
    // A client-auth certificate is accepted without imposing the default DNS name.
    assert_eq!(
        verify_cert_against_ca(&cert, &[], &roots, None).unwrap(),
        ()
    );
    let err = verify_cert_against_ca(&cert, &[], &[], None).unwrap_err();
    assert_eq!(
        (err.kind(), err.to_string()),
        (
            ErrorKind::InvalidData,
            "No valid CA root certificates provided".into()
        )
    );
    let err = verify_cert_against_ca(&cert, &[], &roots, Some("bad name")).unwrap_err();
    assert_eq!(
        (err.kind(), err.to_string()),
        (
            ErrorKind::InvalidInput,
            "Invalid server name: 'bad name'".into()
        )
    );
    let err =
        verify_cert_against_ca(&cert, &[], &[CertificateDer::from(vec![1, 2])], None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(
        err.to_string(),
        "Invalid CA certificate: invalid peer certificate: BadEncoding"
    );
    let err =
        verify_cert_against_ca(&CertificateDer::from(vec![1, 2]), &[], &roots, None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(
        err.to_string(),
        "Certificate trust chain or validity verification failed against CA: invalid peer certificate: BadEncoding"
    );
    let key = load_private_key(key_path).unwrap();
    assert_eq!(verify_cert_matches_key(&cert, &key).unwrap(), ());
    assert_eq!(
        compute_cert_pin(leaf.as_bytes()).unwrap(),
        crate::tls::verifier::cert_sha256_fingerprint(&cert)
    );
}

#[test]
fn malformed_pem_and_non_ascii_san_are_rejected() {
    let (ca, key) = generate_ca("Test CA").unwrap();
    let err = generate_node_cert(&ca, &key, &["é.test".into()], &[]).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert_eq!(
        err.to_string(),
        "invalid DNS SAN 'é.test': Invalid IA5String: 'é.test'"
    );
    for (cert, key, prefix) in [
        ("invalid", key.as_str(), "invalid CA cert PEM: "),
        (ca.as_str(), "invalid", "invalid CA key PEM: "),
    ] {
        let err = generate_node_cert(cert, key, &[], &[]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(err.to_string().starts_with(prefix), "{err}");
    }
    let err = compute_cert_pin(b"no PEM").unwrap_err();
    assert_eq!(
        (err.kind(), err.to_string()),
        (
            ErrorKind::InvalidData,
            "no certificate found in PEM data".into()
        )
    );
    let (leaf, _) = generate_node_cert(&ca, &key, &[], &[]).unwrap();
    assert_eq!(
        compute_cert_pin(format!("{leaf}{ca}").as_bytes()).unwrap(),
        compute_cert_pin(leaf.as_bytes()).unwrap()
    );
}

#[test]
fn key_loader_accepts_all_supported_pem_labels_and_skips_certificates() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("key.pem");
    for (label, kind) in [
        ("PRIVATE KEY", "pkcs8"),
        ("RSA PRIVATE KEY", "pkcs1"),
        ("EC PRIVATE KEY", "sec1"),
    ] {
        let pem = format!(
            "-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n-----BEGIN {label}-----\nBAUG\n-----END {label}-----\n"
        );
        write_cert_and_key(temp.path(), "key", "unused", &pem).unwrap();
        let loaded = load_private_key(temp.path().join("key.key")).unwrap();
        assert_eq!(loaded.secret_der(), &[4, 5, 6]);
        assert_eq!(
            match loaded {
                rustls::pki_types::PrivateKeyDer::Pkcs8(_) => "pkcs8",
                rustls::pki_types::PrivateKeyDer::Pkcs1(_) => "pkcs1",
                rustls::pki_types::PrivateKeyDer::Sec1(_) => "sec1",
                _ => "unknown",
            },
            kind
        );
    }
    std::fs::write(&path, "").unwrap();
    let err = load_certs(&path).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(
        err.to_string(),
        format!("No valid certificates found in '{path:?}'")
    );
    write_cert_and_key(temp.path(), "empty", "unused", "not a key").unwrap();
    let empty = temp.path().join("empty.key");
    let err = load_private_key(&empty).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert_eq!(
        err.to_string(),
        format!("No supported private key (PKCS#8, PKCS#1 RSA, or SEC1 EC) found in '{empty:?}'")
    );
    let malformed = "-----BEGIN PRIVATE KEY-----\n!!!!\n-----END PRIVATE KEY-----";
    write_cert_and_key(temp.path(), "malformed", "unused", malformed).unwrap();
    let err = load_private_key(temp.path().join("malformed.key")).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(err.to_string().contains("Failed to parse PEM key"));
    std::fs::write(
        &path,
        "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----",
    )
    .unwrap();
    let err = load_certs(&path).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(err.to_string().contains("Failed to parse PEM certificates"));
    let missing = temp.path().join("missing");
    assert_eq!(
        load_certs(&missing).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        load_private_key(&missing).unwrap_err().kind(),
        ErrorKind::NotFound
    );
}
