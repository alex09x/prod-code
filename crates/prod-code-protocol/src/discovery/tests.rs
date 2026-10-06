/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::client::generate_nonce;
use super::types::{compute_auth_tag, verify_auth_tag};
use super::wire::{
    format_minimal_node_line, format_node_line, format_node_line_with_nonce, format_probe,
    format_probe_with_nonce, inspect_probe, is_valid_probe, parse_node_line,
    parse_node_line_with_auth, parse_node_line_with_auth_and_nonce,
};
use std::net::IpAddr;

#[test]
fn format_and_parse_round_trips() {
    let ws = vec![
        ("prod-code".into(), "rust".into(), 2),
        ("CodeHaus".into(), "go".into(), 1),
    ];
    let line = format_node_line(
        "192.168.2.168:9400",
        "rust,go,python",
        4200,
        0.0712,
        32,
        128000,
        64000,
        3,
        &ws,
        None,
    );
    let node = parse_node_line(&line).expect("should parse");
    assert_eq!(node.addr, "192.168.2.168:9400".parse().unwrap());
    assert_eq!(node.engines, vec!["rust", "go", "python"]);
    assert_eq!(node.rss_mb, 4200);
    assert!((node.load_per_cpu - 0.0712).abs() < 0.001);
    assert_eq!(node.cpus, 32);
    assert_eq!(node.mem_total_mb, 128000);
    assert_eq!(node.mem_avail_mb, 64000);
    assert_eq!(node.sessions, 3);
    assert_eq!(node.workspaces.len(), 2);
    assert_eq!(node.workspaces[0].name, "prod-code");
    assert_eq!(node.workspaces[0].engine, "rust");
    assert_eq!(node.workspaces[0].sessions, 2);
    assert_eq!(node.workspaces[1].name, "CodeHaus");
}

#[test]
fn workspace_names_with_spaces_and_delimiters_are_safely_preserved() {
    let ws = vec![
        ("My Project with spaces".into(), "rust".into(), 1),
        ("repo,with,commas:and:colons".into(), "go".into(), 2),
        ("русский проект".into(), "python".into(), 0),
    ];
    let line = format_node_line(
        "10.0.0.1:9400",
        "rust,go,python",
        500,
        0.1,
        8,
        16000,
        8000,
        3,
        &ws,
        None,
    );
    let node = parse_node_line(&line).expect("should parse despite spaces and colons in names");
    assert_eq!(node.workspaces.len(), 3);
    assert_eq!(node.workspaces[0].name, "My Project with spaces");
    assert_eq!(node.workspaces[1].name, "repo,with,commas:and:colons");
    assert_eq!(node.workspaces[2].name, "русский проект");
}

#[test]
fn authenticated_announcement_round_trips() {
    let token = "secret-cluster-token-12345";
    let line = format_node_line(
        "192.168.2.100:9400",
        "rust",
        200,
        0.05,
        4,
        8000,
        4000,
        1,
        &[],
        Some(token),
    );
    let sender_ip: IpAddr = "192.168.2.100".parse().unwrap();
    let node = parse_node_line_with_auth(&line, Some(token), Some(sender_ip))
        .expect("should accept valid authenticated announcement");
    assert_eq!(node.addr, "192.168.2.100:9400".parse().unwrap());
}

#[test]
fn forged_announcement_rejected_when_token_configured() {
    let token = "secret-cluster-token-12345";
    // Unauthenticated line (tag = -)
    let unauth_line = format_node_line(
        "192.168.2.100:9400",
        "rust",
        200,
        0.05,
        4,
        8000,
        4000,
        1,
        &[],
        None,
    );
    let sender_ip: IpAddr = "192.168.2.100".parse().unwrap();
    assert!(
        parse_node_line_with_auth(&unauth_line, Some(token), Some(sender_ip)).is_none(),
        "must reject unauthenticated announce when token is configured"
    );

    // Forged line with wrong token
    let wrong_token_line = format_node_line(
        "192.168.2.100:9400",
        "rust",
        200,
        0.05,
        4,
        8000,
        4000,
        1,
        &[],
        Some("wrong-token"),
    );
    assert!(
        parse_node_line_with_auth(&wrong_token_line, Some(token), Some(sender_ip)).is_none(),
        "must reject announce with wrong token"
    );
}

