/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What Rust Intelligence (`ridx lsp`) declares beyond LSP, and how its answers are trusted:
//! a diagnostic pull that waits for the compiler's diagnostics of the version sent, and
//! `ridx/status` for whether an index answer is final.

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::engine::GenericLspEngine;

/// `capabilities.experimental.ridx.requestOptions.minVersion`: requests may carry `minVersion`.
pub fn declares_min_version(capabilities: &Value) -> bool {
    level(capabilities, "/experimental/ridx/requestOptions/minVersion") >= 1
}

/// `capabilities.experimental.ridx.methods["textDocument/diagnostic"]`: a pull with
/// `minVersion: N` answers the compiler's diagnostics of version N, an empty list only when
/// that compile found nothing, and fails when it has none. A ridx that takes `minVersion` but
/// does not declare this (0.3.10, 0.3.11) gates the pull on its index, not on the compile, and
/// can answer `[]` for text it never type-checked.
pub fn declares_compiler_pulls(capabilities: &Value) -> bool {
    declares_min_version(capabilities)
        && level(
            capabilities,
            "/experimental/ridx/methods/textDocument~1diagnostic",
        ) >= 1
}

/// `capabilities.experimental.ridx.methods["ridx/status"]`.
pub fn declares_status(capabilities: &Value) -> bool {
    level(capabilities, "/experimental/ridx/methods/ridx~1status") >= 1
}

/// Whether the server is Rust Intelligence at all (declares anything under `experimental.ridx`).
pub fn is_ridx(capabilities: &Value) -> bool {
    capabilities.pointer("/experimental/ridx").is_some()
}

