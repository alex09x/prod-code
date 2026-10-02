//! TLS 1.3 transport security, mutual TLS, certificate pinning, and trust bootstrap
//! for prod-code cluster connections (Phase 5.6).
//!
//! Provides TCP confidentiality, authenticated gateway peer identities, replay-resistant
//! connection establishment, and secure key lifecycle management without plaintext fallback.

use std::fs::File;
use std::io::{BufReader, Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as RustlsError, RootCertStore, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use tokio_rustls::server::TlsStream as ServerTlsStream;
pub use tokio_rustls::{TlsAcceptor, TlsConnector};

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

impl TlsMode {
    pub fn from_env() -> Result<Self> {
        match std::env::var(TLS_MODE_ENV) {
            Ok(raw) => {
                let s = raw.trim();
                if s.eq_ignore_ascii_case("disabled") || s == "0" || s.eq_ignore_ascii_case("false") {
                    Ok(TlsMode::Disabled)
                } else if s.eq_ignore_ascii_case("auto") {
                    Ok(TlsMode::Auto)
                } else if s.eq_ignore_ascii_case("strict") || s == "1" || s.eq_ignore_ascii_case("true") {
                    Ok(TlsMode::Strict)
                } else if s.eq_ignore_ascii_case("mutual") || s.eq_ignore_ascii_case("mtls") {
                    Ok(TlsMode::Mutual)
                } else {
                    Err(Error::new(
                        ErrorKind::InvalidInput,
                        format!("invalid {TLS_MODE_ENV}: '{s}'. Expected disabled, auto, strict, or mutual"),
                    ))
                }
            }
            Err(std::env::VarError::NotPresent) => {
                // If cert or CA is explicitly configured, default to Strict, else Auto.
                if std::env::var(TLS_CERT_ENV).is_ok()
                    || std::env::var(TLS_CA_ENV).is_ok()
                    || std::env::var(TLS_PIN_ENV).is_ok()
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
        .map_err(|e| Error::new(ErrorKind::InvalidData, format!("Failed to parse PEM certificates in '{path:?}': {e}")))?;
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
        match rustls_pemfile::read_one(&mut reader)
            .map_err(|e| Error::new(ErrorKind::InvalidData, format!("Failed to parse PEM key in '{path:?}': {e}")))?
        {
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
            return ca_verifier.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now);
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

/// Server TLS configuration builder.
#[derive(Debug)]
pub struct ServerTlsConfig {
    pub certs: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub client_ca_roots: Option<RootCertStore>,
    pub require_client_auth: bool,
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

        let cert_var = std::env::var(TLS_CERT_ENV).ok().map(PathBuf::from);
        let key_var = std::env::var(TLS_KEY_ENV).ok().map(PathBuf::from);
        let ca_var = std::env::var(TLS_CA_ENV).ok();

        if mode.is_required() {
            if cert_var.is_none() || key_var.is_none() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("TLS mode '{mode:?}' is required on server, but '{TLS_CERT_ENV}' and/or '{TLS_KEY_ENV}' are missing"),
                ));
            }
            if mode == TlsMode::Mutual && ca_var.is_none() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("Mutual TLS mode ('{mode:?}') requires '{TLS_CA_ENV}' for verifying client certificates"),
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
                ))
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
                    Error::new(ErrorKind::InvalidData, format!("Failed to add client CA: {e}"))
                })?;
            }
            config = config.with_client_ca(roots, mode.requires_client_cert());
        }

        Ok(Some(config))
    }

    pub fn build(self) -> Result<Arc<ServerConfig>> {
        ensure_crypto_provider();
        let builder = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| Error::other(format!("Failed to configure server TLS 1.3: {e}")))?;

        let builder = if let Some(ca_roots) = self.client_ca_roots {
            let verifier = if self.require_client_auth {
                rustls::server::WebPkiClientVerifier::builder(Arc::new(ca_roots))
                    .build()
                    .map_err(|e| Error::new(ErrorKind::InvalidData, format!("Invalid client verifier: {e}")))?
            } else {
                rustls::server::WebPkiClientVerifier::builder(Arc::new(ca_roots))
                    .allow_unauthenticated()
                    .build()
                    .map_err(|e| Error::new(ErrorKind::InvalidData, format!("Invalid client verifier: {e}")))?
            };
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };

        let server_config = builder
            .with_single_cert(self.certs, self.key)
            .map_err(|e| Error::new(ErrorKind::InvalidInput, format!("Invalid server cert/key: {e}")))?;

        Ok(Arc::new(server_config))
    }

    pub fn build_acceptor(self) -> Result<TlsAcceptor> {
        let server_config = self.build()?;
        Ok(TlsAcceptor::from(server_config))
    }
}

