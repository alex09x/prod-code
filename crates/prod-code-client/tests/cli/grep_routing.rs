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
    // The outline of a script is read from the file, so no shell server is asked for (#1851): a
    // script with no function has an empty outline, which says so, not that no node serves shell.
    let out = run_cli(&ws, gw.addr, &["outline", "scripts/check.sh"]).await;
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    assert!(stderr.contains("no outline symbols found"), "{stderr}");
    assert!(!stderr.contains("no reachable gateway"), "{stderr}");
}
