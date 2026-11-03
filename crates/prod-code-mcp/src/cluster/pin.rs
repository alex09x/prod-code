/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which nodes a workspace's work may run on (#1793). Unpinned, the cluster places the workspace
//! and may move it. Gateways named with `--remote` on the command line are a closed set: work may
//! move between them but never to a node that was not listed. A strict pin (`--pin-node`,
//! `PROD_CODE_PIN_NODE` or `[cluster] pin` in `prod-code.toml`) holds the workspace to one logical
//! endpoint: the addresses its name resolves to are alternatives of one node, and anything that
//! would run elsewhere fails with the reason instead.

use std::net::{SocketAddr, ToSocketAddrs};

use anyhow::{Result, anyhow, bail};

/// The environment variable that pins a workspace, as `--pin-node` does.
pub const PIN_ENV: &str = "PROD_CODE_PIN_NODE";

/// Set for a re-executed MCP server whose startup checkout was pinned by its `prod-code.toml`:
/// the pin it started with, read in place of the file. Unlike [`PIN_ENV`] it governs only that
/// checkout, so other checkouts keep their own pins.
pub const PIN_RESUMED_ENV: &str = "PROD_CODE_PIN_RESUMED";

/// The source of a pin read from a checkout's `prod-code.toml`.
pub const WORKSPACE_PIN_SOURCE: &str = "prod-code.toml [cluster] pin";

/// How far a workspace's work may move.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PinMode {
    /// The cluster places the workspace and may move it (the default).
    #[default]
    Auto,
    /// Only the gateways listed on the command line.
    Listed,
    /// One logical endpoint and the addresses its name resolves to.
    Strict,
}

/// A gateway as configured: the name given and every address it resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub name: String,
    pub addrs: Vec<SocketAddr>,
}

/// The node policy of this process's workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodePolicy {
    pub mode: PinMode,
    pub endpoints: Vec<Endpoint>,
    /// Where the policy came from (`--pin-node`, `PROD_CODE_PIN_NODE`, `prod-code.toml`,
    /// `--remote`).
    pub source: String,
}

/// What the startup policy is made of.
#[derive(Debug, Clone, Copy, Default)]
pub struct PinRequest<'a> {
    /// `--pin-node` or `PROD_CODE_PIN_NODE`; `auto` (or `none`/`off`) overrides a workspace pin.
    pub pin: Option<&'a str>,
    /// How `pin` was given, for messages.
    pub pin_source: &'a str,
    /// `[cluster] pin` of the workspace's `prod-code.toml`.
    pub workspace_pin: Option<&'a str>,
    /// The `--remote` / `PROD_CODE_REMOTE` value; `--pin-node remote` pins to it.
    pub remote: &'a str,
    /// The addresses of `remote` when it was typed on the command line: a closed set.
    pub listed: Option<&'a [SocketAddr]>,
}

impl NodePolicy {
    pub fn is_pinned(&self) -> bool {
        self.mode != PinMode::Auto
    }

    /// The endpoint a strict pin holds the workspace to.
    pub fn strict_endpoint(&self) -> Option<&Endpoint> {
        match self.mode {
            PinMode::Strict => self.endpoints.first(),
            _ => None,
        }
    }

    /// Every address work may run on, in configured order; empty when unpinned.
    pub fn allowed(&self) -> Vec<SocketAddr> {
        let mut out = Vec::new();
        for addr in self.endpoints.iter().flat_map(|e| &e.addrs) {
            if !out.contains(addr) {
                out.push(*addr);
            }
        }
        out
    }

    /// Whether work may run on `addr`.
    pub fn allows(&self, addr: SocketAddr) -> bool {
        !self.is_pinned() || self.allowed().contains(&addr)
    }

    /// The addresses a call that could not reach `from` before it started may try instead:
    /// the other addresses of the pinned endpoint, or the other listed gateways. Unpinned, the
    /// caller searches the cluster itself, so none are named here.
    pub fn alternatives(&self, from: SocketAddr) -> Vec<SocketAddr> {
        self.allowed().into_iter().filter(|a| *a != from).collect()
    }

    /// `pinned to build.example:9400 (--pin-node)`, for messages.
    pub fn describe(&self) -> String {
        let names: Vec<&str> = self.endpoints.iter().map(|e| e.name.as_str()).collect();
        match self.mode {
            PinMode::Auto => "not pinned".to_string(),
            PinMode::Listed => format!("limited to {} ({})", names.join(", "), self.source),
            PinMode::Strict => format!("pinned to {} ({})", names.join(", "), self.source),
        }
    }

    /// `Ok` when work may run on `addr`; otherwise the reason it may not.
    pub fn check(&self, addr: SocketAddr, action: &str) -> Result<()> {
        if self.allows(addr) {
            return Ok(());
        }
        Err(anyhow!(
            "{action} would run on {addr}, but this workspace is {}; refusing to move work to \
             a node outside the pin",
            self.describe()
        ))
    }

    /// Whether a handshake may offer to follow gateway redirects.
    pub fn offers_redirects(&self) -> bool {
        !self.is_pinned()
    }