#[test]
fn anti_spoofing_rejects_mismatched_sender_ip() {
    let line = format_node_line(
        "192.168.2.168:9400",
        "rust",
        200,
        0.05,
        4,
        8000,
        4000,
        1,
        &[],
        None,
    );
    // Sender IP is attacker at 192.168.2.99 pretending to advertise 192.168.2.168
    let attacker_ip: IpAddr = "192.168.2.99".parse().unwrap();
    assert!(
        parse_node_line_with_auth(&line, None, Some(attacker_ip)).is_none(),
        "must reject spoofed sender IP"
    );

    // Legitimate sender matching advertised IP is accepted
    let real_ip: IpAddr = "192.168.2.168".parse().unwrap();
    assert!(parse_node_line_with_auth(&line, None, Some(real_ip)).is_some());
}

#[test]
fn probe_validation_with_token() {
    let token = "my-secret-token";
    let valid_probe = format_probe(Some(token));
    assert!(is_valid_probe(&valid_probe, Some(token)));

    let unauth_probe = b"PROD_CODE_DISCOVER\n";
    assert!(!is_valid_probe(unauth_probe, Some(token)));

    let wrong_token_probe = format_probe(Some("wrong-token"));
    assert!(!is_valid_probe(&wrong_token_probe, Some(token)));
}

#[test]
fn parse_minimal_legacy_line() {
    let line = "PROD_CODE_NODE 10.0.0.1:9400 rust 0 0.0";
    let node = parse_node_line(line).unwrap();
    assert_eq!(node.addr, "10.0.0.1:9400".parse().unwrap());
    assert_eq!(node.engines, vec!["rust"]);
    assert_eq!(node.cpus, 0);
    assert_eq!(node.mem_total_mb, 0);
    assert!(node.workspaces.is_empty());
}

#[test]
fn parse_garbage_returns_none() {
    assert!(parse_node_line("hello world").is_none());
    assert!(parse_node_line("PROD_CODE_NODE badaddr rust 0 0").is_none());
}

#[test]
fn hmac_sha256_mac_computation_and_verification() {
    let token = "test-cluster-secret-key-32bytes!";
    let data = "192.168.2.168:9400:rust,go:32:128000";

    let tag = compute_auth_tag(token, data);
    assert_eq!(
        tag.len(),
        64,
        "HMAC-SHA-256 hex string must be 64 characters (256 bits)"
    );

    // Valid verification
    assert!(verify_auth_tag(token, data, &tag));

    // Wrong token fails
    assert!(!verify_auth_tag("different-token", data, &tag));

    // Tampered payload fails
    assert!(!verify_auth_tag(
        token,
        "192.168.2.168:9400:rust,go:32:128001",
        &tag
    ));

    // Tampered tag (single bit flip) fails
    let mut tampered_tag = tag.clone();
    let last_char = if tampered_tag.ends_with('0') {
        '1'
    } else {
        '0'
    };
    tampered_tag.pop();
    tampered_tag.push(last_char);
    assert!(!verify_auth_tag(token, data, &tampered_tag));

    // Truncated / malformed tag fails
    assert!(!verify_auth_tag(token, data, &tag[..32]));
    assert!(!verify_auth_tag(
        token,
        data,
        "invalid-hex-characters-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
    ));
}

#[test]
fn tampering_any_announcement_field_fails_verification() {
    let token = "secret-cluster-token-987654";
    let ws = vec![("my-app".into(), "rust".into(), 1)];
    let valid_line = format_node_line(
        "192.168.2.168:9400",
        "rust,go",
        1000,
        0.05,
        16,
        64000,
        32000,
        2,
        &ws,
        Some(token),
    );
    let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();

    // 1. Valid line must pass
    assert!(parse_node_line_with_auth(&valid_line, Some(token), Some(sender_ip)).is_some());

    let tokens: Vec<&str> = valid_line.split_whitespace().collect();

    // 2. Tampering with engines
    let mut tampered = tokens.clone();
    tampered[3] = "rust,go,python";
    assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

    // 3. Tampering with rss_mb
    let mut tampered = tokens.clone();
    tampered[4] = "2000";
    assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

    // 4. Tampering with load_per_cpu
    let mut tampered = tokens.clone();
    tampered[5] = "0.0100";
    assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

    // 5. Tampering with cpus
    let mut tampered = tokens.clone();
    tampered[6] = "32";
    assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

    // 6. Tampering with mem_total_mb
    let mut tampered = tokens.clone();
    tampered[7] = "128000";
    assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

    // 7. Tampering with mem_avail_mb (routing priority!)
    let mut tampered = tokens.clone();
    tampered[8] = "60000";
    assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

    // 8. Tampering with sessions
    let mut tampered = tokens.clone();
    tampered[9] = "10";
    assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());

    // 9. Tampering with workspaces (warm-cache routing priority!)
    let mut tampered = tokens.clone();
    tampered[10] = "other-app:rust:1";
    assert!(parse_node_line_with_auth(&tampered.join(" "), Some(token), Some(sender_ip)).is_none());
}

