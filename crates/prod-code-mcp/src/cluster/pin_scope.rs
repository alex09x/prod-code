/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which node policy governs a piece of work (#1793). The process starts under the policy of
//! the checkout it was started in; an MCP call that selects another checkout of the repository
//! (`workspace`, or a path inside it) runs under that checkout's own `[cluster] pin`, unless a
//! pin given on the command line or in the environment overrides every checkout. The selected
//! policy is scoped to the call's task, never stored for the process, so concurrent calls on
//! different checkouts each keep their own.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Result, anyhow};

use super::pin::{NodePolicy, PinRequest, WORKSPACE_PIN_SOURCE, resolve_policy, workspace_pin};

/// What the startup policy was made of, so another checkout's policy can be made the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinInputs {
    /// `--pin-node` or `PROD_CODE_PIN_NODE`, when given and not blank: it governs every checkout.
    pub pin: Option<String>,
    /// The `--remote` / `PROD_CODE_REMOTE` value.
    pub remote: String,
    /// The addresses of `remote` when it was typed on the command line.
    pub listed: Option<Vec<SocketAddr>>,
    /// The checkout the process started in, canonical.
    pub root: Option<PathBuf>,
}

static INPUTS: OnceLock<PinInputs> = OnceLock::new();

/// Records how the startup policy was made; set once, before any call runs.
pub fn set_pin_inputs(inputs: PinInputs) {
    let _ = INPUTS.set(inputs);
}

#[derive(Clone)]
struct Scoped {
    root: PathBuf,
    policy: NodePolicy,
}

tokio::task_local! {
    static SCOPED: Scoped;
}

fn canonical(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// The policy of the process: the checkout it started in.
pub fn process_policy() -> NodePolicy {
    super::routing::ROUTING
        .get()
        .map(|routing| routing.policy.clone())
        .unwrap_or_default()
}

/// The node policy of the current work: the call's checkout inside an MCP call, the process's
/// otherwise.
pub fn node_policy() -> NodePolicy {
    SCOPED
        .try_with(|scoped| scoped.policy.clone())
        .unwrap_or_else(|_| process_policy())
}

/// Runs `work` under `policy`, the policy of the checkout at `root`.
pub async fn with_root_policy<F: Future>(root: &Path, policy: NodePolicy, work: F) -> F::Output {
    SCOPED
        .scope(
            Scoped {
                root: canonical(root),
                policy,
            },
            work,
        )
        .await
}

/// The policy work on the checkout at `root` runs under.
pub fn policy_for_root(root: &Path) -> Result<NodePolicy> {
    effective_policy(root, &process_policy())
}

/// [`policy_for_root`], with `fallback` as the policy of the checkout the process started in.
pub(crate) fn effective_policy(root: &Path, fallback: &NodePolicy) -> Result<NodePolicy> {
    let root = canonical(root);
    if let Ok(Some(policy)) =
        SCOPED.try_with(|scoped| (scoped.root == root).then(|| scoped.policy.clone()))
    {
        return Ok(policy);
    }
    match INPUTS.get() {
        Some(inputs) => policy_for_root_with(&root, inputs, fallback),
        None => policy_for_root_with(&root, &PinInputs::default(), fallback),
    }
}

/// The policy of the checkout at `root` given how the startup policy was made. The startup
/// checkout and an explicit pin keep `startup`. Another checkout with a `[cluster] pin` of its
/// own uses it, within the same `--remote` set (`auto` there turns it off for that checkout);
/// one without keeps `startup`, so selecting it never loosens the pin the process runs under.
pub fn policy_for_root_with(
    root: &Path,
    inputs: &PinInputs,
    startup: &NodePolicy,
) -> Result<NodePolicy> {
    let root = canonical(root);
    if inputs.pin.is_some() || inputs.root.as_deref() == Some(root.as_path()) {
        return Ok(startup.clone());
    }
    let Some(pin) = workspace_pin(&root)? else {
        return Ok(startup.clone());
    };
    let key = format!("{pin}\0{}\0{:?}", inputs.remote, inputs.listed);
    static RESOLVED: Mutex<Option<HashMap<String, NodePolicy>>> = Mutex::new(None);
    if let Some(policy) = RESOLVED
        .lock()
        .ok()
        .and_then(|cache| cache.as_ref()?.get(&key).cloned())
    {
        return Ok(policy);
    }
    let policy = resolve_policy(PinRequest {
        pin: None,
        pin_source: WORKSPACE_PIN_SOURCE,
        workspace_pin: Some(&pin),
        remote: &inputs.remote,
        listed: inputs.listed.as_deref(),
    })
    .map_err(|err| err.context(format!("cannot apply the node pin of {}", root.display())))?;
    if let Ok(mut cache) = RESOLVED.lock() {
        cache
            .get_or_insert_with(HashMap::new)
            .insert(key, policy.clone());
    }
    Ok(policy)
}

/// The policy of a call on the checkout at `root`, and the gateway it starts on: `remote` when
/// the policy allows it, otherwise the first address the checkout is pinned to.
pub fn place_call(root: &Path, remote: SocketAddr) -> Result<(NodePolicy, SocketAddr)> {
    let policy = policy_for_root(root)?;
    let remote = place_within(&policy, remote)?;
    Ok((policy, remote))
}

/// `remote` when `policy` allows it, otherwise the first address it allows.
pub fn place_within(policy: &NodePolicy, remote: SocketAddr) -> Result<SocketAddr> {
    if policy.allows(remote) {
        return Ok(remote);
    }
    policy.allowed().first().copied().ok_or_else(|| {
        anyhow!(
            "this checkout is {}, which allows no address",
            policy.describe()
        )
    })
}

#[cfg(test)]
#[path = "pin_scope_tests.rs"]
mod tests;
