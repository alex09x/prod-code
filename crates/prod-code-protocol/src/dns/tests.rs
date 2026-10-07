/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::smart::{
    extract_project_name, generate_srv_records, handle_dns_packet, is_code_internal_domain,
    is_srv_service_domain, resolve_project_node, resolve_smart_domain,
};
use super::types::{DnsAnswer, DnsQueryType, DnsQuestion, DnsRecordData, DnsSrvRecord};
use super::wire::{format_dns_response, parse_dns_query};
use crate::discovery::{DiscoveredNode, LoadedWorkspace};
use crate::messages::content_hash;
use std::net::Ipv4Addr;

fn mock_node(addr: &str, workspaces: &[(&str, &str, u32)]) -> DiscoveredNode {
    DiscoveredNode {
        addr: addr.parse().unwrap(),
        engines: vec!["rust".into(), "go".into()],
        rss_mb: 1024,
        load_per_cpu: 0.25,
        cpus: 16,
        mem_total_mb: 64000,
        mem_avail_mb: 32000,
        sessions: 2,
        workspaces: workspaces
            .iter()
            .map(|(n, e, s)| LoadedWorkspace {
                name: (*n).into(),
                engine: (*e).into(),
                sessions: *s,
            })
            .collect(),
        nonce: None,
    }
}

#[test]
fn test_is_code_internal_domain() {
    assert!(is_code_internal_domain("code.internal"));
    assert!(is_code_internal_domain("shop.code.internal"));
    assert!(is_code_internal_domain("billing.code.internal:9400"));
    assert!(is_code_internal_domain("_prod-code._tcp.code.internal"));
    assert!(!is_code_internal_domain("github.com"));
    assert!(!is_code_internal_domain("localhost:9400"));
}

#[test]
fn test_extract_project_name() {
    assert_eq!(
        extract_project_name("shop.code.internal").as_deref(),
        Some("shop")
    );
    assert_eq!(
        extract_project_name("SHOP.CODE.INTERNAL").as_deref(),
        Some("shop")
    );
    assert_eq!(
        extract_project_name("Shop.Code.Internal").as_deref(),
        Some("shop")
    );
    assert_eq!(
        extract_project_name("billing.code.internal:9400").as_deref(),
        Some("billing")
    );
    assert_eq!(
        extract_project_name("cluster.code.internal").as_deref(),
        Some("cluster")
    );
    assert_eq!(
        extract_project_name("code.internal").as_deref(),
        Some("cluster")
    );
    assert_eq!(extract_project_name("example.com"), None);
}

#[test]
fn test_internal_domain_case_insensitivity_warm_placement() {
    let node1 = mock_node("192.0.2.10:9400", &[("shop", "rust", 1)]);
    let node2 = mock_node("192.0.2.20:9400", &[("billing", "go", 2)]);
    let nodes = vec![node1, node2];

    // Lowercase, uppercase, and mixed-case must all resolve to the exact same warm workspace node
    let lower = resolve_smart_domain("shop.code.internal", &nodes, 9400).expect("lower");
    let upper = resolve_smart_domain("SHOP.CODE.INTERNAL", &nodes, 9400).expect("upper");
    let mixed = resolve_smart_domain("ShOp.CoDe.InTeRnAl:9400", &nodes, 9400).expect("mixed");

    assert_eq!(lower, vec!["192.0.2.10:9400".parse().unwrap()]);
    assert_eq!(upper, lower);
    assert_eq!(mixed, lower);
}

#[test]
fn test_resolve_project_node_warm_preference() {
    let node1 = mock_node("192.0.2.10:9400", &[("shop", "rust", 1)]);
    let node2 = mock_node("192.0.2.20:9400", &[("billing", "go", 2)]);
    let nodes = vec![node1, node2];

    let resolved_shop = resolve_project_node("shop", &nodes).expect("shop node");
    assert_eq!(resolved_shop.addr, "192.0.2.10:9400".parse().unwrap());

    let resolved_billing = resolve_project_node("billing", &nodes).expect("billing node");
    assert_eq!(resolved_billing.addr, "192.0.2.20:9400".parse().unwrap());

    // Unloaded project resolves deterministically via rendezvous hashing
    let resolved_other = resolve_project_node("analytics", &nodes).expect("analytics node");
    let rendezvous = nodes
        .iter()
        .max_by_key(|node| content_hash(format!("analytics|{}", node.addr).as_bytes()))
        .unwrap();
    assert_eq!(resolved_other.addr, rendezvous.addr);

    let unrelated_prefix = mock_node("192.0.2.10:9400", &[("shopping-cart", "rust", 1)]);
    let worktree = mock_node("192.0.2.20:9400", &[("shop--wt-a1b2", "rust", 1)]);
    let nodes = vec![unrelated_prefix, worktree.clone()];
    assert_eq!(
        resolve_project_node("shop", &nodes).unwrap().addr,
        worktree.addr
    );
}