#[test]
fn minimal_node_announcement_satisfies_privacy_and_parses() {
    let token = "privacy-token-secret";
    let line = format_minimal_node_line("192.168.2.168:9400", "rust,go", Some(token));
    let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();
    let node = parse_node_line_with_auth(&line, Some(token), Some(sender_ip))
        .expect("minimal announcement should parse and authenticate");

    assert_eq!(node.addr, "192.168.2.168:9400".parse().unwrap());
    assert_eq!(node.engines, vec!["rust", "go"]);
    assert_eq!(node.rss_mb, 0);
    assert_eq!(node.load_per_cpu, 0.0);
    assert_eq!(node.cpus, 0);
    assert_eq!(node.mem_total_mb, 0);
    assert_eq!(node.mem_avail_mb, 0);
    assert_eq!(node.sessions, 0);
    assert!(
        node.workspaces.is_empty(),
        "workspaces must be stripped for privacy"
    );
    assert!(node.nonce.is_none());
}

#[test]
fn challenge_nonce_probe_and_reply_verification() {
    let token = "test-token-nonce";
    let nonce = generate_nonce().unwrap();
    assert_eq!(nonce.len(), 32);

    // Probe formatting and inspection
    let probe = format_probe_with_nonce(Some(token), Some(&nonce));
    let inspected = inspect_probe(&probe, Some(token)).expect("probe must be valid");
    assert_eq!(inspected.as_deref(), Some(nonce.as_str()));

    // Reply formatting with nonce
    let ws = vec![("secure-project".into(), "rust".into(), 1)];
    let reply_line = format_node_line_with_nonce(
        "192.168.2.168:9400",
        "rust",
        500,
        0.1,
        8,
        16000,
        8000,
        1,
        &ws,
        Some(token),
        Some(&nonce),
    );
    let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();

    // Valid reply with matching nonce is accepted
    let node = parse_node_line_with_auth_and_nonce(
        &reply_line,
        Some(token),
        Some(sender_ip),
        Some(&nonce),
    )
    .expect("should accept valid reply with matching nonce");
    assert_eq!(node.nonce.as_deref(), Some(nonce.as_str()));
    assert_eq!(node.workspaces.len(), 1);
    assert_eq!(node.workspaces[0].name, "secure-project");

    // Reply with wrong expected nonce is rejected
    let different_nonce = generate_nonce().unwrap();
    assert!(
        parse_node_line_with_auth_and_nonce(
            &reply_line,
            Some(token),
            Some(sender_ip),
            Some(&different_nonce)
        )
        .is_none(),
        "must reject when expected nonce differs from wire nonce"
    );
}

#[test]
fn replayed_announcement_without_nonce_fails_when_nonce_expected() {
    let token = "test-token-replay";
    let old_line = format_node_line(
        "192.168.2.168:9400",
        "rust",
        500,
        0.1,
        8,
        16000,
        8000,
        1,
        &[],
        Some(token),
    );
    let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();
    let fresh_nonce = generate_nonce().unwrap();

    assert!(
        parse_node_line_with_auth_and_nonce(
            &old_line,
            Some(token),
            Some(sender_ip),
            Some(&fresh_nonce)
        )
        .is_none(),
        "must reject replayed line that lacks fresh challenge nonce"
    );
}

#[test]
fn replayed_signed_reply_with_empty_workspaces_fails_without_nonce() {
    let token = "test-token-empty-ws";
    // Node with empty workspaces: ws_csv is "-"
    let empty_ws_reply = format_node_line(
        "192.168.2.168:9400",
        "rust",
        500,
        0.1,
        8,
        16000,
        8000,
        0,
        &[],
        Some(token),
    );
    let sender_ip: IpAddr = "192.168.2.168".parse().unwrap();
    let fresh_nonce = generate_nonce().unwrap();

    assert!(
        parse_node_line_with_auth_and_nonce(
            &empty_ws_reply,
            Some(token),
            Some(sender_ip),
            Some(&fresh_nonce)
        )
        .is_none(),
        "must reject replayed signed reply even when workspaces are empty"
    );
}
