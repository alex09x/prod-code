/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::wrap_return::{WrappedReturn, Wrapper, wrap_polyglot_ext};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub fn workspace(source: &str, extension: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (&format!("selected.{extension}"), source),
        ("package.json", "{\"type\":\"module\"}"),
    ])
}

pub async fn gateway(
    ws: &Workspace,
    source: &str,
    needles: &[&str],
    extension: &str,
) -> (ScriptedGateway, Arc<AtomicUsize>) {
    let path = ws.root().join(format!("selected.{extension}"));
    let positions: Vec<_> = needles
        .iter()
        .map(|needle| {
            let at = source.find(needle).expect(needle);
            let line = source[..at].bytes().filter(|b| *b == b'\n').count() as u32 + 1;
            let col = source[source[..at].rfind('\n').map_or(0, |p| p + 1)..at]
                .encode_utf16()
                .count() as u32
                + 1;
            (line, col)
        })
        .collect();
    let diagnostics = Arc::new(AtomicUsize::new(0));
    let count = diagnostics.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => answers::locations(&path, &positions),
        "textDocument/diagnostic" => {
            count.fetch_add(1, Ordering::SeqCst);
            answers::no_diagnostics()
        }
        _ => Value::Null,
    })
    .await;
    (gateway, diagnostics)
}

pub async fn wrap(
    remote: SocketAddr,
    ws: &Workspace,
    extension: &str,
    apply: bool,
    force: bool,
) -> anyhow::Result<WrappedReturn> {
    wrap_polyglot_ext(
        remote,
        &ws.root(),
        &ws.root().join(format!("selected.{extension}")),
        Some("retry"),
        None,
        None,
        Wrapper::Promise,
        None,
        None,
        apply,
        force,
    )
    .await
}

pub async fn tool(
    remote: SocketAddr,
    ws: &Workspace,
    extension: &str,
    apply: bool,
    force: bool,
) -> Result<String, String> {
    match prod_code_mcp::tools::execute_tool(
        remote,
        &ws.root(),
        "code_wrap_return",
        json!({
            "path": format!("selected.{extension}"), "symbol": "retry", "wrapper": "promise",
            "apply": apply, "force": force,
        }),
    )
    .await
    {
        Ok(result) => {
            let text = result
                .content
                .iter()
                .map(|c| match c {
                    McpContentItem::Text { text } => text.as_str(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            if result.is_error { Err(text) } else { Ok(text) }
        }
        Err(error) => Err(format!("{error:#}")),
    }
}

pub fn planned(result: &WrappedReturn, extension: &str) -> String {
    result
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with(&format!("selected.{extension}")))
        .expect("selected file in plan")
        .1
        .clone()
}

pub fn validated(count: &AtomicUsize) {
    assert!(
        count.load(Ordering::SeqCst) > 0,
        "must validate planned text"
    );
}
