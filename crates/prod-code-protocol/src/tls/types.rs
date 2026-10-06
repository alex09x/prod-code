/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs::File;
use std::io::{BufReader, Error, ErrorKind, Result};
use std::path::{Path, PathBuf};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// Default server name used for TLS SNI and certificate validation on LAN nodes.
pub const DEFAULT_TLS_SERVER_NAME: &str = "prod-code.internal";

/// Environment variable specifying TLS mode: "disabled", "auto", "strict", "mutual".
pub const TLS_MODE_ENV: &str = "PROD_CODE_TLS_MODE";

/// Environment variable specifying path to server or client TLS certificate (PEM).
pub const TLS_CERT_ENV: &str = "PROD_CODE_TLS_CERT";

/// Environment variable specifying path to server or client TLS private key (PEM).
pub const TLS_KEY_ENV: &str = "PROD_CODE_TLS_KEY";

/// Environment variable specifying path to CA certificate (PEM) for peer/gateway verification.
pub const TLS_CA_ENV: &str = "PROD_CODE_TLS_CA";

/// Environment variable specifying comma-separated SHA-256 certificate pins (hex).
pub const TLS_PIN_ENV: &str = "PROD_CODE_TLS_PIN";

/// Environment variable overriding the expected TLS server name (SNI).
pub const TLS_SERVER_NAME_ENV: &str = "PROD_CODE_TLS_SERVER_NAME";

/// All TLS environment variables that must be scrubbed from child process environments.
pub const TLS_ENV_VARS: [&str; 6] = [
    TLS_MODE_ENV,
    TLS_CERT_ENV,
    TLS_KEY_ENV,
    TLS_CA_ENV,
    TLS_PIN_ENV,
    TLS_SERVER_NAME_ENV,
];

/// Operating mode for cluster TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// Plaintext TCP connections only (legacy compatibility).
    Disabled,
    /// Use TLS when certificate/CA is configured or advertised, otherwise plaintext.
    Auto,
    /// TLS is mandatory. Plaintext connections fail closed.
    Strict,
    /// Mutual TLS is mandatory (both gateway and client/peer authenticate with certificates).
    Mutual,
}

/// Standard filesystem directory for cluster TLS credentials (~/.prod-code/tls).
pub fn standard_tls_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(|h| PathBuf::from(h).join(".prod-code").join("tls"))
}

impl TlsMode {
    pub fn from_env() -> Result<Self> {
        match std::env::var(TLS_MODE_ENV) {
            Ok(raw) => {
                let s = raw.trim();
                if s.eq_ignore_ascii_case("disabled") || s == "0" || s.eq_ignore_ascii_case("false")
                {
                    Ok(TlsMode::Disabled)
                } else if s.eq_ignore_ascii_case("auto") {
                    Ok(TlsMode::Auto)
                } else if s.eq_ignore_ascii_case("strict")
                    || s == "1"
                    || s.eq_ignore_ascii_case("true")
                {
                    Ok(TlsMode::Strict)
                } else if s.eq_ignore_ascii_case("mutual") || s.eq_ignore_ascii_case("mtls") {
                    Ok(TlsMode::Mutual)
                } else {
                    Err(Error::new(
                        ErrorKind::InvalidInput,
                        format!(
                            "invalid {TLS_MODE_ENV}: '{s}'. Expected disabled, auto, strict, or mutual"
                        ),
                    ))
                }
            }
            Err(std::env::VarError::NotPresent) => {
                let has_default_certs = standard_tls_dir()
                    .map(|d| d.join("node.crt").exists() || d.join("ca.crt").exists())
                    .unwrap_or(false);

                // If cert or CA is explicitly configured or present in standard path, default to Strict, else Auto.
                if std::env::var(TLS_CERT_ENV).is_ok()
                    || std::env::var(TLS_CA_ENV).is_ok()
                    || std::env::var(TLS_PIN_ENV).is_ok()
                    || has_default_certs
                {
                    Ok(TlsMode::Strict)
                } else {
                    Ok(TlsMode::Auto)
                }
            }
            Err(e) => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("failed reading {TLS_MODE_ENV}: {e}"),
            )),
        }
    }

    pub fn is_required(&self) -> bool {
        matches!(self, TlsMode::Strict | TlsMode::Mutual)
    }

    pub fn requires_client_cert(&self) -> bool {
        matches!(self, TlsMode::Mutual)
    }
}

/// Ensures rustls crypto provider (ring) is installed as default.
pub fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Verify that private key file has restrictive permissions (0600 or 0400 on Unix).
pub fn check_key_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path)?;
        let mode = metadata.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(Error::new(
                ErrorKind::PermissionDenied,
                format!(
                    "Private key file '{path:?}' has insecure permissions ({mode:04o}); must be readable only by owner (0600 or 0400)"
                ),
            ));
        }
    }
    Ok(())
}

/// Reads PEM certificates from a file into DER certificates.
pub fn load_certs(path: impl AsRef<Path>) -> Result<Vec<CertificateDer<'static>>> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|e| {
        Error::new(
            ErrorKind::NotFound,
            format!("Failed to open certificate file '{path:?}': {e}"),
        )
    })?;
    let mut reader = BufReader::new(file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Failed to parse PEM certificates in '{path:?}': {e}"),
            )
        })?;
    if certs.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("No valid certificates found in '{path:?}'"),
        ));
    }
    Ok(certs)
}

/// Reads a private key (PKCS#8, RSA, or SEC1 EC) from a PEM file.
pub fn load_private_key(path: impl AsRef<Path>) -> Result<PrivateKeyDer<'static>> {
    let path = path.as_ref();
    check_key_permissions(path)?;

    let file = File::open(path).map_err(|e| {
        Error::new(
            ErrorKind::NotFound,
            format!("Failed to open private key file '{path:?}': {e}"),
        )
    })?;
    let mut reader = BufReader::new(file);

    loop {
        match rustls_pemfile::read_one(&mut reader).map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Failed to parse PEM key in '{path:?}': {e}"),
            )
        })? {
            Some(rustls_pemfile::Item::Pkcs8Key(key)) => return Ok(PrivateKeyDer::Pkcs8(key)),
            Some(rustls_pemfile::Item::Pkcs1Key(key)) => return Ok(PrivateKeyDer::Pkcs1(key)),
            Some(rustls_pemfile::Item::Sec1Key(key)) => return Ok(PrivateKeyDer::Sec1(key)),
            Some(_) => continue,
            None => break,
        }
    }

    Err(Error::new(
        ErrorKind::InvalidData,
        format!("No supported private key (PKCS#8, PKCS#1 RSA, or SEC1 EC) found in '{path:?}'"),
    ))
}
