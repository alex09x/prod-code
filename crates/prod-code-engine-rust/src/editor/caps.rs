/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What an editor said in `initialize` it can take back, so that an answer uses no form it did
//! not advertise: snippet placeholders in completions, `documentChanges` in a workspace edit,
//! and file creation or renaming among them (#1516).

use serde_json::Value;

/// The response forms an editor supports. The default is everything, for a caller that sent no
/// `initialize` (a tool rather than an editor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCaps {
    /// `textDocument.completion.completionItem.snippetSupport`.
    pub snippets: bool,
    /// `workspace.workspaceEdit.documentChanges`.
    pub document_changes: bool,
    /// `workspace.workspaceEdit.resourceOperations` includes `create`.
    pub create_files: bool,
    /// `workspace.workspaceEdit.resourceOperations` includes `rename`.
    pub rename_files: bool,
}

impl Default for EditorCaps {
    fn default() -> Self {
        Self {
            snippets: true,
            document_changes: true,
            create_files: true,
            rename_files: true,
        }
    }
}

impl EditorCaps {
    /// The forms the `capabilities` of an `initialize` request's params advertise; what is not
    /// advertised is not supported, as LSP defines.
    pub fn from_initialize(params: &Value) -> Self {
        let caps = &params["capabilities"];
        let flag = |pointer: &str| caps.pointer(pointer).and_then(Value::as_bool) == Some(true);
        let operations: Vec<&str> = caps
            .pointer("/workspace/workspaceEdit/resourceOperations")
            .and_then(Value::as_array)
            .map(|ops| ops.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        Self {
            snippets: flag("/textDocument/completion/completionItem/snippetSupport"),
            document_changes: flag("/workspace/workspaceEdit/documentChanges"),
            create_files: operations.contains(&"create"),
            rename_files: operations.contains(&"rename"),
        }
    }
}
