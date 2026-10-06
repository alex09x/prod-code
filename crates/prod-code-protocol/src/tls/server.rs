/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::io::{Error, ErrorKind, Result};
use std::path::PathBuf;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{RootCertStore, ServerConfig};
use tokio::net::TcpStream;
pub use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream as ServerTlsStream;

use super::types::{
    TLS_CA_ENV, TLS_CERT_ENV, TLS_KEY_ENV, TlsMode, ensure_crypto_provider, load_certs,
    load_private_key, standard_tls_dir,
};

/// Server TLS configuration builder.
pub struct ServerTlsConfig {
    pub certs: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub client_ca_roots: Option<RootCertStore>,
    pub require_client_auth: bool,
}

impl std::fmt::Debug for ServerTlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerTlsConfig")
            .field("certs_count", &self.certs.len())
            .field("key", &"[REDACTED]")
            .field("client_ca_roots", &self.client_ca_roots)
            .field("require_client_auth", &self.require_client_auth)
            .finish()
    }
}

impl Clone for ServerTlsConfig {
    fn clone(&self) -> Self {
        Self {
            certs: self.certs.clone(),
            key: self.key.clone_key(),
            client_ca_roots: self.client_ca_roots.clone(),
            require_client_auth: self.require_client_auth,
        }
    }
}

impl ServerTlsConfig {
    pub fn new(certs: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>) -> Self {
        Self {
            certs,
            key,
            client_ca_roots: None,
            require_client_auth: false,
        }
    }

    pub fn with_client_ca(mut self, ca_roots: RootCertStore, require: bool) -> Self {
        self.client_ca_roots = Some(ca_roots);
        self.require_client_auth = require;
        self
    }

    /// Load server TLS config from specified files or standard environment variables.
    pub fn from_env() -> Result<Option<Self>> {
        ensure_crypto_provider();
        let mode = TlsMode::from_env()?;
        if mode == TlsMode::Disabled {
            return Ok(None);
        }

        let default_cert = standard_tls_dir()
            .map(|d| d.join("node.crt"))
            .filter(|p| p.exists());
        let default_key = standard_tls_dir()
            .map(|d| d.join("node.key"))
            .filter(|p| p.exists());
        let default_ca = standard_tls_dir()
            .map(|d| d.join("ca.crt"))
            .filter(|p| p.exists());

        let cert_var = std::env::var(TLS_CERT_ENV)
            .ok()
            .map(PathBuf::from)
            .or(default_cert);
        let key_var = std::env::var(TLS_KEY_ENV)
            .ok()
            .map(PathBuf::from)
            .or(default_key);
        let ca_var = std::env::var(TLS_CA_ENV)
            .ok()
            .or_else(|| default_ca.map(|p| p.to_string_lossy().into_owned()));

        if mode.is_required() {
            if cert_var.is_none() || key_var.is_none() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "TLS mode '{mode:?}' is required on server, but '{TLS_CERT_ENV}' and/or '{TLS_KEY_ENV}' are missing"
                    ),
                ));
            }
            if mode == TlsMode::Mutual && ca_var.is_none() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "Mutual TLS mode ('{mode:?}') requires '{TLS_CA_ENV}' for verifying client certificates"
                    ),
                ));
            }
        }

        let cert_path = match cert_var {
            Some(p) => p,
            None => return Ok(None),
        };
        let key_path = match key_var {
            Some(p) => p,
            None => {
                return Err(Error::new(
                    ErrorKind::NotFound,
                    format!("'{TLS_CERT_ENV}' is set but '{TLS_KEY_ENV}' is missing"),
                ));
            }
        };

        let certs = load_certs(&cert_path)?;
        let key = load_private_key(&key_path)?;

        let mut config = ServerTlsConfig::new(certs, key);

        if let Some(ca_path) = ca_var {
            let ca_certs = load_certs(ca_path)?;
            let mut roots = RootCertStore::empty();
            for ca in ca_certs {
                roots.add(ca).map_err(|e| {
                    Error::new(
                        ErrorKind::InvalidData,
                        format!("Failed to add client CA: {e}"),
                    )
                })?;
            }
            config = config.with_client_ca(roots, mode.requires_client_cert());
        }

        Ok(Some(config))
    }

    pub fn build(self) -> Result<Arc<ServerConfig>> {
        ensure_crypto_provider();
        let builder =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| Error::other(format!("Failed to configure server TLS 1.3: {e}")))?;

        let builder = if let Some(ca_roots) = self.client_ca_roots {
            let verifier = if self.require_client_auth {
                rustls::server::WebPkiClientVerifier::builder(Arc::new(ca_roots))
                    .build()
                    .map_err(|e| {
                        Error::new(
                            ErrorKind::InvalidData,
                            format!("Invalid client verifier: {e}"),
                        )
                    })?
            } else {
                rustls::server::WebPkiClientVerifier::builder(Arc::new(ca_roots))
                    .allow_unauthenticated()
                    .build()
                    .map_err(|e| {
                        Error::new(
                            ErrorKind::InvalidData,
                            format!("Invalid client verifier: {e}"),
                        )
                    })?
            };
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };

        let server_config = builder
            .with_single_cert(self.certs, self.key)
            .map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("Invalid server cert/key: {e}"),
                )
            })?;

        Ok(Arc::new(server_config))
    }

    pub fn build_acceptor(self) -> Result<TlsAcceptor> {
        let server_config = self.build()?;
        Ok(TlsAcceptor::from(server_config))
    }
}

/// Upgrades an accepted TCP stream to TLS on server side.
pub async fn upgrade_server_stream(
    tcp: TcpStream,
    config: Arc<ServerConfig>,
) -> Result<ServerTlsStream<TcpStream>> {
    let acceptor = TlsAcceptor::from(config);
    acceptor.accept(tcp).await.map_err(|e| {
        Error::new(
            ErrorKind::ConnectionAborted,
            format!("TLS server handshake failed: {e}"),
        )
    })
}
