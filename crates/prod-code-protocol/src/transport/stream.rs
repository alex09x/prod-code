/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::client::connect_stream;
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpStream;

pub const AUTH_TOKEN_ENV: &str = "PROD_CODE_AUTH_TOKEN";
pub const SOCKET_PATH_ENV: &str = "PROD_CODE_SOCKET";
pub const AUTH_TOKEN_FILE_ENV: &str = "PROD_CODE_AUTH_TOKEN_FILE";

pub const AUTH_TOKEN_VARS: [&str; 2] = [AUTH_TOKEN_ENV, AUTH_TOKEN_FILE_ENV];

/// Helper trait to scrub all cluster secret credentials (cluster auth tokens and TLS material)
/// from command environments before spawning child processes (#402, Phase 5.6).
pub trait ScrubSecrets {
    /// Removes all [`AUTH_TOKEN_VARS`] and [`crate::tls::TLS_ENV_VARS`] from the command environment.
    fn scrub_cluster_secrets(&mut self) -> &mut Self;
}

impl ScrubSecrets for std::process::Command {
    fn scrub_cluster_secrets(&mut self) -> &mut Self {
        for var in AUTH_TOKEN_VARS {
            self.env_remove(var);
        }
        for var in crate::tls::TLS_ENV_VARS {
            self.env_remove(var);
        }
        self
    }
}

impl ScrubSecrets for tokio::process::Command {
    fn scrub_cluster_secrets(&mut self) -> &mut Self {
        for var in AUTH_TOKEN_VARS {
            self.env_remove(var);
        }
        for var in crate::tls::TLS_ENV_VARS {
            self.env_remove(var);
        }
        self
    }
}

/// A transport stream that can be either TCP, TLS, a local Unix domain socket, or a Windows Named Pipe.
pub enum AnyStream {
    Tcp(TcpStream),
    TlsClient(tokio_rustls::client::TlsStream<TcpStream>),
    TlsServer(tokio_rustls::server::TlsStream<TcpStream>),
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    NamedPipe(tokio::net::windows::named_pipe::NamedPipeClient),
}

impl std::fmt::Debug for AnyStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnyStream::Tcp(s) => f.debug_tuple("Tcp").field(s).finish(),
            AnyStream::TlsClient(_) => f
                .debug_tuple("TlsClient")
                .field(&self.peer_addr().ok())
                .finish(),
            AnyStream::TlsServer(_) => f
                .debug_tuple("TlsServer")
                .field(&self.peer_addr().ok())
                .finish(),
            #[cfg(unix)]
            AnyStream::Unix(s) => f.debug_tuple("Unix").field(s).finish(),
            #[cfg(windows)]
            AnyStream::NamedPipe(_) => f.debug_tuple("NamedPipe").finish(),
        }
    }
}

impl AnyStream {
    pub fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        match self {
            AnyStream::Tcp(s) => s.peer_addr(),
            AnyStream::TlsClient(s) => s.get_ref().0.peer_addr(),
            AnyStream::TlsServer(s) => s.get_ref().0.peer_addr(),
            #[cfg(unix)]
            AnyStream::Unix(_) => Err(std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "unix domain socket has no IP peer_addr",
            )),
            #[cfg(windows)]
            AnyStream::NamedPipe(_) => Err(std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "named pipe has no IP peer_addr",
            )),
        }
    }

    pub async fn connect(addr: SocketAddr) -> std::io::Result<Self> {
        connect_stream(addr).await
    }

    pub async fn connect_tcp(addr: SocketAddr) -> std::io::Result<Self> {
        connect_stream(addr).await
    }

    pub async fn connect_stream(addr: SocketAddr) -> std::io::Result<Self> {
        connect_stream(addr).await
    }

    #[cfg(unix)]
    pub async fn connect_unix(path: impl AsRef<Path>) -> std::io::Result<Self> {
        super::client::connect_unix(path).await.map(AnyStream::Unix)
    }

    #[cfg(windows)]
    pub async fn connect_named_pipe(path: impl AsRef<Path>) -> std::io::Result<Self> {
        super::client::connect_named_pipe(path)
            .await
            .map(AnyStream::NamedPipe)
    }
}

impl From<TcpStream> for AnyStream {
    fn from(s: TcpStream) -> Self {
        AnyStream::Tcp(s)
    }
}

impl From<tokio_rustls::client::TlsStream<TcpStream>> for AnyStream {
    fn from(s: tokio_rustls::client::TlsStream<TcpStream>) -> Self {
        AnyStream::TlsClient(s)
    }
}

impl From<tokio_rustls::server::TlsStream<TcpStream>> for AnyStream {
    fn from(s: tokio_rustls::server::TlsStream<TcpStream>) -> Self {
        AnyStream::TlsServer(s)
    }
}

#[cfg(unix)]
impl From<tokio::net::UnixStream> for AnyStream {
    fn from(s: tokio::net::UnixStream) -> Self {
        AnyStream::Unix(s)
    }
}

#[cfg(windows)]
impl From<tokio::net::windows::named_pipe::NamedPipeClient> for AnyStream {
    fn from(s: tokio::net::windows::named_pipe::NamedPipeClient) -> Self {
        AnyStream::NamedPipe(s)
    }
}

impl tokio::io::AsyncRead for AnyStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            AnyStream::Tcp(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            AnyStream::TlsClient(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            AnyStream::TlsServer(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            #[cfg(unix)]
            AnyStream::Unix(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            AnyStream::NamedPipe(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for AnyStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            AnyStream::Tcp(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            AnyStream::TlsClient(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            AnyStream::TlsServer(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            #[cfg(unix)]
            AnyStream::Unix(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            AnyStream::NamedPipe(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            AnyStream::Tcp(s) => std::pin::Pin::new(s).poll_flush(cx),
            AnyStream::TlsClient(s) => std::pin::Pin::new(s).poll_flush(cx),
            AnyStream::TlsServer(s) => std::pin::Pin::new(s).poll_flush(cx),
            #[cfg(unix)]
            AnyStream::Unix(s) => std::pin::Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            AnyStream::NamedPipe(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            AnyStream::Tcp(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            AnyStream::TlsClient(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            AnyStream::TlsServer(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            #[cfg(unix)]
            AnyStream::Unix(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            AnyStream::NamedPipe(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}
