/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    AnyStream, HandshakeRequest, ProdCodeCodec, SyncProbeResponse, SyncResponse, WireMessage,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

pub async fn socket_pair() -> (
    Framed<AnyStream, ProdCodeCodec>,
    Framed<TcpStream, ProdCodeCodec>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (peer, _) = listener.accept().await.unwrap();
    (
        Framed::new(AnyStream::Tcp(socket), ProdCodeCodec::new()),
        Framed::new(peer, ProdCodeCodec::new()),
    )
}

pub async fn handshake(peer: &mut Framed<TcpStream, ProdCodeCodec>) -> HandshakeRequest {
    loop {
        match peer.next().await.unwrap().unwrap() {
            WireMessage::SyncProbeRequest(req) => peer
                .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                    server_workspace_root: req.client_workspace_root,
                    seeded: false,
                    files_deleted: 0,
                    missing: vec![],
                }))
                .await
                .unwrap(),
            WireMessage::SyncRequest(req) => peer
                .send(WireMessage::SyncResponse(SyncResponse {
                    server_workspace_root: req.client_workspace_root,
                    files_updated: 0,
                    files_deleted: 0,
                    bytes_transferred: 0,
                    duration_ms: 0,
                    workspace_was_fresh: false,
                    stale_paths: vec![],
                }))
                .await
                .unwrap(),
            WireMessage::HandshakeRequest(req) => return req,
            other => panic!("unexpected setup message {other:?}"),
        }
    }
}
