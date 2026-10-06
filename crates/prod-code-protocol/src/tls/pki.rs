/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Cluster PKI, certificate generation, authority management, and pinning (Phase 5.6).

use std::fs::OpenOptions;
use std::io::{BufReader, Error, ErrorKind, Result, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::RootCertStore;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};

use super::types::{
    DEFAULT_TLS_SERVER_NAME, TLS_CA_ENV, TLS_CERT_ENV, TLS_KEY_ENV, TLS_MODE_ENV, TLS_PIN_ENV,
    TLS_SERVER_NAME_ENV, check_key_permissions, ensure_crypto_provider,
};
use super::verifier::cert_sha256_fingerprint;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PkiInitReport {
    pub ca_cert_path: PathBuf,
    pub ca_key_path: PathBuf,
    pub node_cert_path: PathBuf,
    pub node_key_path: PathBuf,
    pub cert_pin: String,
    pub server_name: String,
    pub env_example: String,
}

/// Generate a self-signed Root Certificate Authority (CA) in PEM format.
pub fn generate_ca(common_name: &str) -> Result<(String, String)> {
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let ca_key = rcgen::KeyPair::generate()
        .map_err(|e| Error::other(format!("failed generating CA keypair: {e}")))?;
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|e| Error::other(format!("failed signing CA cert: {e}")))?;
    let cert_pem = ca_cert.pem();
    let key_pem = ca_key.serialize_pem();
    Ok((cert_pem, key_pem))
}

/// Issue a node certificate and private key in PEM format signed by a CA.
pub fn generate_node_cert(
    ca_cert_pem: &str,
    ca_key_pem: &str,
    san_names: &[String],
    san_ips: &[std::net::IpAddr],
) -> Result<(String, String)> {
    let ca_key = rcgen::KeyPair::from_pem(ca_key_pem)
        .map_err(|e| Error::new(ErrorKind::InvalidInput, format!("invalid CA key PEM: {e}")))?;
    let ca_params = rcgen::CertificateParams::from_ca_cert_pem(ca_cert_pem)
        .map_err(|e| Error::new(ErrorKind::InvalidInput, format!("invalid CA cert PEM: {e}")))?;
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|e| Error::other(format!("failed parsing CA cert: {e}")))?;

    let mut san_entries: Vec<rcgen::SanType> = Vec::new();
    for name in san_names {
        let ia5 = rcgen::Ia5String::try_from(name.to_string()).map_err(|e| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("invalid DNS SAN '{name}': {e}"),
            )
        })?;
        san_entries.push(rcgen::SanType::DnsName(ia5));
    }
    for ip in san_ips {
        san_entries.push(rcgen::SanType::IpAddress(*ip));
    }

    let mut server_params = rcgen::CertificateParams::default();
    server_params.subject_alt_names = san_entries;
    if let Some(first_name) = san_names.first() {
        server_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, first_name);
    }
    server_params.extended_key_usages = vec![
        rcgen::ExtendedKeyUsagePurpose::ServerAuth,
        rcgen::ExtendedKeyUsagePurpose::ClientAuth,
    ];
    server_params.key_usages = vec![
        rcgen::KeyUsagePurpose::DigitalSignature,
        rcgen::KeyUsagePurpose::KeyEncipherment,
    ];

    let server_key = rcgen::KeyPair::generate()
        .map_err(|e| Error::other(format!("failed generating node key: {e}")))?;
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .map_err(|e| Error::other(format!("failed signing node cert: {e}")))?;

    let cert_pem = server_cert.pem();
    let key_pem = server_key.serialize_pem();
    Ok((cert_pem, key_pem))
}

/// Write certificate and private key files with restrictive permissions (0600 on keys).
///
/// Writes private keys through an exclusive temporary file created with mode 0600 and
/// verified permissions before writing any secret material, replacing any preexisting file atomically.
pub fn write_cert_and_key(
    out_dir: &Path,
    prefix: &str,
    cert_pem: &str,
    key_pem: &str,
) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(out_dir)?;

    let cert_path = out_dir.join(format!("{prefix}.crt"));
    std::fs::write(&cert_path, cert_pem)?;

    let key_path = out_dir.join(format!("{prefix}.key"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;

        // Safe replacement: write secret bytes to a private temporary file exclusively with mode 0600
        let tmp_key_path = out_dir.join(format!("{prefix}.key.tmp.{}", std::process::id()));
        let _ = std::fs::remove_file(&tmp_key_path);

        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true).mode(0o600);
        let mut f = opts.open(&tmp_key_path).map_err(|e| {
            Error::other(format!(
                "Failed to create private temporary key file '{tmp_key_path:?}': {e}"
            ))
        })?;

        // Explicitly set 0600 mode on the file descriptor before writing any secret bytes
        let mut perms = f.metadata()?.permissions();
        perms.set_mode(0o600);
        f.set_permissions(perms)?;

        // Verify permissions before writing key pem
        check_key_permissions(&tmp_key_path)?;

        f.write_all(key_pem.as_bytes())?;
        f.flush()?;
        drop(f);

        // Double check permissions before atomic replacement
        check_key_permissions(&tmp_key_path)?;

        // If key_path exists, remove it first so the new 0600 inode replaces it cleanly
        if key_path.exists() {
            let _ = std::fs::remove_file(&key_path);
        }

        // Atomic rename of the 0600 temporary file over key_path
        std::fs::rename(&tmp_key_path, &key_path)?;

        // Verify final destination key permissions
        check_key_permissions(&key_path)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&key_path, key_pem)?;
    }

    Ok((cert_path, key_path))
}

