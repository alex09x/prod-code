/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as RustlsError, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

/// Computes SHA-256 fingerprint of a DER certificate.
pub fn cert_sha256_fingerprint(cert: &CertificateDer<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cert.as_ref());
    let hash = hasher.finalize();
    let mut s = String::with_capacity(hash.len() * 2);
    for b in hash {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Parses certificate pin strings (comma or space separated hex SHA-256 fingerprints).
pub fn parse_pins(pins_str: &str) -> Vec<String> {
    pins_str
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.replace(':', "").to_ascii_lowercase())
        .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .collect()
}

/// Custom certificate verifier that matches server certificates against configured SHA-256 pins.
#[derive(Debug)]
pub struct PinnedCertVerifier {
    pins: Vec<String>,
}

impl PinnedCertVerifier {
    pub fn new(pins: Vec<String>) -> Self {
        Self { pins }
    }
}

impl ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, RustlsError> {
        let fp = cert_sha256_fingerprint(end_entity);
        if self.pins.iter().any(|pin| pin.eq_ignore_ascii_case(&fp)) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(RustlsError::General(format!(
                "Server certificate fingerprint '{fp}' does not match any configured pins"
            )))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        Err(RustlsError::General(
            "TLS 1.2 is disabled; only TLS 1.3 is supported".into(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Composite verifier that allows either a valid PKI/CA certificate chain OR a pinned certificate.
#[derive(Debug)]
pub struct CaOrPinnedVerifier {
    ca_verifier: Option<Arc<dyn ServerCertVerifier>>,
    pin_verifier: Option<PinnedCertVerifier>,
}

impl CaOrPinnedVerifier {
    pub fn new(ca_roots: Option<RootCertStore>, pins: Vec<String>) -> Self {
        let ca_verifier = ca_roots.map(|roots| {
            rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
                .build()
                .expect("valid webpki verifier") as Arc<dyn ServerCertVerifier>
        });
        let pin_verifier = if pins.is_empty() {
            None
        } else {
            Some(PinnedCertVerifier::new(pins))
        };
        Self {
            ca_verifier,
            pin_verifier,
        }
    }
}

impl ServerCertVerifier for CaOrPinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, RustlsError> {
        // First check certificate pin if configured
        if self.pin_verifier.as_ref().is_some_and(|pin_verifier| {
            pin_verifier
                .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
                .is_ok()
        }) {
            return Ok(ServerCertVerified::assertion());
        }

        // Fall back to CA verifier
        if let Some(ca_verifier) = &self.ca_verifier {
            return ca_verifier.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            );
        }

        Err(RustlsError::General(
            "Certificate verification failed: neither pin matched nor valid CA trust chain available".into(),
        ))
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        Err(RustlsError::General(
            "TLS 1.2 is disabled; only TLS 1.3 is supported".into(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
