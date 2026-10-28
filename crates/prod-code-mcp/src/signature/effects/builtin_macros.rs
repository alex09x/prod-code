/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::{collections::BTreeSet, net::SocketAddr, path::Path, time::Duration};

use crate::signature::types::CallSite;

const MAX_STRINGIFY_HOVER_QUERIES: usize = 16;

/// Treat `stringify!` as effect-free only when the analyzer confirms the built-in macro binding.
pub async fn mark_builtin_stringify_args(
    remote: SocketAddr,
    root: &Path,
    calls: &mut [CallSite],
    plan: &[Option<usize>],
    declared_len: usize,
    has_receiver: bool,
) {
    let mut changed = BTreeSet::new();
    for (new_position, old_position) in plan.iter().enumerate() {
        if let Some(old_position) = old_position
            && *old_position != new_position
        {
            changed.insert(*old_position);
        }
    }
    for original in 0..declared_len {
        if !plan.contains(&Some(original)) {
            changed.insert(original);
        }
    }
    if changed.is_empty() {
        return;
    }

    let mut queries = 0;
    for call in calls {
        if call.stringify_calls.is_empty() {
            continue;
        }
        let receiver_offset = usize::from(has_receiver && call.args.len() == declared_len + 1);
        let Some(site) = call.stringify_calls.iter().find(|site| {
            site.argument >= receiver_offset && changed.contains(&(site.argument - receiver_offset))
        }) else {
            continue;
        };
        if queries >= MAX_STRINGIFY_HOVER_QUERIES {
            return;
        }
        queries += 1;
        let Ok(uri) = url::Url::from_file_path(&call.file) else {
            continue;
        };
        let params = serde_json::json!({
            "textDocument": { "uri": uri.as_str() },
            "position": {
                "line": site.line.saturating_sub(1),
                "character": site.column.saturating_sub(1)
            }
        });
        let hover = tokio::time::timeout(
            Duration::from_millis(750),
            crate::tools::execute_lsp_query(remote, root, &call.file, "textDocument/hover", params),
        )
        .await;
        let is_builtin = matches!(hover, Ok(Ok(ref value)) if builtin_stringify_hover(value));
        if is_builtin {
            call.builtin_stringify_args.extend(
                call.stringify_calls
                    .iter()
                    .filter(|site| site.argument >= receiver_offset)
                    .filter(|site| changed.contains(&(site.argument - receiver_offset)))
                    .map(|site| site.argument),
            );
        }
    }
}

fn builtin_stringify_hover(value: &serde_json::Value) -> bool {
    let contents = value.get("contents");
    let text = match contents {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Object(content)) => content
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                part.as_str()
                    .or_else(|| part.get("value").and_then(serde_json::Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    text.contains("core::macros::builtin") && text.contains("macro_rules! stringify")
}
