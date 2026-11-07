/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Multiple gateways (Phase 5.1, client side): a workspace is placed on one node by
//! rendezvous hashing over the reachable nodes, the placement is remembered locally so all
//! sessions of that checkout keep hitting the node whose engine and build cache are warm, and
//! a dead node fails over to the next one.

pub mod discover;
pub mod parse;
pub mod pick;
pub mod pin;
pub mod pin_connect;
#[cfg(test)]
pub(crate) mod pin_fake;
pub mod pin_scope;
pub mod pin_wire;
pub mod placement;
pub mod rebalance;
pub mod routing;
pub mod selection;

#[cfg(test)]
mod tests;

pub use discover::{
    ask_placement, ask_placement_opt, ask_placement_preview, cluster_view, discover_nodes,
    discover_nodes_with_paths, forget_peer, node_metrics, node_status,
};
pub use parse::{discover_auto_nodes_sync, parse_remotes, resolve_auto_remotes};
pub use pick::{pick_node, pick_node_with, pick_pinned_node};
pub use pin::{
    Endpoint, NodePolicy, PIN_ENV, PIN_RESUMED_ENV, PinMode, PinRequest, WORKSPACE_PIN_SOURCE,
    resolve_policy, resolve_policy_with, workspace_pin,
};
pub use pin_connect::{GatedConnection, confirm_gate, connect_gated, connect_gated_with};
pub use pin_scope::{
    PinInputs, node_policy, place_call, place_within, policy_for_root, policy_for_root_with,
    process_policy, set_pin_inputs, with_root_policy,
};
pub use pin_wire::confirm_pin;
pub use placement::{PlacementStore, forget_placement, remember_placement, remembered_node};
pub use rebalance::{evaluate_cluster_rebalance, evaluate_cluster_rebalance_with};
pub use routing::{
    PROBE_TIMEOUT, checkout_node_in, default_serves, engine_for_path, known_cluster_nodes,
    nested_engine, route_for_checkout, route_for_path, route_in, set_routing,
    set_routing_with_policy,
};
pub use selection::{
    choose_best_node, choose_quietest, is_alive, rendezvous_order, runs_os, supports_engine,
};
