/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// Generates self-signed CA certificate and a server certificate issued by it.
pub fn generate_test_ca_and_cert(
    server_dns: &str,
) -> (
    CertificateDer<'static>,
    CertificateDer<'static>,
    PrivateKeyDer<'static>,
) {
    // 1. Generate CA
    let mut ca_params = rcgen::CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "prod-code Test Root CA");
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    // 2. Generate Server Cert
    let mut server_params = rcgen::CertificateParams::new(vec![server_dns.to_string()]).unwrap();
    server_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, server_dns);
    let server_key = rcgen::KeyPair::generate().unwrap();
    let server_cert = server_params
        .signed_by(&server_key, &ca_cert, &ca_key)
        .unwrap();

    let ca_der = CertificateDer::from(ca_cert.der().to_vec());
    let server_der = CertificateDer::from(server_cert.der().to_vec());
    let server_key_der = PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        server_key.serialize_der(),
    ));

    (ca_der, server_der, server_key_der)
}

/// Generates a standalone self-signed certificate and private key.
pub fn generate_test_self_signed(
    server_dns: &str,
) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let mut params = rcgen::CertificateParams::new(vec![server_dns.to_string()]).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, server_dns);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        key.serialize_der(),
    ));
    (cert_der, key_der)
}