#[test]
fn test_resolve_smart_domain_mapping() {
    let node1 = mock_node("192.0.2.10:9400", &[("shop", "rust", 1)]);
    let node2 = mock_node("192.0.2.20:9400", &[("billing", "go", 2)]);
    let nodes = vec![node1, node2];

    // Direct project domain
    let addrs = resolve_smart_domain("shop.code.internal", &nodes, 9400).expect("resolved");
    assert_eq!(addrs, vec!["192.0.2.10:9400".parse().unwrap()]);

    // Explicit port override
    let addrs_custom_port =
        resolve_smart_domain("shop.code.internal:9443", &nodes, 9400).expect("resolved");
    assert_eq!(addrs_custom_port, vec!["192.0.2.10:9443".parse().unwrap()]);

    // Cluster domain returns all nodes
    let cluster_addrs =
        resolve_smart_domain("cluster.code.internal", &nodes, 9400).expect("cluster");
    assert_eq!(cluster_addrs.len(), 2);
}

#[test]
fn test_dns_wire_encode_decode_round_trip() {
    let question = DnsQuestion {
        name: "shop.code.internal".to_string(),
        qtype: DnsQueryType::A,
        qclass: 1,
    };
    let answers = vec![DnsAnswer {
        name: "shop.code.internal".to_string(),
        ttl: 60,
        data: DnsRecordData::A(Ipv4Addr::new(192, 168, 2, 10)),
    }];

    let packet = format_dns_response(1234, &question, &answers, true, 0);
    assert!(!packet.is_empty());

    let (parsed_id, parsed_q) = parse_dns_query(&packet).expect("parse query");
    assert_eq!(parsed_id, 1234);
    assert_eq!(parsed_q.name, "shop.code.internal");
    assert_eq!(parsed_q.qtype, DnsQueryType::A);
}

#[test]
fn test_handle_dns_packet_srv_and_a() {
    let node = mock_node("192.0.2.15:9400", &[("api-gateway", "rust", 1)]);
    let nodes = vec![node];

    // A query
    let query_a = format_dns_response(
        555,
        &DnsQuestion {
            name: "api-gateway.code.internal".into(),
            qtype: DnsQueryType::A,
            qclass: 1,
        },
        &[],
        false,
        0,
    );

    let resp_a = handle_dns_packet(&query_a, &nodes).expect("resp A");
    assert!(!resp_a.is_empty());

    // SRV query
    let query_srv = format_dns_response(
        666,
        &DnsQuestion {
            name: "_prod-code._tcp.code.internal".into(),
            qtype: DnsQueryType::SRV,
            qclass: 1,
        },
        &[],
        false,
        0,
    );

    let resp_srv = handle_dns_packet(&query_srv, &nodes).expect("resp SRV");
    assert!(!resp_srv.is_empty());
}

#[test]
fn test_unsupported_qtype_returns_noerror_nodata() {
    let node = mock_node("192.0.2.15:9400", &[("api-gateway", "rust", 1)]);
    let nodes = vec![node];

    // Query AAAA (type 28) for existing internal name
    let query_aaaa = format_dns_response(
        777,
        &DnsQuestion {
            name: "api-gateway.code.internal".into(),
            qtype: DnsQueryType::Other(28),
            qclass: 1,
        },
        &[],
        false,
        0,
    );

    let resp = handle_dns_packet(&query_aaaa, &nodes).expect("response");
    assert!(resp.len() >= 12);
    let flags = u16::from_be_bytes([resp[2], resp[3]]);
    let rcode = flags & 0x000F;
    let ancount = u16::from_be_bytes([resp[6], resp[7]]);

    // Standards-compliant: NOERROR (RCODE = 0) with ANCOUNT = 0 (NODATA)
    assert_eq!(
        rcode, 0,
        "must return NOERROR for existing name with unsupported qtype"
    );
    assert_eq!(ancount, 0, "must return 0 answers for unsupported qtype");
}