/// Client TLS configuration builder.
#[derive(Debug)]
pub struct ClientTlsConfig {
    pub ca_roots: Option<RootCertStore>,
    pub pins: Vec<String>,
    pub client_cert: Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>,
    pub server_name: String,
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

        if let Ok(ca_path) = std::env::var(TLS_CA_ENV) {
            let ca_certs = load_certs(ca_path)?;
            let mut roots = RootCertStore::empty();
            for ca in ca_certs {
                roots.add(ca).map_err(|e| {
                    Error::new(ErrorKind::InvalidData, format!("Failed to add trusted CA: {e}"))
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
                format!("Mutual TLS mode ('{mode:?}') requires client '{TLS_CERT_ENV}' and '{TLS_KEY_ENV}'"),
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
                format!("TLS mode '{mode:?}' is required, but neither '{TLS_CA_ENV}', '{TLS_PIN_ENV}', nor client credentials are configured"),
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
            .map_err(|_| Error::new(ErrorKind::InvalidInput, format!("Invalid TLS server name: '{}'", self.server_name)))?
            .to_owned();

        let verifier: Arc<dyn ServerCertVerifier> = Arc::new(CaOrPinnedVerifier::new(self.ca_roots, self.pins));

        let builder = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| Error::other(format!("Failed to configure client TLS 1.3: {e}")))?
            .dangerous()
            .with_custom_certificate_verifier(verifier);

        let client_config = if let Some((certs, key)) = self.client_cert {
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| Error::new(ErrorKind::InvalidInput, format!("Invalid client cert/key: {e}")))?
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
    connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| Error::new(ErrorKind::ConnectionAborted, format!("TLS client handshake failed: {e}")))
}

/// Upgrades an accepted TCP stream to TLS on server side.
pub async fn upgrade_server_stream(
    tcp: TcpStream,
    config: Arc<ServerConfig>,
) -> Result<ServerTlsStream<TcpStream>> {
    let acceptor = TlsAcceptor::from(config);
    acceptor
        .accept(tcp)
        .await
        .map_err(|e| Error::new(ErrorKind::ConnectionAborted, format!("TLS server handshake failed: {e}")))
}

/// Cluster PKI, certificate generation, authority management, and pinning (Phase 5.6).
pub mod pki {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;

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
            .map_err(|e| Error::new(ErrorKind::Other, format!("failed generating CA keypair: {e}")))?;
        let ca_cert = ca_params
            .self_signed(&ca_key)
            .map_err(|e| Error::new(ErrorKind::Other, format!("failed signing CA cert: {e}")))?;
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
            .map_err(|e| Error::new(ErrorKind::Other, format!("failed parsing CA cert: {e}")))?;

        let mut san_entries: Vec<rcgen::SanType> = Vec::new();
        for name in san_names {
            let ia5 = rcgen::Ia5String::try_from(name.to_string())
                .map_err(|e| Error::new(ErrorKind::InvalidInput, format!("invalid DNS SAN '{name}': {e}")))?;
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
            .map_err(|e| Error::new(ErrorKind::Other, format!("failed generating node key: {e}")))?;
        let server_cert = server_params
            .signed_by(&server_key, &ca_cert, &ca_key)
            .map_err(|e| Error::new(ErrorKind::Other, format!("failed signing node cert: {e}")))?;

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
                Error::new(
                    ErrorKind::Other,
                    format!("Failed to create private temporary key file '{tmp_key_path:?}': {e}"),
                )
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
                Error::new(ErrorKind::InvalidData, format!("Invalid CA certificate: {e}"))
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
                Error::new(ErrorKind::InvalidInput, format!("Invalid server name: '{name}'"))
            })?;
            let verifier = rustls::client::WebPkiServerVerifier::builder(roots_arc)
                .build()
                .map_err(|e| Error::new(ErrorKind::InvalidData, format!("Failed building server verifier: {e}")))?;
            verifier
                .verify_server_cert(cert, intermediates, &server_name, &[], now)
                .map_err(|e| Error::new(ErrorKind::InvalidData, format!("Server certificate validation failed for '{name}': {e}")))?;
            return Ok(());
        }

