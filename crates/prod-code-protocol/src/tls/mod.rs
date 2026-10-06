/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! TLS 1.3 transport security, mutual TLS, certificate pinning, and trust bootstrap
//! for prod-code cluster connections (Phase 5.6).
//!
//! Provides TCP confidentiality, authenticated gateway peer identities, replay-resistant
//! connection establishment, and secure key lifecycle management without plaintext fallback.

pub mod client;
pub mod pki;
pub mod server;
pub mod types;
pub mod verifier;

#[cfg(test)]
pub mod test_helpers;
#[cfg(test)]
pub(crate) mod tests;

pub use client::{ClientTlsConfig, TlsConnector, upgrade_client_stream};
pub use server::{ServerTlsConfig, TlsAcceptor, upgrade_server_stream};
pub use types::{
    DEFAULT_TLS_SERVER_NAME, TLS_CA_ENV, TLS_CERT_ENV, TLS_ENV_VARS, TLS_KEY_ENV, TLS_MODE_ENV,
    TLS_PIN_ENV, TLS_SERVER_NAME_ENV, TlsMode, check_key_permissions, ensure_crypto_provider,
    load_certs, load_private_key, standard_tls_dir,
};
pub use verifier::{CaOrPinnedVerifier, PinnedCertVerifier, cert_sha256_fingerprint, parse_pins};
