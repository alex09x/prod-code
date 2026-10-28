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

use super::hazards::swapped_and_removed_parameters;
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
    let (swapped, removed) = swapped_and_removed_parameters(plan, declared_len);
    let changed: BTreeSet<usize> = swapped
        .into_iter()
        .flat_map(|(earlier, later)| [earlier, later])
        .chain(removed)
        .collect();
    if changed.is_empty() {
        return;
    }

    let mut queries = 0;
    for call in calls {
        if call.stringify_calls.is_empty() {
            continue;
        }
        let receiver_offset = usize::from(has_receiver && call.args.len() == declared_len + 1);
        let candidates: Vec<_> = call
            .stringify_calls
            .iter()
            .copied()
            .filter(|site| {
                site.argument >= receiver_offset
                    && changed.contains(&(site.argument - receiver_offset))
            })
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let Ok(uri) = url::Url::from_file_path(&call.file) else {
            continue;
        };
        for site in candidates {
            if queries >= MAX_STRINGIFY_HOVER_QUERIES {
                return;
            }
            queries += 1;
            let params = serde_json::json!({
                "textDocument": { "uri": uri.as_str() },
                "position": {
                    "line": site.line.saturating_sub(1),
                    "character": site.column.saturating_sub(1)
                }
            });
            let hover = tokio::time::timeout(
                Duration::from_millis(750),
                crate::tools::execute_lsp_query(
                    remote,
                    root,
                    &call.file,
                    "textDocument/hover",
                    params,
                ),
            )
            .await;
            if matches!(hover, Ok(Ok(ref value)) if builtin_stringify_hover(value)) {
                call.builtin_stringify_args.insert(site.argument);
            }
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
