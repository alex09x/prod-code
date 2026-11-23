/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A Rust editor server chosen while build scripts were allowed is not started if, by the time
//! it would be, the workspace's prod-code.toml forbids them (a sync between the command's
//! choice and the server's registration).

use prod_code_protocol::{PathTranslator, ProdCodeCodec, readiness::ReadySignal};
use std::time::Duration;
use tokio_util::codec::Framed;

use super::{EditorProxyOptions, EditorServers, ServerCommand, run_with_options};

#[cfg(unix)]
#[tokio::test]
async fn a_rust_editor_server_is_not_started_once_its_policy_forbids_build_scripts() {
    use std::os::unix::fs::PermissionsExt;
    let checkout = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(checkout.path().join(".git")).unwrap();
    let root = checkout.path().join("crates/inner");
    std::fs::create_dir_all(&root).unwrap();
    // The server that would run: it leaves a marker if it is ever started.
    let bin = checkout.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let ridx = bin.join("ridx");
    let marker = checkout.path().join("started");
    std::fs::write(
        &ridx,
        format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&ridx, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Chosen while allowed; the prohibition arrives before the server is started.
    std::fs::write(
        checkout.path().join("prod-code.toml"),
        "[rust]\nbuild_scripts = false\n",
    )
    .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (_client, (server, _)) = tokio::try_join!(
        tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
        listener.accept()
    )
    .unwrap();
    let servers = EditorServers::default();
    let path = root.to_string_lossy();
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        run_with_options(
            Framed::new(server, ProdCodeCodec::new()),
            PathTranslator::new(&path, &path),
            ServerCommand {
                program: ridx.display().to_string(),
                args: vec!["lsp".to_string()],
                env: Vec::new(),
                initialization_options: None,
                ready: ReadySignal::Unknown,
            },
            &root,
            &servers,
            1,
            EditorProxyOptions {
                health_probe_interval: None,
                ..Default::default()
            },
        ),
    )
    .await
    .expect("the session ends at once");
    let err = outcome.expect_err("refused");
    assert!(
        format!("{err:#}").contains("forbids build scripts"),
        "{err:#}"
    );
    assert!(!marker.exists(), "the server was never started");
    assert_eq!(servers.count(), 0, "and nothing stays registered");
}
