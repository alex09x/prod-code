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

#[test]
fn runtime_paths_2_discovery_seed_inputs_deduplicate_and_ignore_modes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cluster.json");
    let a: SocketAddr = "127.0.0.1:9001".parse().unwrap();
    let b: SocketAddr = "127.0.0.1:9002".parse().unwrap();
    let c: SocketAddr = "[::1]:9003".parse().unwrap();
    for contents in [
        None,
        Some("invalid"),
        Some("{}"),
        Some("{\"nodes\":[\"bad\",\"127.0.0.1:9001\",\"127.0.0.1:9001\"]}"),
    ] {
        if let Some(contents) = contents {
            std::fs::write(&path, contents).unwrap();
        }
        let specs = [
            " auto ",
            "10G",
            "TRUE",
            "1",
            "LAN",
            " ,127.0.0.1:9001,127.0.0.1:9002,127.0.0.1:9001,",
            "[::1]:9003",
            "invalid address",
        ]
        .map(String::from);
        assert_eq!(discovery_seeds(Some(&path), &specs), [a, b, c]);
    }
    assert_eq!(discovery_seeds(None, &[]), []);
}

#[test]
fn runtime_paths_2_parse_numeric_remotes_and_errors() {
    assert_eq!(
        parse_remotes(" , [::1]:9001, 127.0.0.1:9002, [::1]:9001, ").unwrap(),
        [
            "[::1]:9001".parse::<SocketAddr>().unwrap(),
            "127.0.0.1:9002".parse().unwrap()
        ]
    );
    assert_eq!(
        parse_remotes("127.0.0.1").unwrap(),
        ["127.0.0.1:9400".parse::<SocketAddr>().unwrap()]
    );
    assert_eq!(
        parse_remotes(" , ").unwrap_err().to_string(),
        "no gateway addresses given"
    );
    let err = parse_remotes("invalid address").unwrap_err().to_string();
    assert!(
        err.starts_with("cannot resolve gateway address invalid address:"),
        "{err}"
    );
}

#[test]
fn issue_1807_keeps_all_unique_resolved_addresses_in_order() {
    let first: SocketAddr = "[::1]:9400".parse().unwrap();
    let second: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let third: SocketAddr = "127.0.0.1:9401".parse().unwrap();
    let addresses = parse_remotes_with("dual.example:9400, other.example:9401", |host| {
        Ok(if host == "dual.example:9400" {
            vec![first, second, first]
        } else {
            vec![second, third]
        })
    })
    .unwrap();
    assert_eq!(addresses, [first, second, third]);
}

#[test]
fn issue_1807_resolution_errors_retain_the_gateway_context() {
    let empty = parse_remotes_with("empty.example:9400", |_| Ok(Vec::new())).unwrap_err();
    assert_eq!(
        empty.to_string(),
        "gateway address empty.example:9400 resolved to nothing"
    );
    let failed = parse_remotes_with("missing.example:9400", |_| {
        Err(std::io::Error::other("lookup failed"))
    })
    .unwrap_err();
    assert_eq!(
        failed.to_string(),
        "cannot resolve gateway address missing.example:9400: lookup failed"
    );
}

#[tokio::test]
async fn issue_1807_selects_a_live_address_after_the_first_family_refuses() {
    use futures_util::{SinkExt, StreamExt};
    use prod_code_protocol::{ProdCodeCodec, StatusResponse, WireMessage};
    use tokio_util::codec::Framed;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let live = listener.local_addr().unwrap();
    let refused = SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), live.port());
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let mut framed = Framed::new(socket, ProdCodeCodec::new());
            if let Some(Ok(WireMessage::StatusRequest)) = framed.next().await {
                let _ = framed
                    .send(WireMessage::StatusResponse(StatusResponse {
                        detected_engines: vec!["rust".into()],
                        ..Default::default()
                    }))
                    .await;
            }
        }
    });
    let nodes = parse_remotes_with("dual.example:9400", |_| Ok(vec![refused, live])).unwrap();
    let picked = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        crate::cluster::pick_node_with(&nodes, "dns-fallback", Some("rust"), None, None),
    )
    .await
    .expect("candidate probing must remain bounded")
    .unwrap();
    server.abort();
    assert_eq!(picked, live);
}
