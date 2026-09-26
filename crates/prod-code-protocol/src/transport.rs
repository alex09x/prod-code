//! TCP connections between clients, gateways and their peers: Nagle off, keepalive on so that a
//! peer that went away without closing is noticed, and the cluster's token first when it has
//! one.

use crate::messages::{AuthToken, WireMessage};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

/// Turns Nagle off and TCP keepalive on for `stream`, a connection either side opened.
pub fn tune(stream: &TcpStream) {
    let _ = stream.set_nodelay(true);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL)
        .with_retries(KEEPALIVE_RETRIES);
    let _ = socket2::SockRef::from(stream).set_tcp_keepalive(&keepalive);
}

/// The environment variable that holds the cluster's token.
pub const AUTH_TOKEN_ENV: &str = "PROD_CODE_AUTH_TOKEN";

/// The environment variable that names a file holding the cluster's token.
pub const AUTH_TOKEN_FILE_ENV: &str = "PROD_CODE_AUTH_TOKEN_FILE";

/// The token every connection of this cluster opens with (#402), for clients and gateways
/// alike: [`AUTH_TOKEN_ENV`], else the first line of the file [`AUTH_TOKEN_FILE_ENV`] names,
/// else of `~/.config/prod-code/auth-token`. `None` when none is set, which is the default: a
/// cluster without a token takes every connection.
pub fn auth_token() -> Option<String> {
    let file = std::env::var_os(AUTH_TOKEN_FILE_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".config/prod-code/auth-token"))
        });
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

/// Connects to `addr`, [`tune`]s the connection, and opens it with the cluster's
/// [`auth_token`] when there is one.
pub async fn connect(addr: SocketAddr) -> std::io::Result<TcpStream> {
    connect_with(addr, auth_token().as_deref()).await
}

/// Connects to `addr`, [`tune`]s the connection, and sends `token` as its first frame when one
/// is given.
pub async fn connect_with(addr: SocketAddr, token: Option<&str>) -> std::io::Result<TcpStream> {
    let mut stream = TcpStream::connect(addr).await?;
    tune(&stream);
    if let Some(token) = token {
        let mut frame = bytes::BytesMut::new();
        crate::codec::ProdCodeCodec::new()
            .encode(WireMessage::Auth(AuthToken(token.to_string())), &mut frame)?;
        stream.write_all(&frame).await?;
    }
    Ok(stream)
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
        let (client, accepted) = tokio::join!(connect_with(addr, None), listener.accept());
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
            tokio::join!(connect_with(addr, Some("s3cret")), listener.accept());
        let _client = client.unwrap();
        let mut server = accepted.unwrap().0;
        assert_eq!(
            first_frame(&mut server).await,
            WireMessage::Auth(AuthToken("s3cret".to_string()))
        );

        let (client, accepted) = tokio::join!(connect_with(addr, None), listener.accept());
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
}
