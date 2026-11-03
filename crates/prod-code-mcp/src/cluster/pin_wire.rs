/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The client half of a strict pin on the wire (#1793): before a pinned workspace is synced to a
//! gateway or a command runs there, the gateway must confirm on that same connection that it runs
//! the work itself. A gateway too old to know the request closes the connection; that, a refusal
//! or no answer at all stops the command before anything reached the node.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{NodePinRequest, ProdCodeCodec, WireMessage};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;

/// How long a gateway has to confirm a pin.
pub const PIN_CONFIRM_TIMEOUT: Duration = Duration::from_secs(10);

/// Asks the gateway behind `framed`, reached at `connected_to`, to hold this connection's work
/// to itself, and returns the node it names. Nothing else has been sent on the connection yet.
pub async fn confirm_pin<S>(
    framed: &mut Framed<S, ProdCodeCodec>,
    endpoint: &str,
    connected_to: SocketAddr,
    root: &Path,
) -> Result<String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    confirm_pin_within(framed, endpoint, connected_to, root, PIN_CONFIRM_TIMEOUT).await
}

/// [`confirm_pin`] waiting at most `limit` for the answer.
pub async fn confirm_pin_within<S>(
    framed: &mut Framed<S, ProdCodeCodec>,
    endpoint: &str,
    connected_to: SocketAddr,
    root: &Path,
    limit: Duration,
) -> Result<String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let refuse = |why: String| -> anyhow::Error {
        anyhow::anyhow!(
            "the gateway at {connected_to} did not confirm the node pin to {endpoint}: {why}; \
             no workspace sync or command was sent. Upgrade that gateway, or run unpinned with \
             `--pin-node auto`"
        )
    };
    framed
        .send(WireMessage::NodePinRequest(NodePinRequest {
            endpoint: endpoint.to_string(),
            connected_to: Some(connected_to.to_string()),
            client_workspace_root: root.to_string_lossy().to_string(),
        }))
        .await
        .map_err(|err| refuse(format!("sending the request failed ({err})")))?;
    let answer = match tokio::time::timeout(limit, framed.next()).await {
        Err(_) => return Err(refuse(format!("no answer in {}s", limit.as_secs()))),
        Ok(answer) => answer,
    };
    match answer {
        // The confirmed node belongs to this connection only: it is returned to the caller that
        // reports the run, never kept for a later connection to the same address.
        Some(Ok(WireMessage::NodePinResponse(resp))) if resp.accepted => {
            Ok(resp.node.unwrap_or_else(|| connected_to.to_string()))
        }
        Some(Ok(WireMessage::NodePinResponse(resp))) => Err(refuse(format!(
            "it refused ({})",
            resp.reason.unwrap_or_else(|| "no reason given".to_string())
        ))),
        Some(Ok(WireMessage::Disconnect { reason })) => {
            Err(refuse(format!("it disconnected ({reason})")))
        }
        Some(Ok(other)) => Err(refuse(format!("it answered with {}", kind(&other)))),
        Some(Err(err)) => Err(refuse(format!(
            "the connection failed ({err}), as a gateway older than node pins closes it"
        ))),
        None => Err(refuse(
            "it closed the connection, as a gateway older than node pins does".to_string(),
        )),
    }
}

fn kind(message: &WireMessage) -> String {
    serde_json::to_value(message)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| "an unknown message".to_string())
}

#[cfg(test)]
#[path = "pin_wire_tests.rs"]
mod tests;
