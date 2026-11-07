/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Forgetting a peer address by hand (#1973): a node that came back under another address stays
//! listed under the old one as offline, because the nodes hand the address back to one another
//! even after each has dropped it. The tombstone here stops that, and the request is passed on to
//! every other node so that no one is left to hand it back.

use crate::*;
use prod_code_protocol::{ForgetPeerRequest, ForgetPeerResponse};

/// How long other nodes' gossip is not believed about a forgotten address.
const FORGET_FOR: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

impl ServerState {
    /// Drops `addr` from the listing and from the peers this node gossips with, and refuses to
    /// learn it from other nodes' gossip from now on. Whether the address was known.
    pub async fn forget_peer(&self, addr: &str) -> bool {
        let mut forgotten = self.forgotten.write().await;
        let now = Instant::now();
        // An entry is the moment it stops holding: no clock is read backwards from it.
        forgotten.retain(|_, until| *until > now);
        forgotten.insert(addr.to_string(), now + FORGET_FOR);
        let in_cluster = self.cluster.write().await.remove(addr).is_some();
        let in_peers = self.peers.write().await.remove(addr);
        in_cluster || in_peers
    }

    /// The nodes to pass a forget request on to: every other address this node knows.
    pub async fn forget_targets(&self, addr: &str) -> Vec<String> {
        let me = self.advertise.read().await.clone();
        let mut targets: std::collections::BTreeSet<String> =
            self.peers.read().await.iter().cloned().collect();
        targets.extend(self.cluster.read().await.keys().cloned());
        targets.remove(addr);
        targets.remove(&me);
        targets.into_iter().collect()
    }

    /// Forgets `req.addr` here and, when asked to, tells every other known node to.
    pub async fn handle_forget(&self, req: &ForgetPeerRequest) -> ForgetPeerResponse {
        // The nodes to tell are read before the address goes, so that a node known only through
        // the forgotten one is still told.
        let targets = if req.propagate {
            self.forget_targets(&req.addr).await
        } else {
            Vec::new()
        };
        let forgotten = self.forget_peer(&req.addr).await;
        let mut response = ForgetPeerResponse {
            forgotten,
            ..Default::default()
        };
        let passed_on = ForgetPeerRequest {
            addr: req.addr.clone(),
            propagate: false,
        };
        // All at once, each within its own 3 s: up to 64 peers must not add up to more than the
        // client waits, which would make it ask again.
        let told = futures_util::future::join_all(
            targets
                .iter()
                .map(|target| self.tell_forget(target, &passed_on)),
        )
        .await;
        for (target, told) in targets.into_iter().zip(told) {
            if told {
                response.forwarded.push(target);
            } else {
                response.failed.push(target);
            }
        }
        response
    }

    async fn tell_forget(&self, target: &str, req: &ForgetPeerRequest) -> bool {
        let Ok(addr) = target.parse::<SocketAddr>() else {
            return false;
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let stream = prod_code_protocol::transport::connect_stream_with_client_config(
                addr,
                self.auth_token.as_deref(),
                self.client_tls.clone(),
            )
            .await
            .ok()?;
            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            framed
                .send(WireMessage::ForgetPeerRequest(req.clone()))
                .await
                .ok()?;
            match framed.next().await {
                Some(Ok(WireMessage::ForgetPeerResponse(_))) => Some(()),
                _ => None,
            }
        })
        .await
        .ok()
        .flatten()
        .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gossip(addr: &str, peers: &[&str]) -> NodeGossip {
        let mut gossip = NodeGossip {
            addr: addr.into(),
            status: crate::tests::common::peer(addr, "linux", &[], 0.1).status,
            workspaces: vec![],
            peers: peers.iter().map(|p| p.to_string()).collect(),
            sent_at_ms: 0,
        };
        gossip.addr = addr.into();
        gossip
    }

    fn state() -> (tempfile::TempDir, ServerState) {
        let temp = tempfile::tempdir().unwrap();
        let state = ServerState::new(temp.path().join("storage"));
        (temp, state)
    }

    /// The address survives each node dropping it, because they hand it back to one another; once
    /// forgotten, a third node naming it does not bring it back, and the peer itself speaking does.
    #[tokio::test]
    async fn a_forgotten_address_is_not_brought_back_by_other_nodes_gossip() {
        let (_temp, state) = state();
        *state.advertise.write().await = "10.0.0.1:9400".into();
        state
            .absorb_gossip(gossip("10.0.0.2:9400", &["10.0.0.9:9400"]))
            .await;
        assert!(state.peers.read().await.contains("10.0.0.9:9400"));

        assert!(state.forget_peer("10.0.0.9:9400").await, "it was known");
        assert!(!state.peers.read().await.contains("10.0.0.9:9400"));
        assert!(!state.forget_peer("10.0.0.7:9400").await, "never heard of");

        state
            .absorb_gossip(gossip("10.0.0.2:9400", &["10.0.0.9:9400"]))
            .await;
        assert!(
            !state.peers.read().await.contains("10.0.0.9:9400"),
            "a third node naming it does not bring it back"
        );

        state.absorb_gossip(gossip("10.0.0.9:9400", &[])).await;
        assert!(state.peers.read().await.contains("10.0.0.9:9400"));
        assert!(state.cluster.read().await.contains_key("10.0.0.9:9400"));
        assert!(state.forgotten.read().await.get("10.0.0.9:9400").is_none());
    }

    /// A tombstone holds for a time, not for ever: once it ran out other nodes' gossip is believed
    /// again, without another forget being needed to sweep it away.
    #[tokio::test]
    async fn a_tombstone_runs_out() {
        let (_temp, state) = state();
        *state.advertise.write().await = "10.0.0.1:9400".into();
        state
            .forgotten
            .write()
            .await
            .insert("10.0.0.9:9400".into(), Instant::now());
        state
            .absorb_gossip(gossip("10.0.0.2:9400", &["10.0.0.9:9400"]))
            .await;
        assert!(state.peers.read().await.contains("10.0.0.9:9400"));
        assert!(
            state.forgotten.read().await.is_empty(),
            "the old entry is gone"
        );
    }

    /// Unreachable targets are asked together: ten of them take one timeout, not ten.
    #[tokio::test]
    async fn the_nodes_are_told_together_not_one_after_another() {
        let (_temp, state) = state();
        // Listeners that accept and never answer, so each hop waits its whole 3 s.
        let mut keep = Vec::new();
        for _ in 0..5 {
            let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            state
                .peers
                .write()
                .await
                .insert(silent.local_addr().unwrap().to_string());
            keep.push(tokio::spawn(async move {
                let mut held = Vec::new();
                while let Ok((socket, _)) = silent.accept().await {
                    held.push(socket);
                }
            }));
        }
        let started = std::time::Instant::now();
        let response = state
            .handle_forget(&ForgetPeerRequest {
                addr: "127.0.0.1:9".into(),
                propagate: true,
            })
            .await;
        keep.iter().for_each(|task| task.abort());
        assert_eq!(response.failed.len(), 5);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(6),
            "five silent nodes took {:?}: they were asked one after another",
            started.elapsed()
        );
    }

    /// A request that is passed on goes to every other known node, and says which did not answer.
    #[tokio::test]
    async fn a_forget_request_is_passed_on_to_the_other_known_nodes() {
        let (_temp, state) = state();
        *state.advertise.write().await = "127.0.0.1:1".into();
        state
            .absorb_gossip(gossip("127.0.0.1:2", &["127.0.0.1:3"]))
            .await;
        let targets = state.forget_targets("127.0.0.1:3").await;
        assert_eq!(
            targets,
            ["127.0.0.1:2"],
            "not itself, not the forgotten one"
        );

        let response = state
            .handle_forget(&ForgetPeerRequest {
                addr: "127.0.0.1:3".into(),
                propagate: true,
            })
            .await;
        assert!(response.forgotten);
        // Nothing listens on port 2 here: it is reported, not dropped silently.
        assert_eq!(response.failed, ["127.0.0.1:2"]);
        assert!(response.forwarded.is_empty());

        let local_only = state
            .handle_forget(&ForgetPeerRequest {
                addr: "127.0.0.1:2".into(),
                propagate: false,
            })
            .await;
        assert!(local_only.forwarded.is_empty() && local_only.failed.is_empty());
    }
}