#[test]
fn test_nonexistent_domain_returns_nxdomain() {
    // Query when nodes list is empty and name is unrecognized
    let query = format_dns_response(
        888,
        &DnsQuestion {
            name: "nonexistent.example.internal".into(),
            qtype: DnsQueryType::A,
            qclass: 1,
        },
        &[],
        false,
        0,
    );

    // Not in .code.internal virtual domain -> ignored (None)
    assert!(handle_dns_packet(&query, &[]).is_none());
}

#[test]
fn test_generate_srv_records_weights_and_priority() {
    let mut node1 = mock_node("192.0.2.11:9400", &[]);
    node1.load_per_cpu = 0.1;
    node1.mem_avail_mb = 64 * 1024; // 64 GB

    let mut node2 = mock_node("192.0.2.12:9400", &[]);
    node2.load_per_cpu = 0.9;
    node2.mem_avail_mb = 16 * 1024; // 16 GB

    let srvs = generate_srv_records(&[node1, node2]);
    assert_eq!(srvs.len(), 2);

    // Node 1 (low load) has priority 10, weight 64
    assert_eq!(srvs[0].priority, 10);
    assert_eq!(srvs[0].weight, 64);
    assert_eq!(srvs[0].port, 9400);
    assert_eq!(srvs[0].target, "node-192-0-2-11.code.internal");

    // Node 2 (high load) has priority 20, weight 16
    assert_eq!(srvs[1].priority, 20);
    assert_eq!(srvs[1].weight, 16);
    assert_eq!(srvs[1].port, 9400);
    assert_eq!(srvs[1].target, "node-192-0-2-12.code.internal");
}

#[test]
fn test_srv_targets_resolve_to_advertised_nodes() {
    let node1 = mock_node("192.0.2.10:9400", &[("shop", "rust", 1)]);
    let node2 = mock_node("192.0.2.20:9401", &[("billing", "go", 2)]);
    let node3 = mock_node("192.0.2.30:9400", &[]);
    let nodes = vec![node1.clone(), node2.clone(), node3.clone()];

    let srv_records = generate_srv_records(&nodes);
    assert_eq!(srv_records.len(), 3);

    // End-to-end SRV then A lookup: each advertised SRV target must resolve back to that exact node
    for srv in &srv_records {
        let resolved = resolve_smart_domain(&srv.target, &nodes, 9400)
            .unwrap_or_else(|| panic!("failed to resolve target {}", srv.target));
        assert_eq!(resolved.len(), 1);
        let addr = resolved[0];
        assert_eq!(addr.port(), srv.port);
        assert!(
            nodes.iter().any(|n| n.addr == addr),
            "resolved address {} must match an advertised node",
            addr
        );
    }
}

#[test]
fn test_dns_records_serialization() {
    let srv = DnsSrvRecord {
        priority: 10,
        weight: 64,
        port: 9400,
        target: "node-192-0-2-10.code.internal".to_string(),
    };
    let serialized = serde_json::to_string(&srv).expect("serialize srv");
    let deserialized: DnsSrvRecord = serde_json::from_str(&serialized).expect("deserialize srv");
    assert_eq!(srv, deserialized);
}

#[test]
fn test_resolve_smart_domain_empty_nodes_fallback() {
    let addrs = resolve_smart_domain("shop.code.internal:9400", &[], 9400).expect("fallback");
    assert_eq!(addrs, vec!["127.0.0.1:9400".parse().unwrap()]);
}

#[test]
fn test_non_internal_srv_domain_rejected() {
    let attacker_name = "_prod-code._tcp.attacker.example";
    assert!(!is_code_internal_domain(attacker_name));
    assert!(!is_srv_service_domain(attacker_name));

    let node1 = mock_node("192.0.2.10:9400", &[]);
    let nodes = vec![node1];

    let attacker_query = format_dns_response(
        777,
        &DnsQuestion {
            name: attacker_name.into(),
            qtype: DnsQueryType::SRV,
            qclass: 1,
        },
        &[],
        false,
        0,
    );

    assert!(handle_dns_packet(&attacker_query, &nodes).is_none());
}
