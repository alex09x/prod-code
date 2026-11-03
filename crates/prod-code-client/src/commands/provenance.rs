/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Where a verification ran (#1793): the candidates a check, lint, test or benchmark may move
//! between under the workspace's node policy, and the node it actually ran on, with every move,
//! for the text summary, the JSON report and the event stream alike.

use prod_code_mcp::cluster::{NodePolicy, PinMode};
use std::net::SocketAddr;

/// A move from one node to another, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    pub from: SocketAddr,
    pub to: SocketAddr,
    pub reason: String,
}

/// The node a run was asked of and the one it ran on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    pub requested: SocketAddr,
    pub node: SocketAddr,
    pub relocations: Vec<Relocation>,
    /// The node the gateway confirmed on the run's own connection, under a strict pin.
    pub confirmed_node: Option<String>,
}

impl Provenance {
    pub fn new(requested: SocketAddr) -> Self {
        Self {
            requested,
            node: requested,
            relocations: Vec::new(),
            confirmed_node: None,
        }
    }

    /// This provenance with what the report says of where it ran: the node its executor names
    /// and the node the gateway confirmed there.
    pub fn for_report(&self, report: &prod_code_mcp::verify::VerifyReport) -> Self {
        let mut shown = self.clone();
        if let Some(executor) = &report.executor {
            if let Ok(node) = executor.node.parse() {
                shown.node = node;
            }
            shown.confirmed_node = executor.confirmed_node.clone();
        }
        shown
    }

    /// Records a move to `to`, which is where the run now goes.
    pub fn relocate(&mut self, to: SocketAddr, reason: impl Into<String>) -> &Relocation {
        let from = self.node;
        self.node = to;
        self.relocations.push(Relocation {
            from,
            to,
            reason: reason.into(),
        });
        self.relocations.last().expect("just pushed")
    }

    /// The `execution` object of the JSON report and the report event.
    pub fn json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "node": self.node.to_string(),
            "requested": self.requested.to_string(),
            "relocations": self.relocations.iter().map(relocation_json).collect::<Vec<_>>(),
        });
        if let Some(gateway) = &self.confirmed_node {
            value["gateway_node"] = serde_json::Value::String(gateway.clone());
        }
        value
    }

    /// `ran on node A (relocated from B: why)`, for the text summary.
    pub fn summary(&self) -> String {
        let mut text = format!("ran on node {}", self.node);
        if let Some(gateway) = &self.confirmed_node {
            text.push_str(&format!(" (gateway confirms {gateway})"));
        }
        for moved in &self.relocations {
            text.push_str(&format!(
                "; relocated from {} to {}: {}",
                moved.from, moved.to, moved.reason
            ));
        }
        text
    }
}

/// The `relocated` event and the entries of `execution.relocations`.
pub fn relocation_json(moved: &Relocation) -> serde_json::Value {
    serde_json::json!({
        "from": moved.from.to_string(),
        "to": moved.to.to_string(),
        "reason": moved.reason,
    })
}

/// The report as JSON, with where it ran.
pub fn report_json<T: serde::Serialize>(
    report: &T,
    provenance: &Provenance,
) -> serde_json::Result<serde_json::Value> {
    let mut value = serde_json::to_value(report)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("execution".to_string(), provenance.json());
    }
    Ok(value)
}

/// Prints `report` as `--events`, `--json` or text, naming the node it ran on and every move
/// before it.
pub fn emit_report(
    report: &prod_code_mcp::verify::VerifyReport,
    provenance: &Provenance,
    events: bool,
    json: bool,
    kind: prod_code_mcp::verify::VerifyKind,
) -> anyhow::Result<()> {
    let provenance = &provenance.for_report(report);
    if events {
        let value = serde_json::json!({
            "event": "report",
            "report": report,
            "execution": provenance.json(),
        });
        println!("{value}");
    } else if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json(report, provenance)?)?
        );
    } else {
        print!("{}", report.render(200));
        eprintln!("[prod-code {}] {}", kind.label(), provenance.summary());
    }
    Ok(())
}

/// The nodes a verification may run on, `remote` first. Unpinned, the other known gateways
/// follow; a listed set adds only its own members and a strict pin only the other addresses
/// of its endpoint (#1793).
pub fn verify_candidates(
    remote: SocketAddr,
    remotes: &[SocketAddr],
    policy: &NodePolicy,
    known: impl FnOnce() -> Option<Vec<SocketAddr>>,
) -> Vec<SocketAddr> {
    let mut candidates = vec![remote];
    let add = |addr: SocketAddr, candidates: &mut Vec<SocketAddr>| {
        if !candidates.contains(&addr) && policy.allows(addr) {
            candidates.push(addr);
        }
    };
    match policy.mode {
        PinMode::Strict => {
            for addr in policy.alternatives(remote) {
                add(addr, &mut candidates);
            }
        }
        PinMode::Listed | PinMode::Auto => {
            for &addr in remotes {
                add(addr, &mut candidates);
            }
            if policy.mode == PinMode::Auto && candidates.len() <= 1 {
                for addr in known().unwrap_or_default() {
                    add(addr, &mut candidates);
                }
            }
        }
    }
    candidates
}

/// Whether a failed verification may move on to the next candidate: under a pin only when
/// it never started there.
pub fn may_retry(policy: &NodePolicy, e: &anyhow::Error) -> bool {
    prod_code_mcp::server::retry_permitted(policy, "verify", e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prod_code_mcp::cluster::Endpoint;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn policy(mode: PinMode, addrs: &[SocketAddr]) -> NodePolicy {
        NodePolicy {
            mode,
            endpoints: vec![Endpoint {
                name: "pinned".into(),
                addrs: addrs.to_vec(),
            }],
            source: "--pin-node".into(),
        }
    }

    #[test]
    fn issue_1793_verification_candidates_follow_the_policy() {
        let known = || Some(vec![addr(9), addr(8)]);
        let auto = NodePolicy::default();
        assert_eq!(
            verify_candidates(addr(1), &[addr(1)], &auto, known),
            [addr(1), addr(9), addr(8)]
        );
        let listed = policy(PinMode::Listed, &[addr(1), addr(2)]);
        assert_eq!(
            verify_candidates(addr(1), &[addr(1), addr(2)], &listed, known),
            [addr(1), addr(2)]
        );
        let strict = policy(PinMode::Strict, &[addr(1), addr(3)]);
        assert_eq!(
            verify_candidates(addr(1), &[addr(1), addr(2)], &strict, known),
            [addr(1), addr(3)]
        );
    }

    #[test]
    fn issue_1793_provenance_names_the_executing_node_and_each_move() {
        let mut provenance = Provenance::new(addr(1));
        provenance.relocate(addr(3), "connection refused");
        let json = provenance.json();
        assert_eq!(json["node"], addr(3).to_string());
        assert_eq!(json["requested"], addr(1).to_string());
        assert_eq!(json["relocations"][0]["from"], addr(1).to_string());
        assert!(provenance.summary().starts_with(&format!(
            "ran on node {}; relocated from {} to {}",
            addr(3),
            addr(1),
            addr(3)
        )));
    }
}
