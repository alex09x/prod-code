/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::VecDeque;

use super::reconnect_pending::replay_request;

pub(super) fn normalize_deferred(
    deferred: VecDeque<String>,
) -> (VecDeque<String>, Vec<(serde_json::Value, i64)>) {
    let canceled: Vec<serde_json::Value> = deferred
        .iter()
        .filter_map(|frame| {
            let value: serde_json::Value = serde_json::from_str(frame).ok()?;
            (value.get("method")?.as_str()? == "$/cancelRequest")
                .then(|| value.pointer("/params/id").cloned())
                .flatten()
        })
        .collect();
    let mut frames = VecDeque::new();
    let mut rejected = Vec::new();
    let mut later_edit = false;
    for frame in deferred.into_iter().rev() {
        let value: serde_json::Value = match serde_json::from_str(&frame) {
            Ok(value) => value,
            Err(_) => {
                frames.push_front(frame);
                continue;
            }
        };
        let method = value.get("method").and_then(serde_json::Value::as_str);
        if method == Some("$/cancelRequest") {
            continue;
        }
        if matches!(
            method,
            Some("textDocument/didOpen" | "textDocument/didChange" | "textDocument/didClose")
        ) {
            later_edit = true;
        }
        if let (Some(id), Some(method)) = (value.get("id"), method) {
            if canceled.contains(id) {
                rejected.push((id.clone(), -32800));
            } else if later_edit {
                rejected.push((id.clone(), -32801));
            } else if let Some(request) = replay_request(method, &frame) {
                frames.push_front(request);
            } else {
                rejected.push((id.clone(), -32801));
            }
        } else {
            frames.push_front(frame);
        }
    }
    rejected.reverse();
    (frames, rejected)
}

pub(super) async fn prepare_deferred(
    deferred: &mut VecDeque<String>,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
) -> anyhow::Result<()> {
    let (frames, rejected) = normalize_deferred(std::mem::take(deferred));
    *deferred = frames;
    let mut output = editor_out.lock().await;
    for (id, code) in rejected {
        let message = if code == -32800 {
            "request cancelled"
        } else {
            "request became stale during reconnect"
        };
        let reply =
            serde_json::json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}});
        prod_code_client::editor_files::write_frame(&mut *output, &reply.to_string()).await?;
    }
    Ok(())
}

/// Finish requests accepted while disconnected before ending a healthy editor stream.
pub(super) async fn fail_deferred(
    deferred: &mut VecDeque<String>,
    editor_out: &tokio::sync::Mutex<tokio::io::Stdout>,
) -> anyhow::Result<()> {
    let frames = std::mem::take(deferred);
    let canceled: Vec<serde_json::Value> = frames
        .iter()
        .filter_map(|frame| {
            let value: serde_json::Value = serde_json::from_str(frame).ok()?;
            (value.get("method")?.as_str()? == "$/cancelRequest")
                .then(|| value.pointer("/params/id").cloned())
                .flatten()
        })
        .collect();
    let mut seen = Vec::new();
    let mut output = editor_out.lock().await;
    for frame in frames {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&frame) else {
            continue;
        };
        let (Some(id), Some(_)) = (value.get("id"), value.get("method")) else {
            continue;
        };
        if id.is_null() || seen.contains(id) {
            continue;
        }
        seen.push(id.clone());
        let code = if canceled.contains(id) {
            -32800
        } else {
            -32097
        };
        let reply = serde_json::json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":"editor session ended during reconnect"}});
        prod_code_client::editor_files::write_frame(&mut *output, &reply.to_string()).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_session_bound_requests_are_rejected_or_normalized() {
        let deferred = std::collections::VecDeque::from([
            r#"{"id":1,"method":"completionItem/resolve","params":{"data":"old"}}"#.to_string(),
            r#"{"id":2,"method":"textDocument/semanticTokens/full/delta","params":{"previousResultId":"old"}}"#.to_string(),
        ]);
        let (frames, rejected) = normalize_deferred(deferred);
        assert_eq!(rejected, vec![(serde_json::json!(1), -32801)]);
        let request: serde_json::Value = serde_json::from_str(&frames[0]).unwrap();
        assert_eq!(request["method"], "textDocument/semanticTokens/full");
        assert!(request["params"].get("previousResultId").is_none());
    }

    #[test]
    fn queued_canceled_requests_do_not_reach_the_new_server() {
        let deferred = std::collections::VecDeque::from([
            r#"{"id":4,"method":"textDocument/hover"}"#.to_string(),
            r#"{"method":"$/cancelRequest","params":{"id":4}}"#.to_string(),
        ]);
        let (frames, rejected) = normalize_deferred(deferred);
        assert!(frames.is_empty());
        assert_eq!(rejected, vec![(serde_json::json!(4), -32800)]);
    }

    #[test]
    fn a_later_queued_edit_invalidates_an_earlier_request() {
        let deferred = std::collections::VecDeque::from([
            r#"{"id":4,"method":"textDocument/hover"}"#.to_string(),
            r#"{"method":"textDocument/didChange"}"#.to_string(),
            r#"{"id":5,"method":"textDocument/hover"}"#.to_string(),
        ]);
        let (frames, rejected) = normalize_deferred(deferred);
        assert_eq!(rejected, vec![(serde_json::json!(4), -32801)]);
        assert_eq!(frames.len(), 2);
        let request: serde_json::Value = serde_json::from_str(&frames[1]).unwrap();
        assert_eq!(request["id"], 5);
    }
}