fn level(capabilities: &Value, pointer: &str) -> u64 {
    capabilities
        .pointer(pointer)
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// Why an index answer of a `ridx/status` result is not final: a root whose last build failed,
/// or that has no index. `None` when every root is indexed without an error.
pub fn index_failure(status: &Value) -> Option<String> {
    let Some(roots) = status.get("roots").and_then(Value::as_array) else {
        return Some("ridx/status lists no roots".to_string());
    };
    roots.iter().find_map(|root| {
        let name = root.get("root").and_then(Value::as_str).unwrap_or("?");
        if let Some(error) = root.get("error").and_then(Value::as_str) {
            return Some(format!("the ridx index of {name} failed: {error}"));
        }
        (root.get("indexed").and_then(Value::as_bool) != Some(true))
            .then(|| format!("the ridx index of {name} is not built"))
    })
}

impl GenericLspEngine {
    async fn caps_satisfy(&self, test: fn(&Value) -> bool) -> bool {
        self.capabilities.read().await.as_ref().is_some_and(test)
    }

    /// Whether the server declares the `minVersion` request option.
    pub async fn takes_min_version(&self) -> bool {
        self.caps_satisfy(declares_min_version).await
    }

    /// For Rust Intelligence, the diagnostics of the text last sent for `uri`, and the only
    /// answer: a pull with `minVersion` that fails (timeout, `diagnosticsUnavailable`, a lost
    /// reply) is an error, never replaced by a publication of the same version, which can be
    /// provisional. A ridx that does not declare compiler-backed pulls is refused with what to
    /// upgrade. `None` for every other server.
    /// Which server this is comes from how it was configured (the `ridx` preset), not only
    /// from what it declares: ridx 0.3.8 declares no `experimental.ridx`, and a plain pull of
    /// it answers `[]` for text it never type-checked.
    pub async fn ridx_diagnostics(&self, uri: &str) -> Option<Result<Vec<Value>>> {
        if !(self.config.is_ridx() || self.caps_satisfy(is_ridx).await) {
            return None;
        }
        Some(self.compiler_pull(uri).await)
    }

    async fn compiler_pull(&self, uri: &str) -> Result<Vec<Value>> {
        if !self.caps_satisfy(declares_compiler_pulls).await {
            anyhow::bail!(
                "this ridx does not declare compiler-backed diagnostic pulls \
                 (capabilities.experimental.ridx.methods[\"textDocument/diagnostic\"]); its \
                 diagnostics of unsaved text can miss errors, so no edit is validated with it. \
                 Upgrade Rust Intelligence"
            );
        }
        let version = self
            .sent
            .read()
            .await
            .get(uri)
            .and_then(|s| s.version)
            .ok_or_else(|| anyhow!("{uri} was not sent with a version"))?;
        let answer = self
            .send_request(
                "textDocument/diagnostic",
                serde_json::json!({ "textDocument": { "uri": uri }, "minVersion": version }),
            )
            .await?;
        if let Some(err) = answer.get("error") {
            anyhow::bail!(
                "ridx has no diagnostics for version {version} of {uri}: {} {}",
                err.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed"),
                err.get("data").map(Value::to_string).unwrap_or_default()
            );
        }
        match answer.pointer("/result/kind").and_then(Value::as_str) {
            Some("full") => answer
                .pointer("/result/items")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| anyhow!("ridx answered a full report without items for {uri}")),
            other => Err(anyhow!(
                "ridx answered a {} report for version {version} of {uri}; only a full report \
                 for that version counts",
                other.unwrap_or("malformed")
            )),
        }
    }

    /// For Rust Intelligence after it has reported its index ready: an error when `ridx/status`
    /// shows a root whose index failed or is not built. The progress `end` is sent for a failed
    /// build too, and an older index may still answer, so the end alone does not make an index
    /// answer final.
    pub(crate) async fn ridx_index_gate(&self) -> Result<()> {
        if !self.caps_satisfy(declares_status).await {
            return Ok(());
        }
        // Boxed: the status request goes through `send_request`, which calls this gate (not
        // for `ridx/status` itself, which needs no index).
        let answer = Box::pin(self.send_request("ridx/status", serde_json::json!({}))).await?;
        let status = answer.get("result").ok_or_else(|| {
            anyhow!(
                "ridx/status failed: {}",
                answer
                    .get("error")
                    .map(Value::to_string)
                    .unwrap_or_default()
            )
        })?;
        match index_failure(status) {
            Some(reason) => Err(anyhow!("{reason}; its index answers are not final")),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ridx(methods: Value) -> Value {
        json!({"experimental": {"ridx": {"methods": methods, "requestOptions": {"minVersion": 1}}}})
    }

    #[test]
    fn only_a_declared_min_version_option_is_used() {
        assert!(declares_min_version(&ridx(json!({"ridx/status": 1}))));
        for caps in [
            json!({}),
            json!({"experimental": {"ridx": {"methods": {"ridx/status": 1}}}}),
            json!({"experimental": {"ridx": {"requestOptions": {"minVersion": 0}}}}),
            json!({"experimental": {"other": {"requestOptions": {"minVersion": 1}}}}),
        ] {
            assert!(!declares_min_version(&caps), "{caps}");
        }
    }

    #[test]
    fn compiler_pulls_need_their_own_declaration_besides_min_version() {
        // 0.3.10 and 0.3.11: minVersion gates on the index, not on the compile.
        assert!(!declares_compiler_pulls(&ridx(json!({"ridx/status": 1}))));
        assert!(declares_compiler_pulls(&ridx(
            json!({"ridx/status": 1, "textDocument/diagnostic": 1})
        )));
        let no_min_version =
            json!({"experimental": {"ridx": {"methods": {"textDocument/diagnostic": 1}}}});
        assert!(!declares_compiler_pulls(&no_min_version));
        assert!(is_ridx(&no_min_version) && !is_ridx(&json!({})));
    }

    #[test]
    fn a_failed_or_missing_index_is_not_final() {
        let ok = json!({"roots": [{"root": "/w", "indexed": true, "error": null}]});
        assert_eq!(index_failure(&ok), None);
        let failed = json!({"roots": [{"root": "/w", "indexed": true, "error": "no driver"}]});
        assert_eq!(
            index_failure(&failed).as_deref(),
            Some("the ridx index of /w failed: no driver")
        );
        let unbuilt = json!({"roots": [{"root": "/w", "indexed": false, "error": null}]});
        assert_eq!(
            index_failure(&unbuilt).as_deref(),
            Some("the ridx index of /w is not built")
        );
        assert!(index_failure(&json!({})).is_some());
    }
}
