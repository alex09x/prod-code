/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::client::ClientTlsConfig;
use super::super::server::ServerTlsConfig;
use super::super::types::{
    TLS_CA_ENV, TLS_CERT_ENV, TLS_KEY_ENV, TLS_MODE_ENV, TlsMode, check_key_permissions,
};
use super::super::verifier::parse_pins;
use super::TEST_ENV_LOCK;
use std::io::ErrorKind;

#[test]
fn pin_parsing_and_normalization() {
    let raw = "3f:1a:2b:3c:4d:5e:6f:70:81:92:a3:b4:c5:d6:e7:f8:09:1a:2b:3c:4d:5e:6f:70:81:92:a3:b4:c5:d6:e7:f8,\n 11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff";
    let parsed = parse_pins(raw);
    assert_eq!(parsed.len(), 2);
    assert_eq!(
        parsed[0],
        "3f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8"
    );
    assert_eq!(
        parsed[1],
        "11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff"
    );
}

#[test]
#[cfg(unix)]
fn check_key_permissions_enforces_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::NamedTempFile::new().unwrap();
    let path = temp.path();

    // 0600: valid
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(check_key_permissions(path).is_ok());

    // 0644: invalid (group/others can read)
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let err = check_key_permissions(path).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::PermissionDenied);
}

#[test]
fn tls_mode_rejects_unknown_value() {
    let _lock = TEST_ENV_LOCK.blocking_lock();
    unsafe {
        std::env::set_var(TLS_MODE_ENV, "stricts");
    }
    let err = match TlsMode::from_env() {
        Err(e) => e,
        Ok(m) => panic!("expected invalid mode error, got {:?}", m),
    };
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    unsafe {
        std::env::remove_var(TLS_MODE_ENV);
    }
}

#[test]
fn server_tls_fails_closed_in_mutual_without_client_ca() {
    let _lock = TEST_ENV_LOCK.blocking_lock();
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let key_pem = key_pair.serialize_pem();
    let params = rcgen::CertificateParams::new(vec!["server.internal".to_string()]).unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_pem = cert.pem();

    let cert_file = tempfile::NamedTempFile::new().unwrap();
    let key_file = tempfile::NamedTempFile::new().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_file.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(cert_file.path(), cert_pem).unwrap();
    std::fs::write(key_file.path(), key_pem).unwrap();

    unsafe {
        std::env::set_var(TLS_MODE_ENV, "mutual");
        std::env::set_var(TLS_CERT_ENV, cert_file.path());
        std::env::set_var(TLS_KEY_ENV, key_file.path());
        std::env::remove_var(TLS_CA_ENV);
    }

    let err = match ServerTlsConfig::from_env() {
        Err(e) => e,
        Ok(_) => panic!("expected error for mutual mode without client CA"),
    };
    assert_eq!(err.kind(), ErrorKind::InvalidInput);

    unsafe {
        std::env::remove_var(TLS_MODE_ENV);
        std::env::remove_var(TLS_CERT_ENV);
        std::env::remove_var(TLS_KEY_ENV);
    }
}

#[test]
fn client_tls_fails_closed_in_mutual_without_client_cert() {
    let _lock = TEST_ENV_LOCK.blocking_lock();
    unsafe {
        std::env::set_var(TLS_MODE_ENV, "mutual");
        std::env::remove_var(TLS_CERT_ENV);
        std::env::remove_var(TLS_KEY_ENV);
    }

    let err = match ClientTlsConfig::from_env() {
        Err(e) => e,
        Ok(_) => panic!("expected error for mutual mode without client cert"),
    };
    assert_eq!(err.kind(), ErrorKind::InvalidInput);

    unsafe {
        std::env::remove_var(TLS_MODE_ENV);
    }
}
