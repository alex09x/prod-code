/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Serialize responses within the host's frame limit, preserving non-tool response shapes.
pub(crate) fn bound_serialized_response(
    mut resp: serde_json::Value,
    max_frame_bytes: usize,
) -> String {
    let Ok(initial) = serde_json::to_string(&resp) else {
        return serde_json::json!({"jsonrpc":"2.0", "error":{"code":-32603,"message":"Failed to serialize response"}}).to_string();
    };
    if initial.len() <= max_frame_bytes {
        return initial;
    }
    let text_blocks: Vec<(usize, String)> = resp
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .filter_map(|(i, b)| {
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        b.get("text")
                            .and_then(|t| t.as_str())
                            .map(|s| (i, s.to_string()))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    if text_blocks.is_empty() {
        for id in [
            resp.get("id").cloned().unwrap_or(serde_json::Value::Null),
            serde_json::Value::Null,
        ] {
            let err_resp = serde_json::json!({"jsonrpc":"2.0", "id":id,
                "error":{"code":-32000,"message":"Response exceeds maximum JSON-RPC frame size"}});
            if let Ok(s) = serde_json::to_string(&err_resp)
                && s.len() <= max_frame_bytes
            {
                return s;
            }
        }
        return r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32000,"message":"Response exceeds maximum JSON-RPC frame size"}}"#.to_string();
    }
    let marker = "\n[... output truncated to avoid exceeding MCP frame line limits]\n";
    for (idx, text) in text_blocks {
        let mut target_len = max_frame_bytes / 2;
        while target_len > 100 {
            let mut candidate_text =
                crate::verify::truncate_to_boundary(&text, target_len).to_string();
            candidate_text.push_str(marker);
            if let Some(block) = resp
                .get_mut("result")
                .and_then(|r| r.get_mut("content"))
                .and_then(|c| c.as_array_mut())
                .and_then(|arr| arr.get_mut(idx))
            {
                block["text"] = serde_json::Value::String(candidate_text);
            }
            if let Ok(serialized) = serde_json::to_string(&resp)
                && serialized.len() <= max_frame_bytes
            {
                return serialized;
            }
            target_len = target_len.saturating_sub(4096);
        }
    }
    for id in [
        resp.get("id").cloned().unwrap_or(serde_json::Value::Null),
        serde_json::Value::Null,
    ] {
        let fallback = serde_json::json!({"jsonrpc":"2.0", "id":id,
            "result":{"content":[{"type":"text","text":marker.trim_start()}],"isError":false}});
        if let Ok(s) = serde_json::to_string(&fallback)
            && s.len() <= max_frame_bytes
        {
            return s;
        }
    }
    r#"{"jsonrpc":"2.0","id":null,"result":{"content":[{"type":"text","text":"[... output truncated to avoid exceeding MCP frame line limits]\n"}],"isError":false}}"#.to_string()
}
