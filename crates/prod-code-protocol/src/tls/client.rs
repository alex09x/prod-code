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

use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::net::TcpStream;
pub use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;

use super::types::{
    DEFAULT_TLS_SERVER_NAME, TLS_CA_ENV, TLS_CERT_ENV, TLS_KEY_ENV, TLS_PIN_ENV,
    TLS_SERVER_NAME_ENV, TlsMode, ensure_crypto_provider, load_certs, load_private_key,
    standard_tls_dir,
};
use super::verifier::{CaOrPinnedVerifier, parse_pins};

/// Client TLS configuration builder.
pub struct ClientTlsConfig {
    pub ca_roots: Option<RootCertStore>,
    pub pins: Vec<String>,
    pub client_cert: Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>,
    pub server_name: String,
}

impl std::fmt::Debug for ClientTlsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientTlsConfig")
            .field("ca_roots", &self.ca_roots)
            .field("pins", &self.pins)
            .field(
                "client_cert",
                &self
                    .client_cert
                    .as_ref()
                    .map(|(certs, _)| format!("[{} certificate(s), key: [REDACTED]]", certs.len())),
            )
            .field("server_name", &self.server_name)
            .finish()
    }
}

impl Clone for ClientTlsConfig {
    fn clone(&self) -> Self {
        Self {
            ca_roots: self.ca_roots.clone(),
            pins: self.pins.clone(),
            client_cert: self
                .client_cert
                .as_ref()
                .map(|(certs, key)| (certs.clone(), key.clone_key())),
            server_name: self.server_name.clone(),
        }
    }
}

impl ClientTlsConfig {
    pub fn new() -> Self {
        Self {
            ca_roots: None,
            pins: Vec::new(),
            client_cert: None,
            server_name: DEFAULT_TLS_SERVER_NAME.to_string(),
        }
    }

    pub fn with_ca(mut self, ca_roots: RootCertStore) -> Self {
        self.ca_roots = Some(ca_roots);
        self
    }

    pub fn with_pins(mut self, pins: Vec<String>) -> Self {
        self.pins = pins;
        self
    }

    pub fn with_client_cert(
        mut self,
        certs: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> Self {
        self.client_cert = Some((certs, key));
        self
    }

    pub fn with_server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = name.into();
        self
    }

    /// Load client TLS configuration from environment variables.
    pub fn from_env() -> Result<Option<Self>> {
        ensure_crypto_provider();
        let mode = TlsMode::from_env()?;
        if mode == TlsMode::Disabled {
            return Ok(None);
        }

        let mut config = ClientTlsConfig::new();

        if let Ok(sni) = std::env::var(TLS_SERVER_NAME_ENV) {
            let sni = sni.trim();
            if !sni.is_empty() {
                config = config.with_server_name(sni);
            }
        }

        let mut has_crypto_material = false;

        if let Ok(pins_str) = std::env::var(TLS_PIN_ENV) {
            let pins = parse_pins(&pins_str);
            if !pins.is_empty() {
                has_crypto_material = true;
                config = config.with_pins(pins);
            }
        }

        let default_ca = standard_tls_dir()
            .map(|d| d.join("ca.crt"))
            .filter(|p| p.exists());
        let ca_path_opt = std::env::var(TLS_CA_ENV)
            .ok()
            .map(PathBuf::from)
            .or(default_ca);

        if let Some(ca_path) = ca_path_opt {
            let ca_certs = load_certs(&ca_path)?;
            let mut roots = RootCertStore::empty();
            for ca in ca_certs {
                roots.add(ca).map_err(|e| {
                    Error::new(
                        ErrorKind::InvalidData,
                        format!("Failed to add trusted CA: {e}"),
                    )
                })?;
            }
            has_crypto_material = true;
            config = config.with_ca(roots);
        }

        let cert_var = std::env::var(TLS_CERT_ENV).ok();
        let key_var = std::env::var(TLS_KEY_ENV).ok();

        if mode == TlsMode::Mutual && (cert_var.is_none() || key_var.is_none()) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "Mutual TLS mode ('{mode:?}') requires client '{TLS_CERT_ENV}' and '{TLS_KEY_ENV}'"
                ),
            ));
        }

        if let (Some(cert_path), Some(key_path)) = (cert_var, key_var) {
            let certs = load_certs(&cert_path)?;
            let key = load_private_key(&key_path)?;
            has_crypto_material = true;
            config = config.with_client_cert(certs, key);
        }

        if mode.is_required() && !has_crypto_material {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "TLS mode '{mode:?}' is required, but neither '{TLS_CA_ENV}', '{TLS_PIN_ENV}', nor client credentials are configured"
                ),
            ));
        }

        if !has_crypto_material {
            return Ok(None);
        }

        Ok(Some(config))
    }

    pub fn build(self) -> Result<(Arc<ClientConfig>, ServerName<'static>)> {
        ensure_crypto_provider();
        let server_name = ServerName::try_from(self.server_name.clone())
            .map_err(|_| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("Invalid TLS server name: '{}'", self.server_name),
                )
            })?
            .to_owned();

        let verifier: Arc<dyn ServerCertVerifier> =
            Arc::new(CaOrPinnedVerifier::new(self.ca_roots, self.pins));

        let builder =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(|e| Error::other(format!("Failed to configure client TLS 1.3: {e}")))?
                .dangerous()
                .with_custom_certificate_verifier(verifier);

        let client_config = if let Some((certs, key)) = self.client_cert {
            builder.with_client_auth_cert(certs, key).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("Invalid client cert/key: {e}"),
                )
            })?
        } else {
            builder.with_no_client_auth()
        };

        Ok((Arc::new(client_config), server_name))
    }
}

impl Default for ClientTlsConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Upgrades an established TCP stream to TLS on client side.
pub async fn upgrade_client_stream(
    tcp: TcpStream,
    config: Arc<ClientConfig>,
    server_name: ServerName<'static>,
) -> Result<ClientTlsStream<TcpStream>> {
    let connector = TlsConnector::from(config);
    connector.connect(server_name, tcp).await.map_err(|e| {
        Error::new(
            ErrorKind::ConnectionAborted,
            format!("TLS client handshake failed: {e}"),
        )
    })
}
