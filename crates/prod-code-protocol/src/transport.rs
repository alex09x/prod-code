//! TCP connections between clients, gateways and their peers: Nagle off, and keepalive on so
//! that a peer that went away without closing is noticed.

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpStream;

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

/// Connects to `addr` and [`tune`]s the connection.
pub async fn connect(addr: SocketAddr) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect(addr).await?;
    tune(&stream);
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_connection_has_keepalive_and_no_nagle_on_both_ends() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (client, accepted) = tokio::join!(connect(addr), listener.accept());
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
}
