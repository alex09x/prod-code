/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_testkit::Workspace;

#[tokio::test]
async fn mcp_renders_pre_write_and_nested_cfg_warnings() {
    for (label, before, after) in [
        ("delete", "fn f() { libc::open(); }", None),
        ("erase", "fn f() { libc::open(); }", Some("fn f() {}")),
        (
            "cfg",
            "fn f() { if cfg!(target_os = \"macos\") { old(); } }",
            Some("fn f() { if cfg!(target_os = \"macos\") { new(); } }"),
        ),
        (
            "not",
            "#[cfg(not(unix))] fn f() { old(); }",
            Some("#[cfg(not(unix))] fn f() { new(); }"),
        ),
        (
            "reorder",
            "fn f() { libc::foo(a, b); }",
            Some("fn f() { libc::foo(b, a); }"),
        ),
        (
            "tuple",
            "fn f() { libc::foo((value)); }",
            Some("fn f() { libc::foo((value,)); }"),
        ),
        (
            "braces",
            "#ifdef __APPLE__\nif (apple) { open(); cleanup(); }",
            Some("#ifdef __APPLE__\nif (apple) open(); cleanup();"),
        ),
    ] {
        let ws = Workspace::new(&[("file.rs", before)]);
        let g = gateway(vec![delta("file.rs", after)], other_os()).await;
        let root = ws.root();
        let client = prod_code_mcp::tools::execute_tool(
            g.address,
            &root,
            "code_exec",
            serde_json::json!({"argv": ["fixture"], "timeout_secs": 20}),
        );
        let release = async {
            g.arrived.await.unwrap();
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, release);
        g.task.await.unwrap();
        let value = serde_json::to_value(out.unwrap()).unwrap();
        let text = value["content"][0]["text"].as_str().unwrap();
        let rendered = text.lines().find(|line| line.starts_with("[warning:"));
        println!("MCP {label}: warning={rendered:?}");
        assert!(
            text.contains("[1 file(s) changed by the command were written back: file.rs]"),
            "{text}"
        );
        assert!(
            rendered.is_some_and(|line| line.contains("platform (file.rs)") && line.ends_with(']')),
            "{text}"
        );
    }
}
