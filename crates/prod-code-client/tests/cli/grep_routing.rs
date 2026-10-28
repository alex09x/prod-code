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

#[tokio::test]
async fn cli_grep_searches_shell_directory_without_shell_engine() {
    let ws = make_workspace();
    ws.write("scripts/check.sh", "#!/bin/sh\necho selftest-frame\n");
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;
    // Force the normal local-search fallback after the gateway accepts the request.
    *gw.custom_exec.lock().unwrap() = Some(Vec::new());
    for args in [
        vec!["grep", "selftest-frame", "scripts"],
        vec!["grep", "selftest-frame", "--path", "scripts"],
    ] {
        let out = run_cli(&ws, gw.addr, &args).await;
        assert!(out.status.success(), "{}", stderr_of(&out));
        assert!(
            stdout_of(&out).contains("selftest-frame"),
            "{}",
            stdout_of(&out)
        );
    }
    let out = run_cli(&ws, gw.addr, &["outline", "scripts/check.sh"]).await;
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("no reachable gateway serves shell"));
}