    /// A gateway redirect to `target`: allowed unpinned, or to an address the policy allows.
    pub fn check_redirect(&self, target: &str) -> Result<()> {
        if !self.is_pinned() {
            return Ok(());
        }
        let resolved = super::parse_remotes(target).unwrap_or_default();
        if resolved.iter().any(|addr| self.allows(*addr)) {
            return Ok(());
        }
        bail!(
            "the gateway asked to move this session to {target}, but this workspace is {}; \
             refusing to follow the redirect",
            self.describe()
        )
    }

    /// What a re-executed MCP server needs in its environment to hold the same strict pin,
    /// wherever the pin came from (a `prod-code.toml` edited meanwhile included).
    pub fn reexec_env(&self) -> Vec<(String, String)> {
        // A checkout's own pin stays that checkout's: as an explicit pin it would override the
        // pins of the other checkouts a call may select.
        let var = if self.source == WORKSPACE_PIN_SOURCE {
            PIN_RESUMED_ENV
        } else {
            PIN_ENV
        };
        self.strict_endpoint()
            .map(|e| vec![(var.to_string(), e.name.clone())])
            .unwrap_or_default()
    }
}

/// The node policy for `req`, resolving names with the system resolver.
pub fn resolve_policy(req: PinRequest<'_>) -> Result<NodePolicy> {
    resolve_policy_with(req, |item| {
        item.to_socket_addrs()
            .or_else(|_| (item, 9400).to_socket_addrs())
            .map(Iterator::collect)
    })
}

/// [`resolve_policy`] with the resolver given. A pin from the command line or the environment
/// wins over the workspace's; `auto` turns a workspace pin off.
pub fn resolve_policy_with(
    req: PinRequest<'_>,
    mut resolve: impl FnMut(&str) -> std::io::Result<Vec<SocketAddr>>,
) -> Result<NodePolicy> {
    // A blank `--pin-node` or `PROD_CODE_PIN_NODE` says nothing: it does not turn the
    // workspace's pin off; only `auto`, `off` or `none` does.
    let explicit = req.pin.map(str::trim).filter(|pin| !pin.is_empty());
    let (spec, source) = match explicit {
        Some(pin) => (Some(pin), req.pin_source.to_string()),
        None => (
            req.workspace_pin.map(str::trim),
            WORKSPACE_PIN_SOURCE.to_string(),
        ),
    };
    let spec = spec.filter(|s| !s.is_empty() && !is_off(s));
    let Some(spec) = spec else {
        return Ok(match req.listed {
            Some(listed) => NodePolicy {
                mode: PinMode::Listed,
                endpoints: vec![Endpoint {
                    name: req.remote.trim().to_string(),
                    addrs: listed.to_vec(),
                }],
                source: "--remote".to_string(),
            },
            None => NodePolicy::default(),
        });
    };
    let name = if spec.eq_ignore_ascii_case("remote") {
        req.remote.trim()
    } else {
        spec
    };
    if name.contains(',') {
        bail!(
            "{source} names `{name}`: a strict pin holds one endpoint; list several gateways with \
             --remote instead"
        );
    }
    if is_off(name) || prod_code_protocol::dns::is_code_internal_domain(name) {
        bail!("{source} names `{name}`, which the cluster resolves; pin a gateway's host:port");
    }
    let addrs = resolve(name)
        .map_err(|err| anyhow!("cannot resolve the pinned endpoint {name} ({source}): {err}"))?;
    let mut unique = Vec::new();
    for addr in addrs {
        if !unique.contains(&addr) {
            unique.push(addr);
        }
    }
    if unique.is_empty() {
        bail!("the pinned endpoint {name} ({source}) resolved to nothing");
    }
    if let Some(listed) = req.listed
        && !unique.iter().any(|addr| listed.contains(addr))
    {
        bail!(
            "--remote {} does not include {name}, the endpoint this workspace is pinned to \
             ({source}); name it in --remote or change the pin",
            req.remote.trim()
        );
    }
    Ok(NodePolicy {
        mode: PinMode::Strict,
        endpoints: vec![Endpoint {
            name: name.to_string(),
            addrs: unique,
        }],
        source,
    })
}

fn is_off(spec: &str) -> bool {
    ["auto", "none", "off"]
        .iter()
        .any(|word| spec.eq_ignore_ascii_case(word))
}

/// The `[cluster] pin` of `prod-code.toml` at `root`, read before any endpoint is discovered.
/// A file that does not parse fails closed when it has a `[cluster]` table: whether it pins a
/// node cannot be told, and running elsewhere is what the pin exists to prevent.
pub fn workspace_pin(root: &std::path::Path) -> Result<Option<String>> {
    let path = root.join("prod-code.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let table = match toml::from_str::<toml::Value>(&text) {
        Ok(table) => table,
        Err(err) if text.lines().any(|l| l.trim().starts_with("[cluster")) => {
            bail!(
                "{} has a [cluster] table but is not valid TOML, so its node pin cannot be read: {err}",
                path.display()
            );
        }
        Err(_) => return Ok(None),
    };
    match table.get("cluster").and_then(|cluster| cluster.get("pin")) {
        None => Ok(None),
        Some(toml::Value::String(pin)) => Ok(Some(pin.clone())),
        Some(other) => bail!(
            "{}: [cluster] pin must be a string such as \"host:9400\", not {other}",
            path.display()
        ),
    }
}

#[cfg(test)]
#[path = "pin_tests.rs"]
mod tests;
