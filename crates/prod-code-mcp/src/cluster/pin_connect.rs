/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The one gate every connection that syncs a checkout or can run work passes (#1793): the
//! node must be one the checkout's policy allows, and under a strict pin the gateway must
//! confirm the pin on that very connection before any sync, handshake or command is sent. The
//! transport opens the connection as usual, its token and TLS included; the pin request is the
//! first frame after them, so a gateway too old to know it closes the connection having seen
//! nothing of the workspace.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};
use prod_code_protocol::{AnyStream, ProdCodeCodec};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;

use super::pin::NodePolicy;

/// A connection that passed the gate.
pub struct GatedConnection {
    pub framed: Framed<AnyStream, ProdCodeCodec>,
    /// The node the gateway confirmed it executes the work on, under a strict pin.
    pub confirmed_node: Option<String>,
}

/// Opens a connection to `remote` for `action` on the checkout at `root`, under that
/// checkout's policy.
pub async fn connect_gated(
    remote: SocketAddr,
    root: &Path,
    action: &str,
) -> Result<GatedConnection> {
    let policy = super::pin_scope::policy_for_root(root)?;
    connect_gated_with(remote, root, &policy, action).await
}

/// [`connect_gated`] under `policy`.
pub async fn connect_gated_with(
    remote: SocketAddr,
    root: &Path,
    policy: &NodePolicy,
    action: &str,
) -> Result<GatedConnection> {
    policy.check(remote, action)?;
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let confirmed_node = confirm_gate(&mut framed, remote, root, policy).await?;
    Ok(GatedConnection {
        framed,
        confirmed_node,
    })
}

/// On a connection opened to `remote` and not yet used: under a strict pin, the gateway's
/// confirmation and the node it names; nothing otherwise.
pub async fn confirm_gate<S>(
    framed: &mut Framed<S, ProdCodeCodec>,
    remote: SocketAddr,
    root: &Path,
    policy: &NodePolicy,
) -> Result<Option<String>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match policy.strict_endpoint() {
        Some(endpoint) => super::pin_wire::confirm_pin(framed, &endpoint.name, remote, root)
            .await
            .map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
#[path = "pin_connect_tests.rs"]
mod tests;
