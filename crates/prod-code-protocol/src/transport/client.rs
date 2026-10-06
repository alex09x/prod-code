/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::buffer::tune;
use super::stream::{AUTH_TOKEN_ENV, AUTH_TOKEN_FILE_ENV, AnyStream};
use crate::messages::{AuthToken, WireMessage};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::codec::Encoder;

/// The token every connection of this cluster opens with (#402), for clients and gateways
/// alike: [`AUTH_TOKEN_ENV`], else the first line of the file [`AUTH_TOKEN_FILE_ENV`] names.
pub fn auth_token() -> Option<String> {
    let file = std::env::var_os(AUTH_TOKEN_FILE_ENV).map(PathBuf::from);
    resolve_token(
        std::env::var(AUTH_TOKEN_ENV).ok().as_deref(),
        file.as_deref(),
    )
}

/// The token from the variable's value `env` when it has one, otherwise from the first line of
/// `file`; blank ones do not count.
pub(crate) fn resolve_token(env: Option<&str>, file: Option<&Path>) -> Option<String> {
    if let Some(token) = env.map(str::trim).filter(|t| !t.is_empty()) {
        return Some(token.to_string());
    }
    let text = std::fs::read_to_string(file?).ok()?;
    let token = text.lines().next()?.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Establishes a raw tuned TCP connection to `addr` with backoff retries for transient ARP/network errors.
pub(crate) async fn connect_raw_tcp(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let mut last_err = None;
    for attempt in 0..3 {
        if attempt > 0 {
            let backoff = match last_err
                .as_ref()
                .and_then(|e: &std::io::Error| e.raw_os_error())
            {
                Some(65) => Duration::from_millis(100 * attempt as u64),
                _ => Duration::from_millis(50 * (1 << (attempt - 1))),
            };
            tokio::time::sleep(backoff).await;
        }
        match TcpStream::connect(addr).await {
            Ok(stream) => {
                tune(&stream);
                return Ok(stream);
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap())
}

/// Connects to `addr` with a raw tuned TCP stream, failing closed if TLS is strictly required.
pub(crate) async fn connect_raw_tcp_with(
    addr: SocketAddr,
    token: Option<&str>,
) -> std::io::Result<TcpStream> {
    let mode = crate::tls::TlsMode::from_env()?;
    if mode.is_required() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "Refusing plaintext TCP connection to {addr}: TLS mode '{mode:?}' is strictly required"
            ),
        ));
    }
    let mut stream = connect_raw_tcp(addr).await?;
    if let Some(token) = token {
        let mut frame = bytes::BytesMut::new();
        crate::codec::ProdCodeCodec::new()
            .encode(WireMessage::Auth(AuthToken(token.to_string())), &mut frame)?;
        stream.write_all(&frame).await?;
    }
    Ok(stream)
}

/// Pre-negotiated/built client TLS 1.3 configuration and SNI server name.
pub type BuiltClientTls = (
    Arc<rustls::ClientConfig>,
    rustls::pki_types::ServerName<'static>,
);

static DEFAULT_CLIENT_TLS: RwLock<Option<Option<BuiltClientTls>>> = RwLock::new(None);

/// Pre-sets or caches built client TLS configuration for the current process.
pub fn set_default_client_tls_built(built: Option<BuiltClientTls>) {
    if let Ok(mut guard) = DEFAULT_CLIENT_TLS.write() {
        *guard = Some(built);
    }
}

/// Sets and caches default client TLS configuration from an existing [`crate::tls::ClientTlsConfig`].
pub fn set_default_client_tls(config: Option<crate::tls::ClientTlsConfig>) -> std::io::Result<()> {
    let built = match config {
        Some(cfg) => Some(cfg.build()?),
        None => None,
    };
    set_default_client_tls_built(built);
    Ok(())
}

/// Initializes and caches client TLS configuration from the process environment before credentials are scrubbed.
pub fn init_client_tls_from_env() -> std::io::Result<()> {
    let config = crate::tls::ClientTlsConfig::from_env()?;
    set_default_client_tls(config)
}

