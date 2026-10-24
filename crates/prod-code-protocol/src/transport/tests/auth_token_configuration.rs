/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::client::{
    auth_token, clear_client_tls_cache, connect, resolve_token, try_auth_token,
};
use super::super::stream::{AUTH_TOKEN_ENV, AUTH_TOKEN_FILE_ENV};
use std::path::Path;

struct AuthEnvReset;

impl Drop for AuthEnvReset {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var(AUTH_TOKEN_ENV);
            std::env::remove_var(AUTH_TOKEN_FILE_ENV);
            std::env::remove_var(crate::tls::TLS_MODE_ENV);
        }
    }
}

#[tokio::test]
async fn auth_token_configuration_distinguishes_absent_valid_and_invalid_files() {
    let _lock = crate::tls::tests::TEST_ENV_LOCK.lock().await;
    let _reset = AuthEnvReset;
    clear_client_tls_cache();

    let dir = tempfile::tempdir().unwrap();
    let valid = dir.path().join("valid-token");
    std::fs::write(&valid, "  file-secret  \nignored-second-line\n").unwrap();
    let missing = dir.path().join("missing-token");
    let empty = dir.path().join("empty-token");
    std::fs::write(&empty, "").unwrap();
    let blank_first = dir.path().join("blank-first-token");
    std::fs::write(&blank_first, "   \nsecond-line-canary\n").unwrap();
    let non_utf8 = dir.path().join("non-utf8-token");
    std::fs::write(&non_utf8, [0xff, 0xfe, b'\n']).unwrap();
    let unreadable_dir = dir.path().join("unreadable-dir");
    std::fs::create_dir_all(&unreadable_dir).unwrap();

    // 1. Absent configuration permits tokenless mode.
    unsafe {
        std::env::remove_var(crate::tls::TLS_MODE_ENV);
        std::env::remove_var(AUTH_TOKEN_ENV);
        std::env::remove_var(AUTH_TOKEN_FILE_ENV);
    }
    assert_eq!(resolve_token(None, None).unwrap(), None);
    assert_eq!(resolve_token(Some("   "), None).unwrap(), None);
    assert_eq!(try_auth_token().unwrap(), None);
    assert_eq!(auth_token(), None);

    // 2. Valid file configuration resolves trimmed first line.
    unsafe {
        std::env::set_var(AUTH_TOKEN_FILE_ENV, &valid);
    }
    assert_eq!(try_auth_token().unwrap().as_deref(), Some("file-secret"));
    assert_eq!(auth_token().as_deref(), Some("file-secret"));

    // 3. Empty path in AUTH_TOKEN_FILE_ENV fails closed.
    assert!(resolve_token(None, Some(Path::new(""))).is_err());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    #[cfg(unix)]
    let sock_path = dir.path().join("default-conn.sock");
    #[cfg(unix)]
    let _unix_listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

    // 4. Missing, empty, blank-first-line, non-UTF-8, and unreadable file configurations fail closed.
    for bad_path in [&missing, &empty, &blank_first, &non_utf8, &unreadable_dir] {
        // Direct non-empty token retains precedence even when file setting is invalid.
        assert_eq!(
            resolve_token(Some(" direct-secret "), Some(bad_path))
                .unwrap()
                .as_deref(),
            Some("direct-secret")
        );
        unsafe {
            std::env::set_var(AUTH_TOKEN_ENV, " direct-secret ");
            std::env::set_var(AUTH_TOKEN_FILE_ENV, bad_path);
        }
        assert_eq!(try_auth_token().unwrap().as_deref(), Some("direct-secret"));
        assert_eq!(auth_token().as_deref(), Some("direct-secret"));

        // Without a valid direct token, invalid file configuration fails closed.
        unsafe {
            std::env::remove_var(AUTH_TOKEN_ENV);
        }
        let err = try_auth_token().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("PROD_CODE_AUTH_TOKEN_FILE"), "{msg}");
        assert!(!msg.contains("second-line-canary"), "{msg}");
        assert!(std::panic::catch_unwind(auth_token).is_err());

        let tcp_err = connect(addr).await.unwrap_err();
        assert!(
            tcp_err.to_string().contains("PROD_CODE_AUTH_TOKEN_FILE"),
            "{tcp_err}"
        );
        #[cfg(unix)]
        {
            let unix_err = super::super::client::connect_unix(&sock_path)
                .await
                .unwrap_err();
            assert!(
                unix_err.to_string().contains("PROD_CODE_AUTH_TOKEN_FILE"),
                "{unix_err}"
            );
        }
    }
}
