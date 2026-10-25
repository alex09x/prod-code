/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

// Share the real wire fixture, not a mirror of the warning consumer or runner.
#[allow(dead_code)]
#[path = "../../prod-code-mcp/tests/exec_writeback_preservation/support.rs"]
mod support;

use prod_code_testkit::Workspace;
use support::*;

#[tokio::test]
async fn cli_renders_deleted_and_rewritten_platform_warnings() {
    for (label, path, before, after, command, warns) in [
        (
            "delete",
            "file.rs",
            "fn f() { libc::open(); }",
            None,
            vec!["fixture"],
            true,
        ),
        (
            "erase",
            "file.rs",
            "fn f() { libc::open(); }",
            Some("fn f() {}"),
            vec!["fixture"],
            true,
        ),
        (
            "cfg",
            "file.rs",
            "fn f() { if cfg!(target_os = \"macos\") { old(); } }",
            Some("fn f() { if cfg!(target_os = \"macos\") { new(); } }"),
            vec!["fixture"],
            true,
        ),
        (
            "reorder",
            "file.rs",
            "fn f() { libc::foo(a, b); }",
            Some("fn f() { libc::foo(b, a); }"),
            vec!["cargo", "fmt"],
            true,
        ),
        (
            "tuple",
            "file.rs",
            "fn f() { libc::foo((value)); }",
            Some("fn f() { libc::foo((value,)); }"),
            vec!["fixture"],
            true,
        ),
        (
            "braces",
            "file.c",
            "#ifdef __APPLE__\nif (apple) { open(); cleanup(); }",
            Some("#ifdef __APPLE__\nif (apple) open(); cleanup();"),
            vec!["fixture"],
            true,
        ),
        (
            "fmt",
            "file.rs",
            "fn f() { libc::open(); }",
            Some("fn f() {\n    libc::open();\n}\n"),
            vec!["cargo", "fmt"],
            false,
        ),
    ] {
        let ws = Workspace::new(&[(path, before)]);
        let g = gateway(vec![delta(path, after)], other_os()).await;
        let address = g.address.to_string();
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
        child
            .current_dir(ws.root())
            .args(["exec", "--remote", &address, "--timeout-secs", "20", "--"])
            .args(command)
            .kill_on_drop(true);
        let client = child.output();
        let release = async {
            g.arrived.await.unwrap();
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, release);
        g.task.await.unwrap();
        let out = out.unwrap();
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(out.status.success(), "{stderr}");
        let rendered = stderr
            .lines()
            .find(|line| line.starts_with("[prod-code exec] warning:"));
        println!("CLI {label}: warning={rendered:?}");
        assert_eq!(rendered.is_some(), warns, "{stderr}");
        if let Some(line) = rendered {
            assert!(line.contains(&format!("platform ({path})")), "{stderr}");
        }
        assert_eq!(
            std::fs::read_to_string(ws.path(path)).ok().as_deref(),
            after
        );
    }
}
