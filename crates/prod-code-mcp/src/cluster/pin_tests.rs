/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Node policies (#1793): how they are resolved, and that per-call routing honours them.

use super::*;
use crate::cluster::routing::{Routing, route_with};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn addr(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

/// A resolver that knows `dual.example` by two addresses of one node.
fn resolver(item: &str) -> std::io::Result<Vec<SocketAddr>> {
    match item {
        "dual.example:9400" => Ok(vec![addr("[::1]:9400"), addr("127.0.0.1:9400")]),
        "other.example:9400" => Ok(vec![addr("127.0.0.2:9400")]),
        _ => Err(std::io::Error::other("unknown host")),
    }
}

fn request(pin: Option<&'static str>, workspace_pin: Option<&'static str>) -> PinRequest<'static> {
    PinRequest {
        pin,
        pin_source: "--pin-node",
        workspace_pin,
        remote: "other.example:9400",
        listed: None,
    }
}

#[test]
fn issue_1793_an_unpinned_workspace_and_an_env_seed_stay_automatic() {
    let policy = resolve_policy_with(request(None, None), resolver).unwrap();
    assert_eq!(policy, NodePolicy::default());
    assert!(!policy.is_pinned());
    assert!(policy.allows(addr("192.0.2.9:9400")));
    assert!(policy.offers_redirects());
    assert!(policy.reexec_env().is_empty());
}

#[test]
fn issue_1793_a_strict_pin_keeps_every_address_of_its_one_endpoint() {
    let policy = resolve_policy_with(request(Some("dual.example:9400"), None), resolver).unwrap();
    assert_eq!(policy.mode, PinMode::Strict);
    let endpoint = policy.strict_endpoint().unwrap();
    assert_eq!(endpoint.name, "dual.example:9400");
    assert_eq!(endpoint.addrs, [addr("[::1]:9400"), addr("127.0.0.1:9400")]);
    assert_eq!(
        policy.alternatives(addr("[::1]:9400")),
        [addr("127.0.0.1:9400")]
    );
    assert!(!policy.allows(addr("127.0.0.2:9400")));
    assert!(!policy.offers_redirects());
    let refusal = policy
        .check(addr("127.0.0.2:9400"), "this command")
        .unwrap_err()
        .to_string();
    assert!(
        refusal.contains("pinned to dual.example:9400 (--pin-node)"),
        "{refusal}"
    );
    assert_eq!(
        policy.reexec_env(),
        [(PIN_ENV.to_string(), "dual.example:9400".to_string())]
    );
}

#[test]
fn issue_1793_command_line_pin_wins_over_the_workspace_and_auto_turns_it_off() {
    let workspace = request(None, Some("dual.example:9400"));
    let policy = resolve_policy_with(workspace, resolver).unwrap();
    assert_eq!(policy.source, "prod-code.toml [cluster] pin");
    assert_eq!(policy.strict_endpoint().unwrap().name, "dual.example:9400");

    let overridden = resolve_policy_with(
        request(Some("other.example:9400"), Some("dual.example:9400")),
        resolver,
    )
    .unwrap();
    assert_eq!(
        overridden.strict_endpoint().unwrap().name,
        "other.example:9400"
    );

    let off = resolve_policy_with(request(Some("auto"), Some("dual.example:9400")), resolver);
    assert_eq!(off.unwrap(), NodePolicy::default());

    // `--pin-node remote` pins to the seed only because it was asked to.
    let seed = resolve_policy_with(request(Some("remote"), None), resolver).unwrap();
    assert_eq!(seed.strict_endpoint().unwrap().name, "other.example:9400");
}

#[test]
fn issue_1793_a_listed_remote_set_never_grows_and_must_hold_a_pin() {
    let listed = [addr("127.0.0.2:9400"), addr("127.0.0.3:9400")];
    let req = PinRequest {
        listed: Some(&listed),
        remote: "127.0.0.2:9400,127.0.0.3:9400",
        ..request(None, None)
    };
    let policy = resolve_policy_with(req, resolver).unwrap();
    assert_eq!(policy.mode, PinMode::Listed);
    assert_eq!(policy.allowed(), listed);
    assert!(!policy.allows(addr("127.0.0.4:9400")));

    let conflict = PinRequest {
        pin: Some("dual.example:9400"),
        ..req
    };
    let err = resolve_policy_with(conflict, resolver)
        .unwrap_err()
        .to_string();
    assert!(err.contains("does not include dual.example:9400"), "{err}");
}

#[test]
fn issue_1793_unusable_pins_fail_closed_with_the_reason() {
    for (pin, expected) in [
        ("dual.example:9400,other.example:9400", "holds one endpoint"),
        ("shop.code.internal", "pin a gateway's host:port"),
        ("missing.example:9400", "cannot resolve the pinned endpoint"),
    ] {
        let err = resolve_policy_with(request(Some(pin), None), resolver)
            .unwrap_err()
            .to_string();
        assert!(err.contains(expected), "{pin}: {err}");
    }
    let empty = resolve_policy_with(request(Some("dual.example:9400"), None), |_| Ok(vec![]));
    assert!(
        empty
            .unwrap_err()
            .to_string()
            .contains("resolved to nothing")
    );
}

#[test]
fn issue_1793_workspace_config_is_read_from_prod_code_toml() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(workspace_pin(dir.path()).unwrap(), None);
    let file = dir.path().join("prod-code.toml");
    std::fs::write(&file, "[rust]\nall_targets = true\n").unwrap();
    assert_eq!(workspace_pin(dir.path()).unwrap(), None);
    std::fs::write(&file, "[cluster]\npin = \"build.example:9400\"\n").unwrap();
    assert_eq!(
        workspace_pin(dir.path()).unwrap().as_deref(),
        Some("build.example:9400")
    );
    std::fs::write(&file, "[cluster]\npin = 9400\n").unwrap();
    assert!(workspace_pin(dir.path()).is_err());
    std::fs::write(&file, "[cluster\npin = \"x\"\n").unwrap();
    let err = workspace_pin(dir.path()).unwrap_err().to_string();
    assert!(err.contains("node pin cannot be read"), "{err}");
}