/// Returns the currently cached default client TLS configuration, if explicitly configured or cached.
pub fn default_client_tls_built() -> Option<Option<BuiltClientTls>> {
    DEFAULT_CLIENT_TLS
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Clears the cached client TLS configuration (primarily for unit tests).
pub fn clear_client_tls_cache() {
    if let Ok(mut guard) = DEFAULT_CLIENT_TLS.write() {
        *guard = None;
    }
}

/// Connects to `addr`, negotiating TLS 1.3 / mTLS if configured in the environment,
/// and sending the cluster's auth token strictly inside the encrypted channel.
pub async fn connect_stream(addr: SocketAddr) -> std::io::Result<AnyStream> {
    connect_stream_with(addr, auth_token().as_deref()).await
}

/// Connects to `addr`, negotiating TLS 1.3 / mTLS using the provided client TLS configuration.
pub async fn connect_stream_with_client_config(
    addr: SocketAddr,
    token: Option<&str>,
    client_config: Option<BuiltClientTls>,
) -> std::io::Result<AnyStream> {
    let mode = crate::tls::TlsMode::from_env()?;
    let configured_tls = match client_config {
        Some(c) => Some(c),
        None => {
            if let Some(cached) = default_client_tls_built() {
                cached
            } else if let Some(client_tls) = crate::tls::ClientTlsConfig::from_env()? {
                Some(client_tls.build()?)
            } else {
                None
            }
        }
    };

    if let Some((config, server_name)) = configured_tls {
        let tcp = connect_raw_tcp(addr).await?;
        let mut tls = crate::tls::upgrade_client_stream(tcp, config, server_name).await?;
        if let Some(token) = token {
            let mut frame = bytes::BytesMut::new();
            crate::codec::ProdCodeCodec::new()
                .encode(WireMessage::Auth(AuthToken(token.to_string())), &mut frame)?;
            tls.write_all(&frame).await?;
            tls.flush().await?;
        }
        return Ok(AnyStream::TlsClient(tls));
    } else if mode.is_required() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "TLS is strictly required by PROD_CODE_TLS_MODE ({mode:?}), but no valid client TLS credentials/CA or pins are configured"
            ),
        ));
    }

    connect_raw_tcp_with(addr, token).await.map(AnyStream::Tcp)
}

/// Connects to `addr`, negotiating TLS 1.3 / mTLS with an optional explicit [`crate::tls::ClientTlsConfig`].
pub async fn connect_stream_with_tls(
    addr: SocketAddr,
    token: Option<&str>,
    client_tls: Option<&crate::tls::ClientTlsConfig>,
) -> std::io::Result<AnyStream> {
    let client_config = match client_tls {
        Some(cfg) => Some(cfg.clone().build()?),
        None => None,
    };
    connect_stream_with_client_config(addr, token, client_config).await
}

/// Connects to `addr`, negotiating TLS 1.3 / mTLS if configured, and transmitting `token` inside TLS.
pub async fn connect_stream_with(
    addr: SocketAddr,
    token: Option<&str>,
) -> std::io::Result<AnyStream> {
    connect_stream_with_client_config(addr, token, None).await
}

/// Connects to `addr`, [`tune`]s the connection, and sends `token` as its first frame when one is given.
pub async fn connect_with(addr: SocketAddr, token: Option<&str>) -> std::io::Result<AnyStream> {
    connect_stream_with(addr, token).await
}

/// Connects to `addr` using the default auth token and TLS 1.3 negotiation when configured.
pub async fn connect(addr: SocketAddr) -> std::io::Result<AnyStream> {
    connect_stream(addr).await
}

/// Connects to the Unix domain socket at `path` and opens it with the cluster's [`auth_token`].
#[cfg(unix)]
pub async fn connect_unix(path: impl AsRef<Path>) -> std::io::Result<tokio::net::UnixStream> {
    connect_unix_with(path, auth_token().as_deref()).await
}

/// Connects to the Unix domain socket at `path` and sends `token` as its first frame when one is given.
#[cfg(unix)]
pub async fn connect_unix_with(
    path: impl AsRef<Path>,
    token: Option<&str>,
) -> std::io::Result<tokio::net::UnixStream> {
    let mut stream = tokio::net::UnixStream::connect(path).await?;
    if let Some(token) = token {
        let mut frame = bytes::BytesMut::new();
        crate::codec::ProdCodeCodec::new()
            .encode(WireMessage::Auth(AuthToken(token.to_string())), &mut frame)?;
        stream.write_all(&frame).await?;
    }
    Ok(stream)
}

/// Connects to the Windows named pipe at `path` and opens it with the cluster's [`auth_token`].
#[cfg(windows)]
pub async fn connect_named_pipe(
    path: impl AsRef<Path>,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    connect_named_pipe_with(path, auth_token().as_deref()).await
}

/// Connects to the Windows named pipe at `path` and sends `token` as its first frame when one is given.
#[cfg(windows)]
pub async fn connect_named_pipe_with(
    path: impl AsRef<Path>,
    token: Option<&str>,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    let mut client = tokio::net::windows::named_pipe::ClientOptions::new().open(path.as_ref())?;
    if let Some(token) = token {
        let mut frame = bytes::BytesMut::new();
        crate::codec::ProdCodeCodec::new()
            .encode(WireMessage::Auth(AuthToken(token.to_string())), &mut frame)?;
        client.write_all(&frame).await?;
    }
    Ok(client)
}
