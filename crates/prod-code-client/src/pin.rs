/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The node policy the CLI starts under (#1793), resolved from `--pin-node` or
//! `PROD_CODE_PIN_NODE`, the workspace's `prod-code.toml`, and whether `--remote` was typed on
//! the command line, before any endpoint is discovered.

use anyhow::{Context, Result};
use prod_code_mcp::cluster::{NodePolicy, PIN_ENV, PIN_RESUMED_ENV, PinInputs, PinRequest};
use std::path::Path;

/// The policy for this invocation. A `--remote` typed on the command line is a closed set; one
/// from the environment alone stays a seed for the cluster, unless a pin asks for more. How it
/// was made is recorded, so a call that selects another checkout applies that checkout's own pin
/// the same way.
pub fn startup_policy(
    cli: &crate::cli::Cli,
    remote_typed: bool,
    root: Option<&Path>,
) -> Result<NodePolicy> {
    // A blank `--pin-node` or `PROD_CODE_PIN_NODE` is no pin at all: it neither pins nor turns
    // the workspace's pin off.
    let explicit = cli
        .pin_node
        .as_deref()
        .map(str::trim)
        .filter(|pin| !pin.is_empty());
    // A re-executed server keeps the pin its checkout started with (#1793).
    let resumed = std::env::var(PIN_RESUMED_ENV)
        .ok()
        .filter(|pin| !pin.trim().is_empty());
    let workspace_pin = match (resumed, root) {
        (Some(pin), _) => Some(pin),
        (None, Some(root)) => prod_code_mcp::cluster::workspace_pin(root)?,
        (None, None) => None,
    };
    let listed = if remote_typed {
        Some(prod_code_mcp::cluster::parse_remotes(&cli.remote)?)
    } else {
        None
    };
    let from_env =
        explicit.is_some() && std::env::var(PIN_ENV).ok().as_deref().map(str::trim) == explicit;
    let policy = prod_code_mcp::cluster::resolve_policy(PinRequest {
        pin: explicit,
        pin_source: if from_env { PIN_ENV } else { "--pin-node" },
        workspace_pin: workspace_pin.as_deref(),
        remote: &cli.remote,
        listed: listed.as_deref(),
    })
    .context(
        "cannot apply the workspace's node pin; fix it, or run unpinned with `--pin-node auto`",
    )?;
    prod_code_mcp::cluster::set_pin_inputs(PinInputs {
        pin: explicit.map(str::to_string),
        remote: cli.remote.clone(),
        listed,
        root: root.map(|root| std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())),
    });
    Ok(policy)
}
