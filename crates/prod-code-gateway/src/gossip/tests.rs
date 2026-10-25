/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use prod_code_protocol::dns::{
    DnsAnswer, DnsQueryType, DnsQuestion, DnsRecordData, encode_dns_name, format_dns_response,
};

fn peer(addr: &str, age: Duration) -> PeerEntry {
    let mut status = crate::tests::common::peer(addr, "linux", &["python (scripted)"], 0.25).status;
    status.memory_rss_bytes = Some(3 * 1024 * 1024);
    status.host.memory_available_bytes = Some(10 * 1024 * 1024);
    status.host.memory_total_bytes = Some(20 * 1024 * 1024);
    PeerEntry {
        gossip: NodeGossip {
            addr: addr.into(),
            status,
            workspaces: vec![LoadedWorkspaceInfo {
                name: "sample".into(),
                engine: "python".into(),
                sessions: 2,
            }],
            peers: vec![],
            sent_at_ms: 0,
        },
        last_seen: Instant::now() - age,
    }
}

#[test]
fn peer_announcements_preserve_routing_counters_engines_and_nonce() {
    let entry = peer("127.0.0.2:9400", Duration::ZERO);
    let expected = prod_code_protocol::discovery::format_node_line_with_nonce(
        "127.0.0.2:9400",
        "python",
        3,
        0.25,
        8,
        20,
        10,
        2,
        &[("sample".into(), "python".into(), 2)],
        Some("sample-token"),
        Some("sample-nonce"),
    );
    assert_eq!(
        build_peer_announce(&entry, Some("sample-token"), Some("sample-nonce")),
        expected
    );
    for (engine, expected) in [
        ("python", true),
        ("py", false),
        ("python (scripted)", true),
        ("", false),
        ("go", false),
    ] {
        assert_eq!(
            cluster_supports_engine(&entry.gossip.status, engine),
            expected
        );
    }
    assert_eq!(
        detect_advertise_addr("127.0.0.1:1234".parse().unwrap(), None),
        "127.0.0.1:1234"
    );
    assert_eq!(
        detect_advertise_addr("0.0.0.0:1234".parse().unwrap(), Some("127.0.0.1:1")),
        "127.0.0.1:1234"
    );
}

#[tokio::test]
async fn dns_returns_the_fresh_workspaces_peer_and_ignores_stale_or_invalid_peers() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = ServerState::new(temp.path().join("storage"));
    state.engine_allowlist = vec!["unavailable-test-engine".into()];
    *state.advertise.write().await = "127.0.0.1:9400".into();
    {
        let mut cluster = state.cluster.write().await;
        cluster.insert(
            "127.0.0.2:9400".into(),
            peer("127.0.0.2:9400", Duration::ZERO),
        );
        cluster.insert(
            "127.0.0.3:9400".into(),
            peer("127.0.0.3:9400", Duration::from_secs(60)),
        );
        cluster.insert("invalid".into(), peer("invalid", Duration::ZERO));
    }
    let question = DnsQuestion {
        name: "sample.code.internal".into(),
        qtype: DnsQueryType::A,
        qclass: 1,
    };
    let mut packet = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    packet.extend(encode_dns_name(&question.name));
    packet.extend([0, 1, 0, 1]);
    let expected = format_dns_response(
        0x1234,
        &question,
        &[DnsAnswer {
            name: question.name.clone(),
            ttl: 60,
            data: DnsRecordData::A("127.0.0.2".parse().unwrap()),
        }],
        true,
        0,
    );
    assert_eq!(
        handle_gateway_dns_query(&packet, &state).await,
        Some(expected)
    );
    assert_eq!(handle_gateway_dns_query(&[], &state).await, None);
    *state.advertise.write().await = "invalid".into();
    assert_eq!(handle_gateway_dns_query(&packet, &state).await, None);
}