#[test]
fn issue_1793_a_pinned_session_refuses_redirects_outside_the_pin() {
    let policy = NodePolicy {
        mode: PinMode::Strict,
        endpoints: vec![Endpoint {
            name: "127.0.0.1:9400".into(),
            addrs: vec![addr("127.0.0.1:9400")],
        }],
        source: "--pin".into(),
    };
    assert!(policy.check_redirect("127.0.0.1:9400").is_ok());
    let err = policy
        .check_redirect("127.0.0.9:9400")
        .unwrap_err()
        .to_string();
    assert!(err.contains("refusing to follow the redirect"), "{err}");
    assert!(
        NodePolicy::default()
            .check_redirect("127.0.0.9:9400")
            .is_ok()
    );
}

/// A node that counts every connection and serves `engines`; a placement request names `place`.
struct Counted {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Counted {
    async fn start(engines: &'static [&'static str], place: Option<SocketAddr>) -> Self {
        use futures_util::{SinkExt, StreamExt};
        use prod_code_protocol::{PlaceResponse, ProdCodeCodec, StatusResponse, WireMessage};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let counted = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    let mut framed = tokio_util::codec::Framed::new(socket, ProdCodeCodec::new());
                    while let Some(Ok(message)) = framed.next().await {
                        let answer = match message {
                            WireMessage::StatusRequest => {
                                WireMessage::StatusResponse(StatusResponse {
                                    detected_engines: engines
                                        .iter()
                                        .map(|e| e.to_string())
                                        .collect(),
                                    platform: Some("linux x86_64".into()),
                                    ..Default::default()
                                })
                            }
                            WireMessage::PlaceRequest(_) => {
                                WireMessage::PlaceResponse(PlaceResponse {
                                    node: place.map(|p| p.to_string()),
                                    reason: "test".into(),
                                    notified_sessions: 0,
                                })
                            }
                            _ => return,
                        };
                        if framed.send(answer).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self {
            addr,
            connections,
            task,
        }
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn go_checkout() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("pin-route")
        .tempdir()
        .unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
    std::fs::create_dir_all(dir.path().join("tools/util")).unwrap();
    std::fs::write(
        dir.path().join("tools/util/go.mod"),
        "module example.org/util\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("tools/util/main.go"),
        "package main\n\nfunc main() {}\n",
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn issue_1793_per_call_routing_stays_on_a_strict_pin_without_probing() {
    let root = go_checkout();
    let pinned = Counted::start(&["rust"], None).await;
    let unlisted = Counted::start(&["go"], None).await;
    let routing = Routing {
        nodes: vec![pinned.addr, unlisted.addr],
        workspace: "pin-route".into(),
        policy: NodePolicy {
            mode: PinMode::Strict,
            endpoints: vec![Endpoint {
                name: pinned.addr.to_string(),
                addrs: vec![pinned.addr],
            }],
            source: "--pin".into(),
        },
    };
    let state = tempfile::tempdir().unwrap();
    let placement = state.path().join("placement.json");
    let chosen = route_with(
        &routing,
        pinned.addr,
        root.path(),
        "tools/util/main.go",
        Some(&placement),
    )
    .await
    .unwrap();
    assert_eq!(chosen, pinned.addr);
    assert_eq!(
        unlisted.connections(),
        0,
        "no work reaches the unlisted node"
    );
    assert_eq!(
        pinned.connections(),
        0,
        "no probe can fail once and move the call"
    );

    // A default outside the pin is refused, not used.
    let err = route_with(
        &routing,
        unlisted.addr,
        root.path(),
        "tools/util/main.go",
        Some(&placement),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("refusing"), "{err}");
    assert_eq!(unlisted.connections(), 0);
}

#[tokio::test]
async fn issue_1793_per_call_routing_moves_only_within_the_listed_set() {
    let root = go_checkout();
    let unlisted = Counted::start(&["go"], None).await;
    // The listed rust node would place the Go project on the unlisted node, if asked.
    let first = Counted::start(&["rust"], Some(unlisted.addr)).await;
    let second = Counted::start(&["go"], Some(unlisted.addr)).await;
    let listed = vec![first.addr, second.addr];
    let routing = Routing {
        nodes: listed.clone(),
        workspace: "pin-route".into(),
        policy: NodePolicy {
            mode: PinMode::Listed,
            endpoints: vec![Endpoint {
                name: "listed".into(),
                addrs: listed,
            }],
            source: "--remote".into(),
        },
    };
    let state = tempfile::tempdir().unwrap();
    let placement = state.path().join("placement.json");
    let chosen = route_with(
        &routing,
        first.addr,
        root.path(),
        "tools/util/main.go",
        Some(&placement),
    )
    .await
    .unwrap();
    assert_eq!(chosen, second.addr, "the listed Go node serves the Go path");
    assert_eq!(unlisted.connections(), 0, "no placement request names it");
}
