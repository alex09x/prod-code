/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Lightweight UDP discovery for the prod-code cluster.
//!
//! One UDP port (`DISCOVERY_PORT`, 9401) carries both multicast and unicast messages.

mod client;
mod types;
mod wire;

#[cfg(test)]
mod tests;

pub use client::{
    bind_discovery_socket, discover, discover_addrs, discover_with_token, generate_nonce,
};
pub use types::{
    ANNOUNCE_PERIOD, COLLECT_TIMEOUT, DISCOVERY_PORT, DiscoveredNode, LoadedWorkspace,
    MULTICAST_GROUP, canonical_announce_payload, canonical_announce_payload_with_nonce,
    compute_auth_tag, decode_workspace_name, encode_workspace_name, verify_auth_tag,
};
pub use wire::{
    build_reply, format_minimal_node_line, format_node_line, format_node_line_with_nonce,
    format_probe, format_probe_with_nonce, inspect_probe, is_valid_probe, parse_node_line,
    parse_node_line_with_auth, parse_node_line_with_auth_and_nonce, send_announce,
};
