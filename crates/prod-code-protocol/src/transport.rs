//! TCP connections between clients, gateways and their peers: Nagle off, keepalive on so that a
//! peer that went away without closing is noticed, and the cluster's token first when it has
//! one.

use crate::messages::{AuthToken, WireMessage};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::codec::Encoder;

/// How long a connection may be silent before the kernel starts probing the peer, how far apart
/// the probes are, and how many may go unanswered before the connection is reset. A command on
/// a build node can run for an hour without a byte on the wire, so silence alone proves
/// nothing, but a peer that is gone stops answering probes and the wait ends in about a minute
/// instead of never (#256).
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVE_RETRIES: u32 = 3;

/// The environment variable that configures custom TCP buffer sizes (bytes or e.g. "4M", "8MB", "16MiB").
/// When unset, socket buffers are left to kernel autotuning.
pub const TCP_BUFFER_SIZE_ENV: &str = "PROD_CODE_TCP_BUFFER_SIZE";

/// The environment variable that configures custom TCP receive buffer size.
pub const TCP_RECV_BUFFER_ENV: &str = "PROD_CODE_TCP_RECV_BUFFER";

/// The environment variable that configures custom TCP send buffer size.
pub const TCP_SEND_BUFFER_ENV: &str = "PROD_CODE_TCP_SEND_BUFFER";

/// Parses a human-readable byte size specification (e.g. "4194304", "512K", "4M", "8MB", "16MiB").
pub fn parse_buffer_size(val: &str) -> Option<usize> {
    let s = val.trim();
    if s.is_empty() {
        return None;
    }
    let (num_part, multiplier) = if let Some(stripped) = s
        .strip_suffix("GiB")
        .or_else(|| s.strip_suffix("gib"))
        .or_else(|| s.strip_suffix("GB"))
        .or_else(|| s.strip_suffix("gb"))
        .or_else(|| s.strip_suffix('G'))
        .or_else(|| s.strip_suffix('g'))
    {
        (stripped.trim(), 1024 * 1024 * 1024)
    } else if let Some(stripped) = s
        .strip_suffix("MiB")
        .or_else(|| s.strip_suffix("mib"))
        .or_else(|| s.strip_suffix("MB"))
        .or_else(|| s.strip_suffix("mb"))
        .or_else(|| s.strip_suffix('M'))
        .or_else(|| s.strip_suffix('m'))
    {
        (stripped.trim(), 1024 * 1024)
    } else if let Some(stripped) = s
        .strip_suffix("KiB")
        .or_else(|| s.strip_suffix("kib"))
        .or_else(|| s.strip_suffix("KB"))
        .or_else(|| s.strip_suffix("kb"))
        .or_else(|| s.strip_suffix('K'))
        .or_else(|| s.strip_suffix('k'))
    {
        (stripped.trim(), 1024)
    } else if let Some(stripped) = s.strip_suffix('B').or_else(|| s.strip_suffix('b')) {
        (stripped.trim(), 1)
    } else {
        (s, 1)
    };

    num_part
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
}

/// Configures socket buffer sizes on `stream` according to explicit options, alongside TCP keepalive and nodelay.
pub fn tune_with_buffer_sizes(stream: &TcpStream, recv_size: Option<usize>, send_size: Option<usize>) {
    let _ = stream.set_nodelay(true);
    let sock = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL)
        .with_retries(KEEPALIVE_RETRIES);
    let _ = sock.set_tcp_keepalive(&keepalive);

    if let Some(sz) = recv_size {
        let _ = sock.set_recv_buffer_size(sz);
    }
    if let Some(sz) = send_size {
        let _ = sock.set_send_buffer_size(sz);
    }
}

/// Turns Nagle off, TCP keepalive on, and applies custom socket buffer sizes if configured in the environment.
pub fn tune(stream: &TcpStream) {
    let general = std::env::var(TCP_BUFFER_SIZE_ENV)
        .ok()
        .and_then(|v| parse_buffer_size(&v));
    let rcv = std::env::var(TCP_RECV_BUFFER_ENV)
        .ok()
        .and_then(|v| parse_buffer_size(&v))
        .or(general);
    let snd = std::env::var(TCP_SEND_BUFFER_ENV)
        .ok()
        .and_then(|v| parse_buffer_size(&v))
        .or(general);

    tune_with_buffer_sizes(stream, rcv, snd);
}

