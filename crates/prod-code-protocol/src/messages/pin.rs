/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Strict node pins (#1793): a client whose workspace is pinned to one gateway asks that gateway,
//! on the connection it is about to sync and run a command on, to confirm that it executes the
//! work itself and relocates nothing. A gateway older than this message cannot decode it and
//! closes the connection, so the client refuses before any workspace sync or command is sent,
//! instead of trusting a gateway that silently ignores a field it does not know.

use serde::{Deserialize, Serialize};

/// Asks the gateway to hold this connection to the node it reached.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodePinRequest {
    /// The logical endpoint the workspace is pinned to, as the client configured it
    /// (`host:port`).
    pub endpoint: String,
    /// The address the client connected to for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_to: Option<String>,
    /// The checkout the pin belongs to.
    #[serde(default)]
    pub client_workspace_root: String,
}

/// The gateway's answer to [`NodePinRequest`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NodePinResponse {
    /// The gateway runs every later request of this connection itself: it sends no redirect
    /// and hands no command to another node.
    pub accepted: bool,
    /// The address this gateway advertises to its cluster: the node that executes the work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Why the pin was refused, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WireMessage;

    #[test]
    fn issue_1793_pin_frames_round_trip_with_their_tag() {
        let request = WireMessage::NodePinRequest(NodePinRequest {
            endpoint: "build.example:9400".into(),
            connected_to: Some("[2001:db8::1]:9400".into()),
            client_workspace_root: "/work/app".into(),
        });
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains("\"type\":\"NodePinRequest\""), "{text}");
        assert_eq!(serde_json::from_str::<WireMessage>(&text).unwrap(), request);

        let answer = WireMessage::NodePinResponse(NodePinResponse {
            accepted: true,
            node: Some("build.example:9400".into()),
            reason: None,
        });
        let text = serde_json::to_string(&answer).unwrap();
        assert_eq!(serde_json::from_str::<WireMessage>(&text).unwrap(), answer);
    }

    #[test]
    fn issue_1793_minimal_pin_payloads_decode_with_defaults() {
        let request: WireMessage = serde_json::from_str(
            r#"{"type":"NodePinRequest","payload":{"endpoint":"build.example:9400"}}"#,
        )
        .unwrap();
        assert_eq!(
            request,
            WireMessage::NodePinRequest(NodePinRequest {
                endpoint: "build.example:9400".into(),
                ..Default::default()
            })
        );
        let answer: WireMessage =
            serde_json::from_str(r#"{"type":"NodePinResponse","payload":{"accepted":false}}"#)
                .unwrap();
        assert_eq!(
            answer,
            WireMessage::NodePinResponse(NodePinResponse::default())
        );
    }

    #[test]
    fn issue_1793_existing_messages_keep_their_encoding() {
        // The pin is a separate message: requests an older gateway already reads are unchanged.
        let status = serde_json::to_string(&WireMessage::StatusRequest).unwrap();
        assert_eq!(status, r#"{"type":"StatusRequest"}"#);
    }
}