        // Without an explicit server name, try default server name ("prod-code.internal") first.
        if let Ok(server_name) = ServerName::try_from(DEFAULT_TLS_SERVER_NAME) {
            let server_verifier = rustls::client::WebPkiServerVerifier::builder(roots_arc.clone())
                .build()
                .map_err(|e| Error::new(ErrorKind::InvalidData, format!("Failed building server verifier: {e}")))?;
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
            .map_err(|e| Error::new(ErrorKind::InvalidData, format!("Failed building client verifier: {e}")))?;
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

        let (node_cert_pem, node_key_pem) = generate_node_cert(
            &ca_cert_pem,
            &ca_key_pem,
            &san_names,
            &san_ips,
        )?;
        let (node_cert_path, node_key_path) = write_cert_and_key(out_dir, "node", &node_cert_pem, &node_key_pem)?;
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
}

#[cfg(test)]
pub mod test_helpers {
    use super::*;

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
        let server_cert = server_params.signed_by(&server_key, &ca_cert, &ca_key).unwrap();

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
}

#[cfg(test)]
pub(crate) mod tests {
    use super::test_helpers::*;
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tls_handshake_with_ca_verification_succeeds() {
        let (ca_der, server_cert, server_key) = generate_test_ca_and_cert("localhost");

        // Configure server
        let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
            .build()
            .unwrap();

        // Configure client with CA
        let mut roots = RootCertStore::empty();
        roots.add(ca_der).unwrap();
        let (client_tls, server_name) = ClientTlsConfig::new()
            .with_ca(roots)
            .with_server_name("localhost")
            .build()
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
            let mut buf = [0u8; 5];
            tls_stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping!");
            tls_stream.write_all(b"pong!").await.unwrap();
            tls_stream.flush().await.unwrap();
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let mut tls_client = upgrade_client_stream(client_stream, client_tls, server_name)
            .await
            .unwrap();

        tls_client.write_all(b"ping!").await.unwrap();
        tls_client.flush().await.unwrap();

        let mut reply = [0u8; 5];
        tls_client.read_exact(&mut reply).await.unwrap();
        assert_eq!(&reply, b"pong!");

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn tls_certificate_pinning_verification_succeeds() {
        let (server_cert, server_key) = generate_test_self_signed("node-1.code.internal");
        let pin = cert_sha256_fingerprint(&server_cert);

        let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
            .build()
            .unwrap();

        let (client_tls, server_name) = ClientTlsConfig::new()
            .with_pins(vec![pin])
            .with_server_name("node-1.code.internal")
            .build()
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
            tls_stream.write_all(b"secure").await.unwrap();
            tls_stream.flush().await.unwrap();
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let mut tls_client = upgrade_client_stream(client_stream, client_tls, server_name)
            .await
            .unwrap();

        let mut buf = [0u8; 6];
        tls_client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"secure");

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn tls_invalid_pin_or_untrusted_ca_fails_handshake() {
        let (server_cert, server_key) = generate_test_self_signed("bad-node.internal");

        let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
            .build()
            .unwrap();

        // Pin is completely wrong
        let bogus_pin = "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        let (client_tls, server_name) = ClientTlsConfig::new()
            .with_pins(vec![bogus_pin])
            .with_server_name("bad-node.internal")
            .build()
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = upgrade_server_stream(stream, server_tls).await;
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let client_res = upgrade_client_stream(client_stream, client_tls, server_name).await;
        assert!(client_res.is_err(), "Handshake with wrong pin must fail");

        let _ = server_task.await;
    }

    #[tokio::test]
    async fn mtls_mutual_authentication_succeeds() {
        let (ca_der, server_cert, server_key) = generate_test_ca_and_cert("cluster-peer");
        let (client_ca, client_cert, client_key) = generate_test_ca_and_cert("agent-worker");

        // Server requires client cert and trusts client_ca
        let mut client_roots = RootCertStore::empty();
        client_roots.add(client_ca).unwrap();

        let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
            .with_client_ca(client_roots, true)
            .build()
            .unwrap();

        // Client presents cert and trusts server CA
        let mut server_roots = RootCertStore::empty();
        server_roots.add(ca_der).unwrap();

        let (client_tls, server_name) = ClientTlsConfig::new()
            .with_ca(server_roots)
            .with_client_cert(vec![client_cert], client_key)
            .with_server_name("cluster-peer")
            .build()
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
            tls_stream.write_all(b"mtls-ok").await.unwrap();
            tls_stream.flush().await.unwrap();
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let mut tls_client = upgrade_client_stream(client_stream, client_tls, server_name)
            .await
            .unwrap();

        let mut buf = [0u8; 7];
        tls_client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"mtls-ok");

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn mtls_untrusted_client_cert_is_rejected() {
        let (ca_der, server_cert, server_key) = generate_test_ca_and_cert("cluster-peer");
        let (_untrusted_ca, untrusted_client_cert, untrusted_client_key) =
            generate_test_ca_and_cert("untrusted-agent");

        let mut client_roots = RootCertStore::empty();
        client_roots.add(ca_der.clone()).unwrap();

        let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
            .with_client_ca(client_roots, true)
            .build()
            .unwrap();

        let mut server_roots = RootCertStore::empty();
        server_roots.add(ca_der).unwrap();

        let (client_tls, server_name) = ClientTlsConfig::new()
            .with_ca(server_roots)
            .with_client_cert(vec![untrusted_client_cert], untrusted_client_key)
            .with_server_name("cluster-peer")
            .build()
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let res = upgrade_server_stream(stream, server_tls).await;
            assert!(res.is_err(), "Server must reject untrusted client cert: {res:?}");
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        match upgrade_client_stream(client_stream, client_tls, server_name).await {
            Ok(mut tls_client) => {
                let mut buf = [0u8; 1];
                let read_res = tls_client.read_exact(&mut buf).await;
                assert!(read_res.is_err(), "Read must fail because server rejected client cert");
            }
            Err(_) => {
                // Client handshake failed directly
            }
        }

        server_task.await.unwrap();
    }

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

    pub(crate) static TEST_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

    #[tokio::test]
    async fn only_tls13_is_negotiated() {
        let (ca_der, server_cert, server_key) = generate_test_ca_and_cert("tls13-only.internal");

        let server_tls = ServerTlsConfig::new(vec![server_cert], server_key)
            .build()
            .unwrap();

        let mut roots = RootCertStore::empty();
        roots.add(ca_der).unwrap();
        let (client_tls, server_name) = ClientTlsConfig::new()
            .with_ca(roots)
            .with_server_name("tls13-only.internal")
            .build()
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
            let (_, server_conn) = tls_stream.get_ref();
            assert_eq!(server_conn.protocol_version(), Some(rustls::ProtocolVersion::TLSv1_3));
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let tls_client = upgrade_client_stream(client_stream, client_tls, server_name).await.unwrap();
        let (_, client_conn) = tls_client.get_ref();
        assert_eq!(client_conn.protocol_version(), Some(rustls::ProtocolVersion::TLSv1_3));

        server_task.await.unwrap();
    }

    #[test]
    fn pki_init_creates_valid_pki_and_sets_safe_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let report = pki::init_cluster_pki(
            temp.path(),
            Some("prod-code.test.internal"),
            &["192.168.2.50".parse().unwrap()],
        )
        .unwrap();

        assert!(report.ca_cert_path.exists());
        assert!(report.ca_key_path.exists());
        assert!(report.node_cert_path.exists());
        assert!(report.node_key_path.exists());
        assert_eq!(report.cert_pin.len(), 64);

        // Check key permissions
        check_key_permissions(&report.ca_key_path).unwrap();
        check_key_permissions(&report.node_key_path).unwrap();

        // Pin calculation matches cert
        let node_pem = std::fs::read(&report.node_cert_path).unwrap();
        let pin = pki::compute_cert_pin(&node_pem).unwrap();
        assert_eq!(pin, report.cert_pin);
    }