/// The environment variable that holds the cluster's token.
pub const AUTH_TOKEN_ENV: &str = "PROD_CODE_AUTH_TOKEN";

/// The environment variable that names a Unix domain socket path for local connections.
pub const SOCKET_PATH_ENV: &str = "PROD_CODE_SOCKET";

/// The environment variable that names a file holding the cluster's token.
pub const AUTH_TOKEN_FILE_ENV: &str = "PROD_CODE_AUTH_TOKEN_FILE";

/// Both variables, which a gateway removes from the environment of every command it runs: a
/// build or a test on a node gets neither the cluster's secret nor a token to send to the mock
/// gateways of its own tests.
pub const AUTH_TOKEN_VARS: [&str; 2] = [AUTH_TOKEN_ENV, AUTH_TOKEN_FILE_ENV];

/// The token every connection of this cluster opens with (#402), for clients and gateways
/// alike: [`AUTH_TOKEN_ENV`], else the first line of the file [`AUTH_TOKEN_FILE_ENV`] names.
/// `None` when neither is set, which is the default: a cluster without a token takes every
/// connection. No file is read unless a variable names it, so a process started without them
/// (a command a gateway runs) never picks a token up.
pub fn auth_token() -> Option<String> {
    let file = std::env::var_os(AUTH_TOKEN_FILE_ENV).map(PathBuf::from);
    resolve_token(
        std::env::var(AUTH_TOKEN_ENV).ok().as_deref(),
        file.as_deref(),
    )
}

/// The token from the variable's value `env` when it has one, otherwise from the first line of
/// `file`; blank ones do not count.
fn resolve_token(env: Option<&str>, file: Option<&Path>) -> Option<String> {
    if let Some(token) = env.map(str::trim).filter(|t| !t.is_empty()) {
        return Some(token.to_string());
    }
    let text = std::fs::read_to_string(file?).ok()?;
    let token = text.lines().next()?.trim();
    (!token.is_empty()).then(|| token.to_string())
}


