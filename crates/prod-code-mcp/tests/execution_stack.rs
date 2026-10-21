/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[test]
fn tool_execution_does_not_embed_dispatch_state_in_callers() {
    let call = prod_code_mcp::tools::execute_tool(
        "127.0.0.1:1".parse().unwrap(),
        std::path::Path::new("."),
        "code_hover",
        serde_json::json!({"path": "main.go", "line": 1, "character": 1}),
    );
    let bytes = std::mem::size_of_val(&call);
    assert!(
        bytes <= 1024,
        "tool future embeds {bytes} bytes in every awaiting caller; keep dispatch state on the heap"
    );
}
