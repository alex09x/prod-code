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

/// Version 2 of `ridx/status`: each root says `pendingChanges` from the moment a save or a
/// watched-file change is recorded until an index that read it is loaded.
pub fn declares_pending_changes(capabilities: &Value) -> bool {
    level(capabilities, "/experimental/ridx/methods/ridx~1status") >= 2
}

/// `capabilities.experimental.ridx.methods["ridx/safeDelete"]`: the method exists.
pub fn declares_safe_delete(capabilities: &Value) -> bool {
    level(capabilities, "/experimental/ridx/methods/ridx~1safeDelete") >= 1
}

/// Version 2 of `ridx/safeDelete`: it refuses with `staleIndex` while any change it has been
/// told of (a watched file, a save) is not indexed yet, so its "no uses" covers every saved file.
pub fn declares_fresh_safe_delete(capabilities: &Value) -> bool {
    level(capabilities, "/experimental/ridx/methods/ridx~1safeDelete") >= 2
}

/// Why an index answer may not cover the latest saved changes: a root still being rebuilt, or
/// one with changes reported to ridx that its loaded index does not cover yet
/// (`pendingChanges`, set as soon as ridx is told of a change, before a rebuild starts).
pub fn index_building(status: &Value) -> Option<String> {
    let roots = status.get("roots")?.as_array()?;
    if roots.is_empty() {
        // Before ridx has queued the workspace folders it was given.
        return Some("ridx has not taken up the workspace yet".to_string());
    }
    roots.iter().find_map(|root| {
        let name = root.get("root").and_then(Value::as_str).unwrap_or("?");
        let flag = |key: &str| root.get(key).and_then(Value::as_bool) == Some(true);
        let behind = if root.get("indexed").and_then(Value::as_bool) != Some(true) {
            format!(
                "the ridx index of {name} is not built yet (a first index of a large workspace \
                 takes a minute or two)"
            )
        } else if flag("building") {
            format!("the ridx index of {name} is being rebuilt")
        } else if flag("pendingChanges") {
            format!("the ridx index of {name} does not cover the latest changes yet")
        } else {
            return None;
        };
        // The error of the run being replaced says why the next one may fail the same way
        // (a build script that cannot run on this node, a dependency it cannot fetch).
        Some(match root.get("error").and_then(Value::as_str) {
            Some(error) => format!("{behind}; its last build failed: {error}"),
            None => behind,
        })
    })
}

/// How long a question waits for a ridx index to catch up before it is answered with how far
/// the index got. Shorter than the share a client gives one lookup of a batch (about 20 s), so
/// the caller reads why instead of its own bare timeout (#2090-#2093).
pub const RIDX_GATE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// What a `ridx/safeDelete` refusal whose code alone does not say what to do means, and what to
/// do about it.
fn refusal_explained(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "unlocatedUses" => {
            "the index holds a use of the item whose file it cannot place, so the item may \
             still be used; find it with code_references or a text search, or rebuild the \
             index (remove target/ridx) and try again"
        }
        "staleIndex" => {
            "the index does not cover the current sources yet (a saved change, a file it did \
             not read, or another cfg); try again once it is reindexed"
        }
        "unsavedUses" => "another open buffer has unsaved edits; save them and try again",
        "externallyUsed" => {
            "the item is exported to the linker or is a proc macro, so code outside the \
             workspace may use it"
        }
        "containsImpl" => "the module holds an impl that code elsewhere may need",
        _ => return None,
    })
}