/// Establishes a raw tuned TCP connection to `addr` with backoff retries for transient ARP/network errors.
///
/// This is crate-private to prevent downstream callers from bypassing TLS enforcement (#Phase 5.6).
pub(crate) async fn connect_raw_tcp(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let mut last_err = None;
    for attempt in 0..3 {
        if attempt > 0 {
            let backoff = match last_err.as_ref().and_then(|e: &std::io::Error| e.raw_os_error()) {
                // On macOS, EHOSTUNREACH (65) occurs when ARP entry is resolving.
                // Give ARP resolution time to complete.
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
            format!("Refusing plaintext TCP connection to {addr}: TLS mode '{mode:?}' is strictly required"),
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
pub type BuiltClientTls = (Arc<rustls::ClientConfig>, rustls::pki_types::ServerName<'static>);

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
    DEFAULT_CLIENT_TLS.read().ok().and_then(|guard| guard.clone())
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

/// Connects to `addr`, negotiating TLS 1.3 / mTLS using the provided client TLS configuration (or falling back to
/// the process default / environment), transmitting `token` strictly inside the TLS encrypted channel.
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
            format!("TLS is strictly required by PROD_CODE_TLS_MODE ({mode:?}), but no valid client TLS credentials/CA or pins are configured"),
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

/// Connects to `addr`, negotiating TLS 1.3 / mTLS if configured, and transmitting `token`
/// strictly inside the TLS encrypted channel. Never falls back to plaintext if TLS mode is Strict or Mutual.
pub async fn connect_stream_with(
    addr: SocketAddr,
    token: Option<&str>,
) -> std::io::Result<AnyStream> {
    connect_stream_with_client_config(addr, token, None).await
}

/// Connects to `addr`, [`tune`]s the connection, and sends `token` as its first frame when one
/// is given. When TLS is configured or required, establishes a TLS 1.3 channel before sending credentials.
pub async fn connect_with(addr: SocketAddr, token: Option<&str>) -> std::io::Result<AnyStream> {
    connect_stream_with(addr, token).await
}

/// Connects to `addr` using the default auth token and TLS 1.3 negotiation when configured.
pub async fn connect(addr: SocketAddr) -> std::io::Result<AnyStream> {
    connect_stream(addr).await
}

/// Connects to the Unix domain socket at `path` and opens it with the cluster's
/// [`auth_token`] when there is one.
#[cfg(unix)]
pub async fn connect_unix(path: impl AsRef<Path>) -> std::io::Result<tokio::net::UnixStream> {
    connect_unix_with(path, auth_token().as_deref()).await
}

/// Connects to the Unix domain socket at `path` and sends `token` as its first frame when one
/// is given.
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

/// Connects to the Windows named pipe at `path` and opens it with the cluster's
/// [`auth_token`] when there is one.
#[cfg(windows)]
pub async fn connect_named_pipe(
    path: impl AsRef<Path>,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    connect_named_pipe_with(path, auth_token().as_deref()).await
}

/// Connects to the Windows named pipe at `path` and sends `token` as its first frame when one
/// is given.
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
            AnyStream::TlsClient(_) => f.debug_tuple("TlsClient").field(&self.peer_addr().ok()).finish(),
            AnyStream::TlsServer(_) => f.debug_tuple("TlsServer").field(&self.peer_addr().ok()).finish(),
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
        connect_unix(path).await.map(AnyStream::Unix)
    }

    #[cfg(windows)]
    pub async fn connect_named_pipe(path: impl AsRef<Path>) -> std::io::Result<Self> {
        connect_named_pipe(path).await.map(AnyStream::NamedPipe)
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

/// Maximum aggregate size of the textual headers of one language-server message.
const MAX_LSP_HEADER_BYTES: usize = 64 * 1024;

/// Reads one complete UTF-8 LSP frame, including all headers before the blank line.
///
/// Clean EOF between frames returns `None`. Malformed or truncated input is an error:
/// callers must retire the stream rather than try to find a new frame inside its body.
/// Headers are limited to 64 KiB and the body to the wire codec's 256 MiB limit.
pub async fn read_lsp_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<String>> {
    read_lsp_frame_with_limits(reader, MAX_LSP_HEADER_BYTES, crate::codec::MAX_FRAME_SIZE).await
}

async fn read_lsp_frame_with_limits<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    max_headers: usize,
    max_body: usize,
) -> std::io::Result<Option<String>> {
    use std::io::{Error, ErrorKind};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};

    let invalid = |message| Error::new(ErrorKind::InvalidData, message);
    let truncated = |message| Error::new(ErrorKind::UnexpectedEof, message);
    let mut header_bytes = 0;
    let mut length = None;
    let mut has_content_type = false;
    loop {
        let remaining = max_headers - header_bytes;
        if remaining == 0 {
            return Err(invalid("LSP headers exceed the size limit"));
        }
        let mut line = Vec::new();
        // Limit the read itself: a child can emit an endless line with no newline.
        let count = (&mut *reader)
            .take(remaining as u64)
            .read_until(b'\n', &mut line)
            .await?;
        if count == 0 {
            return if header_bytes == 0 {
                Ok(None)
            } else {
                Err(truncated("EOF inside LSP headers"))
            };
        }
        if line.last() != Some(&b'\n') {
            return Err(if count == remaining {
                invalid("LSP headers exceed the size limit")
            } else {
                truncated("EOF inside an LSP header line")
            });
        }
        header_bytes += count;
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            break;
        }
        if !line.is_ascii() {
            return Err(invalid("LSP headers must be ASCII"));
        }
        let line = std::str::from_utf8(&line).expect("ASCII was checked");
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("Malformed LSP header"))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(invalid("Duplicate LSP Content-Length"));
            }
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid("Invalid LSP Content-Length"));
            }
            let parsed = value
                .parse::<usize>()
                .map_err(|_| invalid("Invalid LSP Content-Length"))?;
            if parsed == 0 || parsed > max_body {
                return Err(invalid("LSP body length exceeds the allowed range"));
            }
            length = Some(parsed);
        } else if name.eq_ignore_ascii_case("Content-Type") {
            if has_content_type {
                return Err(invalid("Duplicate LSP Content-Type"));
            }
            has_content_type = true;
            for parameter in value.split(';').skip(1) {
                if let Some((key, encoding)) = parameter.trim().split_once('=')
                    && key.trim().eq_ignore_ascii_case("charset")
                {
                    let encoding = encoding.trim().trim_matches('"');
                    if !encoding.eq_ignore_ascii_case("utf-8")
                        && !encoding.eq_ignore_ascii_case("utf8")
                    {
                        return Err(invalid("LSP bodies must use UTF-8"));
                    }
                }
            }
        }
    }
    let length = length.ok_or_else(|| invalid("Missing LSP Content-Length"))?;
    // Grow only as bytes arrive, rather than preallocating a child's declared length.
    let mut body = Vec::new();
    (&mut *reader)
        .take(length as u64)
        .read_to_end(&mut body)
        .await?;
    if body.len() != length {
        return Err(truncated("EOF inside LSP body"));
    }
    String::from_utf8(body)
        .map(Some)
        .map_err(|_| invalid("LSP body is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio_util::codec::Decoder;

    /// The first frame `stream` receives.
    async fn first_frame(stream: &mut TcpStream) -> WireMessage {
        let mut codec = crate::ProdCodeCodec::new();
        let mut buffer = bytes::BytesMut::new();
        loop {
            if let Some(message) = codec.decode(&mut buffer).unwrap() {
                return message;
            }
            assert!(stream.read_buf(&mut buffer).await.unwrap() > 0, "closed");
        }
    }

    #[tokio::test]
    async fn a_connection_has_keepalive_and_no_nagle_on_both_ends() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (client, accepted) = tokio::join!(connect_raw_tcp(addr), listener.accept());
        let client = client.unwrap();
        let (server, _) = accepted.unwrap();
        tune(&server);
        for stream in [&client, &server] {
            let socket = socket2::SockRef::from(stream);
            assert!(stream.nodelay().unwrap());
            assert!(socket.keepalive().unwrap());
            assert_eq!(socket.tcp_keepalive_time().unwrap(), KEEPALIVE_IDLE);
            assert_eq!(socket.tcp_keepalive_interval().unwrap(), KEEPALIVE_INTERVAL);
            assert_eq!(socket.tcp_keepalive_retries().unwrap(), KEEPALIVE_RETRIES);
        }
    }

    /// With a token the connection's first frame is the token; without one nothing is sent
    /// before the caller's own frames (#402).
    #[tokio::test]
    async fn a_connection_opens_with_the_token_when_there_is_one() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (client, accepted) =
            tokio::join!(connect_raw_tcp_with(addr, Some("s3cret")), listener.accept());
        let _client = client.unwrap();
        let mut server = accepted.unwrap().0;
        assert_eq!(
            first_frame(&mut server).await,
            WireMessage::Auth(AuthToken("s3cret".to_string()))
        );

        let (client, accepted) = tokio::join!(connect_raw_tcp_with(addr, None), listener.accept());
        let mut client = client.unwrap();
        let mut ping = bytes::BytesMut::new();
        crate::ProdCodeCodec::new()
            .encode(WireMessage::Ping, &mut ping)
            .unwrap();
        client.write_all(&ping).await.unwrap();
        let mut server = accepted.unwrap().0;
        assert_eq!(first_frame(&mut server).await, WireMessage::Ping);
    }

    /// The variable wins over the file, the file's first line is the token, and blank ones do
    /// not count; the token never shows in a message's `Debug` (#402).
    #[test]
    fn the_token_comes_from_the_variable_or_the_file_and_never_shows() {
        let dir = std::env::temp_dir().join(format!("prod-code-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("auth-token");
        std::fs::write(&file, "  from-file  \nsecond line\n").unwrap();
        assert_eq!(
            resolve_token(Some(" from-env "), Some(&file)).as_deref(),
            Some("from-env")
        );
        assert_eq!(
            resolve_token(Some("   "), Some(&file)).as_deref(),
            Some("from-file")
        );
        assert_eq!(resolve_token(None, Some(&dir.join("none"))), None);
        std::fs::write(&file, "\n").unwrap();
        assert_eq!(resolve_token(None, Some(&file)), None);
        let _ = std::fs::remove_dir_all(&dir);

        let message = WireMessage::Auth(AuthToken("s3cret".to_string()));
        assert!(!format!("{message:?}").contains("s3cret"));
        let token = AuthToken("s3cret".to_string());
        assert!(token.matches("s3cret"));
        assert!(!token.matches("s3creT"));
        assert!(!token.matches("s3cret2"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_transport_supports_framing_and_auth_token() {
        let dir = std::env::temp_dir().join(format!("prod-code-test-sock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock_path = dir.join("test.sock");
        let _ = std::fs::remove_file(&sock_path);
        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

        let (client, accepted) = tokio::join!(
            connect_unix_with(&sock_path, Some("unix-secret")),
            listener.accept()
        );
        let client = client.unwrap();
        let mut server = accepted.unwrap().0;

        let mut codec = crate::ProdCodeCodec::new();
        let mut buffer = bytes::BytesMut::new();
        loop {
            if let Some(message) = codec.decode(&mut buffer).unwrap() {
                assert_eq!(
                    message,
                    WireMessage::Auth(AuthToken("unix-secret".to_string()))
                );
                break;
            }
            assert!(server.read_buf(&mut buffer).await.unwrap() > 0);
        }

        // Test AnyStream wrapping
        let mut any_client = AnyStream::Unix(client);
        let mut any_server = AnyStream::Unix(server);
        let mut ping = bytes::BytesMut::new();
        crate::ProdCodeCodec::new()
            .encode(WireMessage::Ping, &mut ping)
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut any_client, &ping)
            .await
            .unwrap();

        let mut buf2 = bytes::BytesMut::new();
        loop {
            if let Some(msg) = codec.decode(&mut buf2).unwrap() {
                assert_eq!(msg, WireMessage::Ping);
                break;
            }
            assert!(
                tokio::io::AsyncReadExt::read_buf(&mut any_server, &mut buf2)
                    .await
                    .unwrap()
                    > 0
            );
        }
    }
}

#[cfg(test)]
mod lsp_frame_tests {
    use super::*;
    use std::io::ErrorKind;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn optional_headers_in_either_order_preserve_successive_utf8_frames() {
        for headers in [
            "Content-Length: 4\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n",
            "Content-Type: application/vscode-jsonrpc; Charset=UTF8\r\ncontent-length: 4\r\n",
            "X-Extension: ignored\r\nCONTENT-LENGTH: 4\r\nContent-Type: application/vscode-jsonrpc; charset=\"UTF-8\"; ignored=yes\r\n",
            "Content-Length: 4\r\nContent-Type: application/vscode-jsonrpc\r\n",
        ] {
            let frames = format!("{headers}\r\n\"é\"Content-Length: 2\r\n\r\n{{}}");
            let mut reader = frames.as_bytes();
            assert_eq!(
                read_lsp_frame(&mut reader).await.unwrap().as_deref(),
                Some("\"é\"")
            );
            assert_eq!(
                read_lsp_frame(&mut reader).await.unwrap().as_deref(),
                Some("{}")
            );
            assert!(read_lsp_frame(&mut reader).await.unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn partial_pipe_reads_and_lf_headers_are_supported() {
        let (mut writer, reader) = tokio::io::duplex(2);
        let sender = tokio::spawn(async move {
            for byte in b"Content-Length: 2\n\n{}" {
                writer.write_all(&[*byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_lsp_frame(&mut reader).await.unwrap().as_deref(),
            Some("{}")
        );
        sender.await.unwrap();
        assert!(read_lsp_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn malformed_headers_and_bodies_refuse_instead_of_resynchronizing() {
        for bytes in [
            b"\r\n".as_slice(),
            b"X: y\r\n\r\n",
            b"bad header\r\n\r\n",
            b"Content-Length: \r\n\r\n",
            b"Content-Length: +2\r\n\r\n{}",
            b"Content-Length: -1\r\n\r\n",
            b"Content-Length: 0\r\n\r\n",
            b"Content-Length: 9999999999999999999999999999999\r\n\r\n",
            b"Content-Length: 268435457\r\n\r\n",
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            b"Content-Length: 2\r\nContent-Type: a\r\nContent-Type: b\r\n\r\n{}",
            b"Content-Length: 2\r\nContent-Type: application/json; charset=utf-16\r\n\r\n{}",
            b"X: \xff\r\nContent-Length: 2\r\n\r\n{}",
            b"Content-Length: 1\r\n\r\n\xff",
        ] {
            let mut reader = bytes;
            assert_eq!(
                read_lsp_frame(&mut reader).await.unwrap_err().kind(),
                ErrorKind::InvalidData,
                "{bytes:?}"
            );
        }
        for bytes in [
            b"Content-Len".as_slice(),
            b"Content-Length: 2\r\n",
            b"Content-Length: 2\r\n\r\n{",
        ] {
            let mut reader = bytes;
            assert_eq!(
                read_lsp_frame(&mut reader).await.unwrap_err().kind(),
                ErrorKind::UnexpectedEof,
                "{bytes:?}"
            );
        }
    }

    #[tokio::test]
    async fn size_limits_apply_while_reading_and_include_the_complete_header_block() {
        let (mut writer, reader) = tokio::io::duplex(128);
        writer.write_all(&[b'A'; 65]).await.unwrap();
        // Keep the pipe open: rejection cannot depend on a newline or EOF.
        let mut reader = BufReader::new(reader);
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            read_lsp_frame_with_limits(&mut reader, 64, 16),
        )
        .await
        .expect("bounded headers must fail without waiting for newline")
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        let mut tail = [0];
        reader.read_exact(&mut tail).await.unwrap();
        assert_eq!(tail, [b'A']);
        let frame = b"Content-Length: 2\r\n\r\n{}";
        let mut exact = frame.as_slice();
        assert_eq!(
            read_lsp_frame_with_limits(&mut exact, frame.len() - 2, 2)
                .await
                .unwrap()
                .as_deref(),
            Some("{}")
        );
        let mut too_short = frame.as_slice();
        assert_eq!(
            read_lsp_frame_with_limits(&mut too_short, frame.len() - 4, 2)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidData
        );
        let mut too_long = frame.as_slice();
        assert_eq!(
            read_lsp_frame_with_limits(&mut too_long, 64, 1)
                .await
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn any_stream_from_tcp() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let any: AnyStream = client.into();
        match any {
            AnyStream::Tcp(_) => {}
            _ => panic!("expected Tcp"),
        }
    }

    #[tokio::test]
    async fn raw_tcp_with_fails_closed_when_tls_required_even_without_token() {
        let _lock = crate::tls::tests::TEST_ENV_LOCK.lock().await;
        unsafe {
            std::env::set_var(crate::tls::TLS_MODE_ENV, "strict");
        }
        let dummy_addr: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
        let err = connect_raw_tcp_with(dummy_addr, None).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        unsafe {
            std::env::remove_var(crate::tls::TLS_MODE_ENV);
        }
    }


    #[tokio::test]
    async fn raw_tcp_with_fails_on_invalid_mode() {
        let _lock = crate::tls::tests::TEST_ENV_LOCK.lock().await;
        unsafe {
            std::env::set_var(crate::tls::TLS_MODE_ENV, "stricts");
        }
        let dummy_addr: std::net::SocketAddr = "127.0.0.1:9".parse().unwrap();
        let err = connect_raw_tcp_with(dummy_addr, None).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        unsafe {
            std::env::remove_var(crate::tls::TLS_MODE_ENV);
        }
    }

    #[tokio::test]
    async fn connect_stream_with_succeeds_in_mutual_mode_after_caching_and_scrubbing_key_env() {
        use crate::tls::{ServerTlsConfig, upgrade_server_stream};
        use tokio::io::AsyncReadExt;

        let _lock = crate::tls::tests::TEST_ENV_LOCK.lock().await;
        clear_client_tls_cache();

        let mut client_cert_params =
            rcgen::CertificateParams::new(vec!["gateway-outbound".to_string()]).unwrap();
        client_cert_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "gateway-outbound");
        let client_key_pair = rcgen::KeyPair::generate().unwrap();
        let client_ca_key = rcgen::KeyPair::generate().unwrap();
        let client_ca_cert =
            rcgen::CertificateParams::new(vec!["client-ca".to_string()]).unwrap()
                .self_signed(&client_ca_key)
                .unwrap();
        let signed_client_cert = client_cert_params
            .signed_by(&client_key_pair, &client_ca_cert, &client_ca_key)
            .unwrap();

        let cert_file = tempfile::NamedTempFile::new().unwrap();
        let key_file = tempfile::NamedTempFile::new().unwrap();
        let ca_file = tempfile::NamedTempFile::new().unwrap();

        std::fs::write(cert_file.path(), signed_client_cert.pem()).unwrap();
        std::fs::write(key_file.path(), client_key_pair.serialize_pem()).unwrap();

        let mut ca_params = rcgen::CertificateParams::new(vec!["server-ca".to_string()]).unwrap();
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "server-ca");
        let ca_key_pair = rcgen::KeyPair::generate().unwrap();
        let ca_cert_obj = ca_params.self_signed(&ca_key_pair).unwrap();
        std::fs::write(ca_file.path(), ca_cert_obj.pem()).unwrap();

        let mut server_params =
            rcgen::CertificateParams::new(vec!["cluster-peer".to_string()]).unwrap();
        server_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "cluster-peer");
        let s_key = rcgen::KeyPair::generate().unwrap();
        let s_cert = server_params
            .signed_by(&s_key, &ca_cert_obj, &ca_key_pair)
            .unwrap();

        let s_cert_der = rustls::pki_types::CertificateDer::from(s_cert.der().to_vec());
        let s_key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(s_key.serialize_der()),
        );

        let mut client_roots = rustls::RootCertStore::empty();
        client_roots
            .add(rustls::pki_types::CertificateDer::from(client_ca_cert.der().to_vec()))
            .unwrap();

        let server_tls = ServerTlsConfig::new(vec![s_cert_der], s_key_der)
            .with_client_ca(client_roots, true)
            .build()
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut tls_stream = upgrade_server_stream(stream, server_tls).await.unwrap();
            let mut token_buf = [0u8; 100];
            let n = tls_stream.read(&mut token_buf).await.unwrap();
            assert!(n > 0);
        });

        unsafe {
            std::env::set_var(crate::tls::TLS_MODE_ENV, "mutual");
            std::env::set_var(crate::tls::TLS_CERT_ENV, cert_file.path());
            std::env::set_var(crate::tls::TLS_KEY_ENV, key_file.path());
            std::env::set_var(crate::tls::TLS_CA_ENV, ca_file.path());
            std::env::set_var(crate::tls::TLS_SERVER_NAME_ENV, "cluster-peer");
        }

        init_client_tls_from_env().unwrap();

        // Simulate gateway scrubbing TLS_KEY_ENV at startup:
        unsafe {
            std::env::remove_var(crate::tls::TLS_KEY_ENV);
        }

        // Direct ClientTlsConfig::from_env() now fails:
        assert!(crate::tls::ClientTlsConfig::from_env().is_err());

        // But connect_stream_with uses cached default client TLS configuration and succeeds:
        let stream = connect_stream_with(addr, Some("cluster-token")).await.unwrap();
        match stream {
            AnyStream::TlsClient(_) => {}
            _ => panic!("expected TlsClient stream"),
        }

        server_task.await.unwrap();

        clear_client_tls_cache();
        unsafe {
            std::env::remove_var(crate::tls::TLS_MODE_ENV);
            std::env::remove_var(crate::tls::TLS_CERT_ENV);
            std::env::remove_var(crate::tls::TLS_CA_ENV);
            std::env::remove_var(crate::tls::TLS_SERVER_NAME_ENV);
        }
    }

    #[test]
    fn test_parse_buffer_size() {
        assert_eq!(parse_buffer_size(""), None);
        assert_eq!(parse_buffer_size("   "), None);
        assert_eq!(parse_buffer_size("4194304"), Some(4_194_304));
        assert_eq!(parse_buffer_size("64K"), Some(64 * 1024));
        assert_eq!(parse_buffer_size("512KiB"), Some(512 * 1024));
        assert_eq!(parse_buffer_size("4M"), Some(4 * 1024 * 1024));
        assert_eq!(parse_buffer_size("8MB"), Some(8 * 1024 * 1024));
        assert_eq!(parse_buffer_size("16MiB"), Some(16 * 1024 * 1024));
        assert_eq!(parse_buffer_size("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_buffer_size("invalid"), None);
    }

    #[tokio::test]
    async fn test_tune_socket_buffers_applied() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        tune_with_buffer_sizes(&stream, Some(1024 * 1024), Some(1024 * 1024));

        let sock = socket2::SockRef::from(&stream);
        let recv_buf = sock.recv_buffer_size().unwrap();
        let send_buf = sock.send_buffer_size().unwrap();
        assert!(
            recv_buf >= 1024 * 1024,
            "recv buffer must be at least 1MB, got {}",
            recv_buf
        );
        assert!(
            send_buf >= 1024 * 1024,
            "send buffer must be at least 1MB, got {}",
            send_buf
        );
    }
}