    #[test]
    fn pki_write_overwrites_insecure_existing_key_with_strict_mode() {
        let temp = tempfile::tempdir().unwrap();
        let out_dir = temp.path().join("pki_sec");
        std::fs::create_dir_all(&out_dir).unwrap();
        let key_path = out_dir.join("test.key");

        // Pre-create an insecure key file with world-readable permissions (0666)
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            use std::os::unix::fs::PermissionsExt;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).mode(0o666);
            let mut f = opts.open(&key_path).unwrap();
            let mut perms = f.metadata().unwrap().permissions();
            perms.set_mode(0o666);
            let _ = f.set_permissions(perms);
            use std::io::Write;
            let _ = writeln!(f, "insecure-preexisting-content");
        }
        #[cfg(not(unix))]
        {
            std::fs::write(&key_path, "insecure-preexisting-content").unwrap();
        }

        // Call write_cert_and_key
        let (_ca_cert, ca_key) = pki::generate_ca("Test Insecure Overwrite CA").unwrap();
        let (written_cert, written_key) = pki::write_cert_and_key(&out_dir, "test", "cert-pem", &ca_key).unwrap();

        assert_eq!(written_key, key_path);
        assert!(written_cert.exists());
        assert!(written_key.exists());

        // Verify the key file permissions are now strictly 0600
        check_key_permissions(&written_key).expect("permissions must be tightened to 0600");
    }

    #[test]
    fn verify_cert_against_ca_validates_chain_and_rejects_untrusted() {
        let (ca_cert_pem, ca_key_pem) = pki::generate_ca("Trusted Test CA").unwrap();
        let (node_cert_pem, _node_key_pem) = pki::generate_node_cert(
            &ca_cert_pem,
            &ca_key_pem,
            &["node.trusted.internal".into()],
            &[],
        ).unwrap();

        let ca_certs = load_certs_from_pem(&ca_cert_pem).unwrap();
        let node_certs = load_certs_from_pem(&node_cert_pem).unwrap();
        let node_leaf = &node_certs[0];

        // 1. Verification succeeds against the issuing CA
        pki::verify_cert_against_ca(node_leaf, &[], &ca_certs, Some("node.trusted.internal")).unwrap();

        // 2. Verification fails when checked against an unrelated CA
        let (unrelated_ca_pem, _) = pki::generate_ca("Unrelated Untrusted CA").unwrap();
        let unrelated_ca_certs = load_certs_from_pem(&unrelated_ca_pem).unwrap();
        let err = pki::verify_cert_against_ca(node_leaf, &[], &unrelated_ca_certs, Some("node.trusted.internal"))
            .expect_err("must reject certificate signed by different untrusted CA");
        assert!(err.to_string().contains("UnknownIssuer") || err.to_string().contains("failed"));

        // 3. Verification fails when checked with wrong hostname
        let name_err = pki::verify_cert_against_ca(node_leaf, &[], &ca_certs, Some("wrong.attacker.internal"))
            .expect_err("must reject certificate with mismatched server name");
        assert!(name_err.to_string().contains("validation failed") || name_err.to_string().contains("NotValidForName"));
    }

    fn load_certs_from_pem(pem: &str) -> std::io::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
        let mut reader = std::io::BufReader::new(pem.as_bytes());
        rustls_pemfile::certs(&mut reader).collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
