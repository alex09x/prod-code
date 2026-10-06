/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

use super::discover::node_status;
use super::pick::pick_node_with;
use super::placement::placement_path;
use super::selection::supports_engine;

/// How long a node has to accept a TCP connection before it counts as down.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// The cluster a process may route a request to by the path it names, and the workspace name
/// placements are remembered under. Set once at startup; unset in tests and in library use,
/// where every request goes to the node it was given.
pub(crate) struct Routing {
    pub(crate) nodes: Vec<SocketAddr>,
    pub(crate) workspace: String,
}

pub(crate) static ROUTING: std::sync::OnceLock<Routing> = std::sync::OnceLock::new();

/// Lets requests that name a path be sent to a node that serves that path's engine (#125).
pub fn set_routing(nodes: Vec<SocketAddr>, workspace: String) {
    let _ = ROUTING.set(Routing { nodes, workspace });
}

/// The node for a request about `path` in the checkout at `root`: `default` when the path
/// belongs to the checkout's own project, otherwise a node that serves the engine of the
/// nested project the path is in (a SwiftPM package or Xcode project under a Rust repository
/// needs the macOS node). That placement is remembered under its own key, `<workspace>#swift`,
/// so it does not displace the checkout's. Without routing set, or with one node, `default`.
pub async fn route_for_path(
    default: SocketAddr,
    root: &Path,
    path: Option<&str>,
) -> Result<SocketAddr> {
    let (Some(routing), Some(path)) = (ROUTING.get(), path) else {
        return Ok(default);
    };
    route_in(
        &routing.nodes,
        &routing.workspace,
        default,
        root,
        path,
        placement_path().as_deref(),
    )
    .await
}

/// The node for a query about another whole checkout at `root`: the one its own workspace is
/// placed on, picked as the CLI picks it when started there, so that checkout's warm analyzer
/// answers instead of a cold copy on this checkout's node (#375). Without routing set, or for
/// this checkout itself, `default`.
pub async fn route_for_checkout(default: SocketAddr, root: &Path) -> Result<SocketAddr> {
    let Some(routing) = ROUTING.get() else {
        return Ok(default);
    };
    checkout_node_in(
        &routing.nodes,
        &routing.workspace,
        default,
        root,
        placement_path().as_deref(),
    )
    .await
}

/// [`route_for_checkout`] over an explicit cluster, remembering placements in `placement_file`.
pub async fn checkout_node_in(
    nodes: &[SocketAddr],
    workspace: &str,
    default: SocketAddr,
    root: &Path,
    placement_file: Option<&Path>,
) -> Result<SocketAddr> {
    let identity = crate::sync::workspace_identity(root);
    let name = identity.base.unwrap_or(identity.name);
    if name == workspace {
        return Ok(default);
    }
    let (_, engine) = crate::sync::engine_project(root, root);
    let os = crate::sync::macos_only_cgo(root).map(|_| "macos");
    pick_node_with(nodes, &name, engine, os, placement_file).await
}

/// [`route_for_path`] over an explicit cluster, remembering placements in `placement_file`.
pub async fn route_in(
    nodes: &[SocketAddr],
    workspace: &str,
    default: SocketAddr,
    root: &Path,
    path: &str,
    placement_file: Option<&Path>,
) -> Result<SocketAddr> {
    let Some(engine) = nested_engine(root, path) else {
        return Ok(default);
    };
    if default_serves(default, engine).await {
        return Ok(default);
    }
    let key = format!("{workspace}#{engine}");
    pick_node_with(nodes, &key, Some(engine), None, placement_file)
        .await
        .with_context(|| format!("`{path}` is in a {engine} project"))
}

/// The engine of the nested project `path` is in, when that is not the checkout's own project:
/// `swift` for `swift/Sources/App/main.swift` under a Rust root. `None` for a path of the root
/// project.
pub fn nested_engine(root: &Path, path: &str) -> Option<&'static str> {
    let p = Path::new(path);
    let hint = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    };
    match crate::sync::engine_project(root, &hint) {
        (Some(_), Some(engine)) => Some(engine),
        _ => None,
    }
}

/// Whether `node` lists `engine`.
async fn default_serves(node: SocketAddr, engine: &str) -> bool {
    node_status(node)
        .await
        .is_ok_and(|status| supports_engine(&status, engine))
}
