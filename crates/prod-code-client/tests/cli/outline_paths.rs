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
async fn cli_outline_missing_path_suggests_split_module() {
    let ws = make_workspace();
    ws.write("src/manager/mod.rs", "pub fn load() {}\n");
    let gw = MockGateway::start(|_, _| serde_json::Value::Null).await;
    for path in [
        "src/manager.rs".to_string(),
        ws.path("src/manager.rs").display().to_string(),
    ] {
        let out = run_cli(&ws, gw.addr, &["outline", &path]).await;
        assert!(!out.status.success(), "{}", stdout_of(&out));
        let stderr = stderr_of(&out);
        assert!(stderr.contains("file does not exist"), "{stderr}");
        assert!(
            stderr.contains("did you mean `src/manager/mod.rs`"),
            "{stderr}"
        );
        assert!(!stderr.contains("Invalid file path for URI"), "{stderr}");
    }
    assert_eq!(
        gw.initializes.load(Ordering::SeqCst),
        0,
        "missing paths must fail before engine load"
    );
}
