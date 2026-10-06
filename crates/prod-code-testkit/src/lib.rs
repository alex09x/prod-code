/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A gateway that answers from a script, and workspaces to point it at.
//!
//! The tools this repository ships are mostly compositions of analyzer answers: ask for a
//! symbol, ask for a rename, merge what comes back, check the result. The bugs live in the
//! composition, not in the analyzers, so testing them does not need rust-analyzer, gopls or a
//! LAN node — it needs answers of the right shape, on demand, in the right order.
//!
//! [`ScriptedGateway`] is a real TCP server speaking the real wire protocol: it accepts the
//! pre-flight sync, answers the handshake, replies to `initialize` itself, and hands every
//! other LSP request to a closure the test provides. [`Workspace`] is the checkout the client
//! syncs — a temporary directory with a commit in it, because the pre-flight sync measures a
//! delta against `HEAD`.
//!
//! ```no_run
//! # use prod_code_testkit::{ScriptedGateway, Workspace, answers};
//! # async fn example() {
//! let workspace = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
//! let gateway = ScriptedGateway::start(|method, _params| match method {
//!     "textDocument/diagnostic" => answers::no_diagnostics(),
//!     _ => serde_json::Value::Null,
//! })
//! .await;
//! // … drive a tool at `gateway.addr()` against `workspace.root()` …
//! # }
//! ```

pub mod answers;
pub mod gopls;
mod scripted_gateway;
mod workspace;

pub use scripted_gateway::{Answer, LSP_ERROR, ScriptedGateway};
pub use workspace::Workspace;

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use prod_code_protocol::{ProdCodeCodec, WireMessage};
    use tokio::net::TcpStream;
    use tokio_util::codec::Framed;

    #[tokio::test]
    async fn incompatible_handshakes_close_the_scripted_connection() {
        let gateway = ScriptedGateway::start(|_, _| panic!("no LSP query may run")).await;
        let socket = TcpStream::connect(gateway.addr()).await.unwrap();
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        let request = serde_json::from_value(serde_json::json!({
            "type": "HandshakeRequest",
            "payload": {
                "protocol_version": 1,
                "supported_versions": [],
                "client_name": "bad-offer",
                "client_pid": 1,
                "auth_token": null,
                "client_workspace_root": "/workspace"
            }
        }))
        .unwrap();
        framed.send(request).await.unwrap();
        assert!(matches!(
            framed.next().await,
            Some(Ok(WireMessage::Disconnect { .. }))
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), framed.next())
                .await
                .expect("the rejected connection must close")
                .is_none()
        );
        assert_eq!(gateway.calls(), 0);
    }
}