/// Perform X.509 path validation of a certificate against a set of CA root certificates.
///
/// Validates signatures, issuer chain, validity period, key usage, and optionally expected server name.
pub fn verify_cert_against_ca(
    cert: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    ca_certs: &[CertificateDer<'_>],
    expected_server_name: Option<&str>,
) -> Result<()> {
    ensure_crypto_provider();
    let mut roots = RootCertStore::empty();
    for ca in ca_certs {
        roots.add(ca.clone()).map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Invalid CA certificate: {e}"),
            )
        })?;
    }
    if roots.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "No valid CA root certificates provided",
        ));
    }

    let now = UnixTime::now();
    let roots_arc = Arc::new(roots);

    // If an explicit server name is supplied, verify server certificate with that server name.
    if let Some(name) = expected_server_name {
        let server_name = ServerName::try_from(name.to_string()).map_err(|_| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("Invalid server name: '{name}'"),
            )
        })?;
        let verifier = rustls::client::WebPkiServerVerifier::builder(roots_arc)
            .build()
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidData,
                    format!("Failed building server verifier: {e}"),
                )
            })?;
        verifier
            .verify_server_cert(cert, intermediates, &server_name, &[], now)
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidData,
                    format!("Server certificate validation failed for '{name}': {e}"),
                )
            })?;
        return Ok(());
    }

    // Without an explicit server name, try default server name ("prod-code.internal") first.
    if let Ok(server_name) = ServerName::try_from(DEFAULT_TLS_SERVER_NAME) {
        let server_verifier = rustls::client::WebPkiServerVerifier::builder(roots_arc.clone())
            .build()
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidData,
                    format!("Failed building server verifier: {e}"),
                )
            })?;
        if server_verifier
            .verify_server_cert(cert, intermediates, &server_name, &[], now)
            .is_ok()
        {
            return Ok(());
        }
    }

    // Also attempt client-cert verification (which validates the full CA chain, signatures, validity period,
    // and client auth usage without requiring a specific hostname/IP SAN).
    let client_verifier = rustls::server::WebPkiClientVerifier::builder(roots_arc)
        .build()
        .map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Failed building client verifier: {e}"),
            )
        })?;
    client_verifier
        .verify_client_cert(cert, intermediates, now)
        .map_err(|e| {
            Error::new(
                ErrorKind::InvalidData,
                format!("Certificate trust chain or validity verification failed against CA: {e}"),
            )
        })?;

    Ok(())
}

/// Verify that the private key matches the public key in the leaf certificate.
///
/// Cryptographically validates that the supplied private key's SubjectPublicKeyInfo
/// matches the public key in the certificate using rustls `CertifiedKey` key consistency verification.
pub fn verify_cert_matches_key(cert: &CertificateDer<'_>, key: &PrivateKeyDer<'_>) -> Result<()> {
    ensure_crypto_provider();
    let provider = rustls::crypto::ring::default_provider();
    let certified_key = rustls::sign::CertifiedKey::from_der(
        vec![cert.clone().into_owned()],
        key.clone_key(),
        &provider,
    )
    .map_err(|e| {
        Error::new(
            ErrorKind::InvalidData,
            format!("Private key does not match certificate public key: {e}"),
        )
    })?;

    certified_key.keys_match().map_err(|e| {
        Error::new(
            ErrorKind::InvalidData,
            format!("Private key does not match certificate public key: {e}"),
        )
    })?;

    Ok(())
}

/// Calculate the SHA-256 certificate pin (lowercase hex) from PEM certificate bytes.
pub fn compute_cert_pin(cert_pem_bytes: &[u8]) -> Result<String> {
    let certs = rustls_pemfile::certs(&mut BufReader::new(cert_pem_bytes))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let cert = certs
        .first()
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "no certificate found in PEM data"))?;
    Ok(cert_sha256_fingerprint(cert))
}

/// Initialize a full cluster PKI hierarchy (CA + node cert/key + pin calculation).
pub fn init_cluster_pki(
    out_dir: &Path,
    server_name: Option<&str>,
    extra_ips: &[std::net::IpAddr],
) -> Result<PkiInitReport> {
    let s_name = server_name.unwrap_or(DEFAULT_TLS_SERVER_NAME).to_string();
    let (ca_cert_pem, ca_key_pem) = generate_ca("prod-code Cluster Root CA")?;
    let (ca_cert_path, ca_key_path) = write_cert_and_key(out_dir, "ca", &ca_cert_pem, &ca_key_pem)?;

    let san_names = vec![s_name.clone(), "localhost".to_string()];
    let mut san_ips = vec!["127.0.0.1".parse::<std::net::IpAddr>().unwrap()];
    for ip in extra_ips {
        if !san_ips.contains(ip) {
            san_ips.push(*ip);
        }
    }

    let (node_cert_pem, node_key_pem) =
        generate_node_cert(&ca_cert_pem, &ca_key_pem, &san_names, &san_ips)?;
    let (node_cert_path, node_key_path) =
        write_cert_and_key(out_dir, "node", &node_cert_pem, &node_key_pem)?;
    let cert_pin = compute_cert_pin(node_cert_pem.as_bytes())?;

    let env_example = format!(
        "export {TLS_MODE_ENV}=strict\nexport {TLS_CA_ENV}={}\nexport {TLS_CERT_ENV}={}\nexport {TLS_KEY_ENV}={}\nexport {TLS_PIN_ENV}={cert_pin}\nexport {TLS_SERVER_NAME_ENV}={s_name}",
        ca_cert_path.display(),
        node_cert_path.display(),
        node_key_path.display()
    );

    Ok(PkiInitReport {
        ca_cert_path,
        ca_key_path,
        node_cert_path,
        node_key_path,
        cert_pin,
        server_name: s_name,
        env_example,
    })
}
