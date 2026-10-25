/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "../../../../tests/support/lsp_secret_spawn.rs"]
mod fixture;

use futures_util::SinkExt;
use prod_code_protocol::{PathTranslator, ProdCodeCodec, WireMessage, readiness::ReadySignal};
use std::time::Duration;
use tokio_util::codec::Framed;

use super::{EditorProxyOptions, EditorServers, ServerCommand, run_with_options};

const DRIVER: &str = concat!(module_path!(), "::lsp_secret_spawn_driver");

#[tokio::test]
async fn lsp_secret_spawn_inherited() {
    fixture::run_driver(DRIVER, "inherited").await;
}

#[tokio::test]
async fn lsp_secret_spawn_configured() {
    fixture::run_driver(DRIVER, "configured").await;
}

#[tokio::test]
#[ignore = "invoked in an isolated subprocess with fixture-only environment"]
async fn lsp_secret_spawn_driver() {
    let fixture = fixture::Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (client, (server, _)) = tokio::try_join!(
        tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
        listener.accept()
    )
    .unwrap();
    let mut client = Framed::new(client, ProdCodeCodec::new());
    for payload in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"exit"}"#,
    ] {
        client
            .send(WireMessage::LspPayload(payload.to_string()))
            .await
            .unwrap();
    }
    let root = fixture.root.path().to_string_lossy();
    tokio::time::timeout(
        Duration::from_secs(10),
        run_with_options(
            Framed::new(server, ProdCodeCodec::new()),
            PathTranslator::new(&root, &root),
            ServerCommand {
                program: "python3".to_string(),
                args: fixture.args.clone(),
                env: fixture.env.clone(),
                initialization_options: None,
                ready: ReadySignal::Unknown,
            },
            fixture.root.path(),
            &EditorServers::default(),
            1,
            EditorProxyOptions {
                health_probe_interval: None,
                ..Default::default()
            },
        ),
    )
    .await
    .expect("editor proxy retires its fake server")
    .unwrap();
    fixture.assert_scrubbed();
}