/// The edit of a `ridx/safeDelete` answer, or why nothing may be deleted: the refusal's reason
/// (`used`, `traitItem`, `moduleFile`, `notAnItem`, `unknownSymbol`, `unlocatedUses`, ...) with
/// the uses left, and what a reason whose code alone says too little means.
pub fn safe_delete_outcome(answer: &Value) -> Result<Value> {
    if let Some(err) = answer.get("error") {
        anyhow::bail!(
            "ridx/safeDelete failed: {}",
            err.get("message")
                .and_then(Value::as_str)
                .unwrap_or("request failed")
        );
    }
    let result = answer.get("result").unwrap_or(&Value::Null);
    if let Some(edit) = result.get("edit").filter(|e| e.is_object()) {
        return Ok(edit.clone());
    }
    if result.get("refused").and_then(Value::as_bool) == Some(true) {
        let reason = result
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("refused");
        let uses: Vec<String> = result
            .get("remainingUses")
            .and_then(Value::as_array)
            .map(|uses| {
                uses.iter()
                    .filter_map(|l| {
                        let uri = l.get("uri")?.as_str()?;
                        let start = l.pointer("/range/start")?;
                        Some(format!(
                            "{uri}:{}:{}",
                            start.get("line")?.as_u64()? + 1,
                            start.get("character")?.as_u64()? + 1
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let why = refusal_explained(reason)
            .map(|why| format!(": {why}"))
            .unwrap_or_default();
        if uses.is_empty() {
            anyhow::bail!("not deleted ({reason}){why}");
        }
        let then = if why.is_empty() { ":" } else { ";" };
        anyhow::bail!(
            "not deleted ({reason}){why}{then} {} use(s) left: {}",
            uses.len(),
            uses.join(", ")
        );
    }
    Err(anyhow!(
        "ridx/safeDelete answered neither an edit nor a refusal: {result}"
    ))
}

/// The Rust Intelligence method that answers a rust-analyzer extension the tools send, when
/// the server declares it: same request, same result.
pub fn ridx_alias(capabilities: &Value, method: &str) -> Option<&'static str> {
    let (alias, pointer) = match method {
        "rust-analyzer/viewSyntaxTree" => (
            "ridx/syntaxTree",
            "/experimental/ridx/methods/ridx~1syntaxTree",
        ),
        "rust-analyzer/expandMacro" => (
            "ridx/expandMacro",
            "/experimental/ridx/methods/ridx~1expandMacro",
        ),
        _ => return None,
    };
    (level(capabilities, pointer) >= 1).then_some(alias)
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

/// Why an index answer of a `ridx/status` result cannot become final by waiting: a root whose
/// last build failed, or a status without its `roots`. A root not built yet is not a failure:
/// [`index_building`] waits for it.
pub fn index_failure(status: &Value) -> Option<String> {
    let Some(roots) = status.get("roots").and_then(Value::as_array) else {
        return Some("ridx/status lists no roots".to_string());
    };
    roots.iter().find_map(|root| {
        let name = root.get("root").and_then(Value::as_str).unwrap_or("?");
        // The error is the last finished run's, kept while a new run builds and after a change
        // it has not started on: that run is being replaced, and [`index_building`] waits.
        let flag = |key: &str| root.get(key).and_then(Value::as_bool) == Some(true);
        if flag("building") || flag("pendingChanges") {
            return None;
        }
        root.get("error")
            .and_then(Value::as_str)
            .map(|error| format!("the ridx index of {name} failed: {error}"))
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

    /// `ridx/safeDelete {textDocument, position}` for a server that declares it: the
    /// WorkspaceEdit that deletes the item, or an error with the refusal's reason and the uses
    /// still left. `None` when the server does not declare the method.
    ///
    /// A deletion is written at once, so it is asked only of a ridx that refuses (`staleIndex`)
    /// while any saved change it has been told of is not indexed yet (version 2 of the method),
    /// and only while `ridx/status` shows every root indexed, without an error, not rebuilding.
    /// A caller saved in another file a moment ago is then never missed for an older index.
    pub async fn ridx_safe_delete(&self, params: &Value) -> Option<Result<Value>> {
        if !self.caps_satisfy(declares_safe_delete).await {
            return None;
        }
        Some(self.fresh_safe_delete(params).await)
    }

    async fn fresh_safe_delete(&self, params: &Value) -> Result<Value> {
        if !self.caps_satisfy(declares_fresh_safe_delete).await {
            anyhow::bail!(
                "this ridx's safe delete can read an index that has not caught up with changes \
                 saved in other files, so nothing is deleted with it. Upgrade Rust Intelligence \
                 (methods[\"ridx/safeDelete\"] >= 2)"
            );
        }
        let status = self.ridx_status(self.config.request_timeout).await?;
        if let Some(reason) = index_failure(&status).or_else(|| index_building(&status)) {
            anyhow::bail!("{reason}; nothing is deleted until it is indexed");
        }
        let request = serde_json::json!({
            "textDocument": params.get("textDocument").cloned().unwrap_or(Value::Null),
            "position": params.get("position").cloned().unwrap_or(Value::Null),
        });
        safe_delete_outcome(&self.send_request("ridx/safeDelete", request).await?)
    }

    async fn ridx_status(&self, timeout: std::time::Duration) -> Result<Value> {
        // Boxed: the status request goes through `send_request_with_timeout`, which calls the
        // index gate (not for `ridx/status` itself, which needs no index).
        let answer =
            Box::pin(self.send_request_with_timeout("ridx/status", serde_json::json!({}), timeout))
                .await?;
        answer.get("result").cloned().ok_or_else(|| {
            anyhow!(
                "ridx/status failed: {}",
                answer
                    .get("error")
                    .map(Value::to_string)
                    .unwrap_or_default()
            )
        })
    }

    /// For Rust Intelligence after it has reported its index ready: an error when `ridx/status`
    /// shows a root whose index failed or is not built. The progress `end` is sent for a failed
    /// build too, and an older index may still answer, so the end alone does not make an index
    /// answer final.
    ///
    /// A root being rebuilt, or with reported changes not indexed yet, is waited for (up to the
    /// index wait): a question asked right after a sync or a command wrote a file must not be
    /// answered from the index of the files before it.
    pub(crate) async fn ridx_index_gate(&self) -> Result<()> {
        if !self.caps_satisfy(declares_status).await {
            // A server configured as ridx that declares no status at all (0.3.8) cannot say
            // whether its index is current either.
            if self.config.is_ridx() {
                anyhow::bail!(
                    "this ridx does not report whether its index covers the latest changes \
                     (ridx/status 2); upgrade Rust Intelligence"
                );
            }
            return Ok(());
        }
        if !self.caps_satisfy(declares_pending_changes).await {
            anyhow::bail!(
                "this ridx does not report whether its index covers the latest changes \
                 (ridx/status 2), so an answer could come from files as they were before; \
                 upgrade Rust Intelligence"
            );
        }
        // One deadline bounds the whole wait: every status request and every pause gets only
        // what is left of it.
        let wait = self.config.index_wait.min(RIDX_GATE_WAIT);
        let deadline = tokio::time::Instant::now() + wait;
        let behind_after = |behind: String| {
            anyhow!(
                "{behind}, still after {:.1}s; ask again in a minute. code_outline, code_grep \
                 and code_exec do not need the index and work meanwhile",
                wait.as_secs_f64()
            )
        };
        let mut last = "the ridx index did not prove current".to_string();
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return Err(behind_after(last));
            }
            let status = match tokio::time::timeout(left, self.ridx_status(left)).await {
                Ok(status) => status?,
                Err(_) => return Err(behind_after(last)),
            };
            if let Some(reason) = index_failure(&status) {
                return Err(anyhow!("{reason}; its index answers are not final"));
            }
            let Some(behind) = index_building(&status) else {
                return Ok(());
            };
            last = behind;
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            tokio::time::sleep(left.min(std::time::Duration::from_millis(100))).await;
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
    fn a_safe_delete_answer_is_an_edit_or_its_refusal_with_the_uses_left() {
        assert!(declares_safe_delete(&ridx(json!({"ridx/safeDelete": 1}))));
        assert!(!declares_safe_delete(&ridx(json!({"ridx/status": 1}))));
        assert!(!declares_fresh_safe_delete(&ridx(
            json!({"ridx/safeDelete": 1})
        )));
        assert!(declares_fresh_safe_delete(&ridx(
            json!({"ridx/safeDelete": 2})
        )));
        let building = json!({"roots": [{"root": "/w", "indexed": true, "building": true}]});
        assert_eq!(
            index_building(&building).as_deref(),
            Some("the ridx index of /w is being rebuilt")
        );
        let idle = json!({"roots": [{"root": "/w", "indexed": true, "building": false}]});
        assert_eq!(index_building(&idle), None);
        let edit = json!({"changes": {"file:///w/src/util.rs": []}});
        assert_eq!(
            safe_delete_outcome(&json!({"result": {"edit": edit}})).unwrap(),
            edit
        );
        let used = json!({"result": {"refused": true, "reason": "used", "remainingUses": [
            {"uri": "file:///w/src/lib.rs", "range": {"start": {"line": 4, "character": 9},
                                                      "end": {"line": 4, "character": 15}}}]}});
        assert_eq!(
            format!("{}", safe_delete_outcome(&used).unwrap_err()),
            "not deleted (used): 1 use(s) left: file:///w/src/lib.rs:5:10"
        );
        let trait_item =
            json!({"result": {"refused": true, "reason": "traitItem", "remainingUses": []}});
        assert_eq!(
            format!("{}", safe_delete_outcome(&trait_item).unwrap_err()),
            "not deleted (traitItem)"
        );
        // 0.3.12: a use the index holds but cannot place keeps the item, with what to do.
        let unlocated =
            json!({"result": {"refused": true, "reason": "unlocatedUses", "remainingUses": []}});
        let text = format!("{}", safe_delete_outcome(&unlocated).unwrap_err());
        assert!(text.starts_with("not deleted (unlocatedUses): "), "{text}");
        assert!(text.contains("code_references"), "{text}");
        for odd in [
            json!({"result": {}}),
            json!({"result": null}),
            json!({"error": {"message": "x"}}),
        ] {
            assert!(safe_delete_outcome(&odd).is_err(), "{odd}");
        }
    }

    #[test]
    fn rust_analyzer_extensions_go_to_the_declared_ridx_methods() {
        let caps = ridx(json!({"ridx/syntaxTree": 1, "ridx/expandMacro": 1}));
        assert_eq!(
            ridx_alias(&caps, "rust-analyzer/viewSyntaxTree"),
            Some("ridx/syntaxTree")
        );
        assert_eq!(
            ridx_alias(&caps, "rust-analyzer/expandMacro"),
            Some("ridx/expandMacro")
        );
        assert_eq!(ridx_alias(&caps, "textDocument/hover"), None);
        let undeclared = ridx(json!({"ridx/status": 1}));
        assert_eq!(
            ridx_alias(&undeclared, "rust-analyzer/viewSyntaxTree"),
            None
        );
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
        // Not built yet, or not taken up yet: waited for, not a failure.
        let unbuilt = json!({"roots": [{"root": "/w", "indexed": false, "error": null}]});
        assert_eq!(index_failure(&unbuilt), None);
        let text = index_building(&unbuilt).unwrap();
        assert!(
            text.starts_with("the ridx index of /w is not built yet"),
            "{text}"
        );
        assert_eq!(index_failure(&json!({"roots": []})), None);
        assert!(index_building(&json!({"roots": []})).is_some());
        assert_eq!(index_building(&ok), None);
        assert!(index_failure(&json!({})).is_some());
    }

    /// ridx keeps the last finished run's `error` while a new run builds, and after a change it
    /// has not started on: that error belongs to a run already being replaced, and is waited
    /// for. Refusing on it at once answered "is not asked" right after an edit fixed the build.
    #[test]
    fn an_error_of_a_run_being_replaced_is_waited_for() {
        for flag in ["building", "pendingChanges"] {
            let status = json!({"roots": [{
                "root": "/w", "indexed": true, "error": "mismatched types", flag: true
            }]});
            assert_eq!(index_failure(&status), None, "{flag}");
            // The caller still reads why the run before failed (#2092: a build script that
            // cannot start on the node fails every run the same way).
            let text = index_building(&status).unwrap();
            assert!(
                text.ends_with("; its last build failed: mismatched types"),
                "{flag}: {text}"
            );
        }
        let settled = json!({"roots": [{
            "root": "/w", "indexed": true, "error": "mismatched types",
            "building": false, "pendingChanges": false
        }]});
        assert!(index_failure(&settled).is_some());
    }
}
