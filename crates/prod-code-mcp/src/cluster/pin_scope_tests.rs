/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Which policy governs which checkout (#1793): a blank pin shadows nothing, another checkout's
//! own pin applies to calls that select it unless the command line pins every checkout, scoped
//! policies stay with their own call, and artifacts are pulled only from allowed nodes.

use super::*;
use crate::cluster::pin::{Endpoint, PIN_RESUMED_ENV, PinMode, resolve_policy_with};
use crate::cluster::pin_fake::{FakeGateway, Kind, checkout, strict, write};
use crate::exec::pull_node_with as pull;
use serde_json::json;

/// A resolver that names the pinned loopback node by two addresses.
fn dns(_: &str) -> std::io::Result<Vec<SocketAddr>> {
    Ok(vec![addr("[::1]:9400"), addr("127.0.0.1:9400")])
}

fn addr(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

fn resolver(item: &str) -> std::io::Result<Vec<SocketAddr>> {
    match item {
        "dual.example:9400" => Ok(vec![addr("[::1]:9400"), addr("127.0.0.1:9400")]),
        _ => Err(std::io::Error::other("unknown host")),
    }
}

#[test]
fn issue_1793_a_blank_pin_does_not_shadow_the_workspace_pin() {
    for blank in ["", "   ", "\t"] {
        let policy = resolve_policy_with(
            PinRequest {
                pin: Some(blank),
                pin_source: "PROD_CODE_PIN_NODE",
                workspace_pin: Some("dual.example:9400"),
                remote: "",
                listed: None,
            },
            resolver,
        )
        .unwrap();
        assert_eq!(policy.mode, PinMode::Strict, "{blank:?}");
        assert_eq!(policy.source, WORKSPACE_PIN_SOURCE, "{blank:?}");
    }
    for off in ["auto", "off", "none", " OFF "] {
        let policy = resolve_policy_with(
            PinRequest {
                pin: Some(off),
                pin_source: "PROD_CODE_PIN_NODE",
                workspace_pin: Some("dual.example:9400"),
                remote: "",
                listed: None,
            },
            resolver,
        )
        .unwrap();
        assert_eq!(policy, NodePolicy::default(), "{off:?}");
    }
}

#[test]
fn issue_1793_a_workspace_pin_resumes_as_that_checkouts_pin_only() {
    let from_file = strict(addr("127.0.0.1:9400"), WORKSPACE_PIN_SOURCE);
    assert_eq!(
        from_file.reexec_env(),
        [(PIN_RESUMED_ENV.to_string(), "127.0.0.1:9400".to_string())]
    );
    let explicit = strict(addr("127.0.0.1:9400"), "--pin-node");
    assert_eq!(explicit.reexec_env()[0].0, crate::cluster::PIN_ENV);
}

#[test]
fn issue_1793_another_checkout_uses_its_own_pin_unless_the_command_line_pins() {
    let (_main_dir, main) = checkout(&[]);
    let (_pinned_dir, pinned) =
        checkout(&[("prod-code.toml", "[cluster]\npin = \"127.0.0.7:9400\"\n")]);
    let (_off_dir, off) = checkout(&[("prod-code.toml", "[cluster]\npin = \"auto\"\n")]);
    let (_plain_dir, plain) = checkout(&[]);
    let startup = strict(addr("127.0.0.1:9400"), WORKSPACE_PIN_SOURCE);
    let inputs = PinInputs {
        root: Some(main.clone()),
        ..PinInputs::default()
    };

    // The startup checkout keeps the startup policy.
    assert_eq!(
        policy_for_root_with(&main, &inputs, &startup).unwrap(),
        startup
    );
    // A sibling with its own pin uses it; `auto` there turns the pin off for it alone.
    let sibling = policy_for_root_with(&pinned, &inputs, &startup).unwrap();
    assert_eq!(
        sibling.strict_endpoint().unwrap().addrs,
        [addr("127.0.0.7:9400")]
    );
    assert_eq!(sibling.source, WORKSPACE_PIN_SOURCE);
    assert_eq!(
        policy_for_root_with(&off, &inputs, &startup).unwrap(),
        NodePolicy::default()
    );
    // A sibling without a pin of its own never loosens the startup pin.
    assert_eq!(
        policy_for_root_with(&plain, &inputs, &startup).unwrap(),
        startup
    );

    // A pin from the command line or the environment governs every checkout.
    let explicit = PinInputs {
        pin: Some("127.0.0.1:9400".into()),
        ..inputs.clone()
    };
    assert_eq!(
        policy_for_root_with(&pinned, &explicit, &startup).unwrap(),
        startup
    );
    // A sibling pin outside a typed --remote set is a conflict, not a silent widening.
    let listed = PinInputs {
        remote: "127.0.0.1:9400".into(),
        listed: Some(vec![addr("127.0.0.1:9400")]),
        ..inputs
    };
    let err = policy_for_root_with(&pinned, &listed, &startup).unwrap_err();
    assert!(format!("{err:#}").contains("does not include"), "{err:#}");
}

#[tokio::test]
async fn issue_1793_concurrent_calls_keep_their_own_scoped_policy() {
    let (_a_dir, a) = checkout(&[]);
    let (_b_dir, b) = checkout(&[]);
    let pin_a = strict(addr("127.0.0.11:9400"), "a");
    let pin_b = strict(addr("127.0.0.12:9400"), "b");
    let observe = |root: std::path::PathBuf| async move {
        let mut seen = Vec::new();
        for _ in 0..20 {
            tokio::task::yield_now().await;
            seen.push((node_policy().source, policy_for_root(&root).unwrap().source));
        }
        seen
    };
    let (seen_a, seen_b) = tokio::join!(
        with_root_policy(&a, pin_a, observe(a.clone())),
        with_root_policy(&b, pin_b, observe(b.clone())),
    );
    assert!(
        seen_a.iter().all(|s| *s == ("a".into(), "a".into())),
        "{seen_a:?}"
    );
    assert!(
        seen_b.iter().all(|s| *s == ("b".into(), "b".into())),
        "{seen_b:?}"
    );
    // Outside any call the process policy is untouched.
    assert_eq!(node_policy(), process_policy());
}

#[test]
fn issue_1793_pulls_come_only_from_nodes_the_pin_allows() {
    let pinned = addr("127.0.0.1:9400");
    let policy = NodePolicy {
        endpoints: vec![Endpoint {
            name: "localhost:9400".into(),
            addrs: vec![pinned],
        }],
        ..strict(pinned, "--pin-node")
    };
    let outside = addr("127.0.0.9:9400");
    // Another address the pinned name resolves to is accepted.
    assert_eq!(
        pull(&policy, pinned, Some("localhost:9400"), None, dns).unwrap(),
        pinned
    );
    // A requested node outside the pin is refused, by address or by name.
    let err = pull(&policy, pinned, Some("127.0.0.9:9400"), None, dns).unwrap_err();
    assert!(
        err.to_string().contains("cannot pull from 127.0.0.9:9400"),
        "{err}"
    );
    let err = pull(&policy, pinned, Some("elsewhere:9400"), None, |_| {
        Ok(vec![outside])
    })
    .unwrap_err();
    assert!(
        err.to_string().contains("pinned to localhost:9400"),
        "{err}"
    );
    // A remembered exec node outside the pin is passed over; one inside is used.
    assert_eq!(
        pull(&policy, pinned, None, Some(outside), dns).unwrap(),
        pinned
    );
    assert_eq!(
        pull(&policy, outside, None, Some(outside), dns).unwrap(),
        pinned
    );
    // Unpinned, the remembered node is followed as before.
    let auto = NodePolicy::default();
    assert_eq!(
        pull(&auto, pinned, None, Some(outside), dns).unwrap(),
        outside
    );
}

#[tokio::test]
async fn issue_1793_code_pull_refuses_a_remote_outside_the_pin_before_connecting() {
    let (_dir, root) = checkout(&[]);
    let pinned = FakeGateway::start(Kind::Current).await;
    let outside = FakeGateway::start(Kind::Current).await;
    let policy = strict(pinned.addr, "--pin-node");
    let result = with_root_policy(
        &root,
        policy.clone(),
        crate::tools::execute_tool(
            pinned.addr,
            &root,
            "code_pull",
            json!({ "files": ["target/out.bin"], "remote": outside.addr.to_string() }),
        ),
    )
    .await;
    let said = match result {
        Err(err) => format!("{err:#}"),
        Ok(result) => serde_json::to_string(&result).unwrap(),
    };
    assert!(said.contains("cannot pull from"), "{said}");
    assert!(outside.seen().is_empty(), "{:?}", outside.seen());

    // The pinned node by another of its names is accepted, and confirmed first.
    let by_name = format!("localhost:{}", pinned.addr.port());
    let _ = with_root_policy(
        &root,
        policy,
        crate::tools::execute_tool(
            pinned.addr,
            &root,
            "code_pull",
            json!({ "files": ["target/out.bin"], "remote": by_name }),
        ),
    )
    .await;
    let frames = pinned.frames();
    assert_eq!(
        frames
            .iter()
            .take(2)
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["NodePinRequest", "PullFileRequest"],
        "{frames:?}"
    );
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[tokio::test]
async fn issue_1793_an_mcp_call_on_a_sibling_worktree_follows_that_worktrees_pin() {
    let (_main_dir, main) = checkout(&[]);
    git(&main, &["init", "-q"]);
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-q", "-m", "init"]);
    let sibling_dir = tempfile::Builder::new()
        .prefix("pin-sib")
        .tempdir()
        .unwrap();
    let sibling = sibling_dir.path().join("wt");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "pin-sib",
            sibling.to_str().unwrap(),
        ],
    );
    let sibling = sibling.canonicalize().unwrap();
    let old = FakeGateway::start(Kind::Old).await;
    let current = FakeGateway::start(Kind::Current).await;
    write(
        &sibling,
        "prod-code.toml",
        &format!("[cluster]\npin = \"{}\"\n", old.addr),
    );

    // Selected by `workspace`, and by a path inside it: either way the sibling's pin holds,
    // the old gateway it names sees only the pin request, and the process's node sees nothing.
    for args in [
        json!({ "argv": ["true"], "workspace": sibling.to_str().unwrap() }),
        json!({ "argv": ["true"], "cwd": sibling.join("src").to_str().unwrap() }),
    ] {
        let said = match crate::tools::execute_tool(current.addr, &main, "code_exec", args).await {
            Err(err) => format!("{err:#}"),
            Ok(result) => serde_json::to_string(&result).unwrap(),
        };
        assert!(said.contains("did not confirm the node pin"), "{said}");
        assert!(current.seen().is_empty(), "{:?}", current.seen());
    }
    assert!(
        old.frames().iter().all(|f| f == "NodePinRequest"),
        "{:?}",
        old.frames()
    );
    assert_eq!(old.frames().len(), 2);

    // The outer MCP retry must use the selected checkout's policy too: a failure
    // before the inner call must not trigger cluster discovery for its pinned root.
    let discovered = std::sync::atomic::AtomicUsize::new(0);
    let mut remote = current.addr;
    let request = json!({ "jsonrpc": "2.0", "id": 99, "method": "tools/call",
        "params": { "name": "code_outline", "arguments": {
            "workspace": sibling, "path": "src/lib.rs" } } });
    crate::server::dispatch::handle_mcp_request_policy(
        &mut remote,
        &main,
        request,
        &NodePolicy::default(),
        |_, _| {
            discovered.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { None }
        },
    )
    .await
    .unwrap();
    assert_eq!(
        discovered.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a sibling pin must also constrain the MCP dispatcher's failover"
    );
    assert_eq!(remote, current.addr);

    // The configured checkout, unpinned, still runs on the node it was given.
    let result = crate::tools::execute_tool(
        current.addr,
        &main,
        "code_exec",
        json!({ "argv": ["true"] }),
    )
    .await
    .unwrap();
    assert!(
        current.frames().contains(&"ExecRequest".to_string()),
        "{result:?}"
    );
    assert!(!current.frames().contains(&"NodePinRequest".to_string()));
}
