//! Model Context Protocol (MCP) JSON-RPC 2.0 schemas and types.

use serde::{Deserialize, Serialize};

pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
pub const SERVER_NAME: &str = "prod-code-mcp";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolCallResult {
    pub content: Vec<McpContentItem>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContentItem {
    #[serde(rename = "text")]
    Text { text: String },
}

impl McpToolCallResult {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: vec![McpContentItem::Text {
                text: content.into(),
            }],
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: vec![McpContentItem::Text {
                text: content.into(),
            }],
            is_error: true,
        }
    }
}

/// What an agent should know to use prod-code well; sent as `instructions` at `initialize`.
pub const AGENT_INSTRUCTIONS: &str = "\
prod-code is a remote code-intelligence gateway: the checkout is mirrored to a LAN server that keeps a warm analyzer (rust-analyzer in memory, gopls, clangd, TypeScript, pyright, sourcekit-lsp) and runs builds and tests there. Your local edits are picked up automatically before every call (a file watcher syncs the delta); never run sync by hand.

Use semantic tools instead of text search:
- Navigation: code_definition / code_references / code_callers / code_callees / code_implementations resolve symbols exactly; code_hover gives signatures and docs; code_outline lists file/dir symbols; code_source shows server-side stdlib/SDK sources; code_search finds ranked declarations by doc/purpose; code_slice extracts declarations with dependencies; code_type_at inspects expression types. Every position tool takes `symbol: \"Type::method\"` instead of line/col, and code_symbols searches by name — never grep for an identifier or line number.

Safe pre-flight validation & hypotheses:
- Before writing a file, call code_validate_edit with the complete proposed content (or code_validate_edits for multi-file patches): it returns analyzer errors (type errors, unresolved names, hallucinated APIs) in memory without touching disk. Write only after clean. Use code_diagnostics on existing files.
- Before committing, call code_impact to determine the blast radius (affected functions, callers, and tests).
- When evaluating multiple implementation hypotheses or bug fixes, call code_shadow_run with alternative file sets: it executes candidate patches concurrently in private server overlays and reports the winning diff without mutating the git working tree.

Refactoring & code modernization (33 polyglot refactorings):
- Never rewrite function call sites manually: use code_change_signature (reorder, add, or remove parameters with all call sites updated), code_extract_parameter, code_introduce_parameter_object, code_inline_parameter, code_wrap_return.
- Object-oriented & pattern refactorings: code_replace_constructor_with_factory, code_replace_constructor_with_builder, code_replace_inheritance_with_delegation, code_replace_conditional_with_polymorphism, code_extract_interface, code_extract_trait, code_extract_delegate.
- Functions, methods & modules: code_extract_function, code_move, code_move_module, code_move_method, code_convert_to_method, code_make_static.
- Fields, types & schemas: code_encapsulate_field, code_extract_field, code_migrate_type, code_generify, code_invert_boolean, code_introduce_variable, code_loop_to_iterator, code_schema_rename (cross-language schema field renaming across Rust, Go, TS, Python), code_codemod (structural search & replace), code_generate_fixture.
- Renaming & code actions: code_rename (workspace-wide), code_assists / code_assist (compiler fix-its and IDE assists).
- Dead code cleanup: code_dead_code lists unreferenced symbols; code_prune_orphans removes dead code in one verified type-checked pass; code_safe_delete verifies zero references before deleting.

Builds, tests & failure diagnosis on the server:
- code_check (compile), code_lint, code_test (parsed results; `path` narrows to crate/package), code_benchmarks, and code_exec for any command.
- When tests fail, call code_diagnose_failure: it analyzes the failure site, code, callers, and diff to pinpoint the root cause. Never build or test locally when these tools are available.

When a prod-code tool itself is at fault (a wrong or empty answer, a hang, a crash, an error that does not say what to do, a call far slower than it should be, or a capability you needed and it lacks), report it with code_report_issue as soon as you are sure, then carry on with your task. The title says what went wrong in which tool, for which language; the body gives the exact call with its arguments, what came back, what you expected and how to reproduce it. The issue is public: never put private details in it, such as host names, IP addresses, internal paths or repository names, credentials, or logs with internal data (addresses, the host name and home paths are also removed automatically). When reproducing needs them, write them into your own private record first, such as an incident in your team's knowledge base, and pass its id as `private_ref`; without one, file the issue without them. Give it `labels`: one type (bug, enhancement, documentation, perf) and the areas it is about (gateway, client, mcp, cluster, worktree, infra, test). When a similar issue is listed, add to it with a comment instead of filing another.\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_response_carries_the_result_and_no_error() {
        let resp = JsonRpcResponse::success(
            Some(serde_json::json!(7)),
            serde_json::json!({ "ok": true }),
        );
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(serde_json::json!(7)));
        assert_eq!(resp.result, Some(serde_json::json!({ "ok": true })));
        assert!(resp.error.is_none());

        // `result` and `error` are mutually exclusive on the wire.
        let value = serde_json::to_value(&resp).unwrap();
        assert!(value.get("result").is_some());
        assert!(value.get("error").is_none());
    }

    #[test]
    fn error_response_carries_the_code_and_message() {
        let resp = JsonRpcResponse::error(Some(serde_json::json!(3)), -32601, "not found");
        assert_eq!(resp.id, Some(serde_json::json!(3)));
        assert!(resp.result.is_none());
        let error = resp.error.as_ref().unwrap();
        assert_eq!(error.code, -32601);
        assert_eq!(error.message, "not found");
        assert!(error.data.is_none());

        let value = serde_json::to_value(&resp).unwrap();
        assert!(value.get("result").is_none());
        assert_eq!(value["error"]["code"], -32601);
    }

    #[test]
    fn tool_call_result_text_is_not_an_error_and_error_is() {
        let ok = McpToolCallResult::text("done");
        assert!(!ok.is_error);
        let McpContentItem::Text { text } = &ok.content[0];
        assert_eq!(text, "done");

        let failed = McpToolCallResult::error("boom");
        assert!(failed.is_error);
        let McpContentItem::Text { text } = &failed.content[0];
        assert_eq!(text, "boom");
    }

    #[test]
    fn content_item_serializes_with_a_text_tag() {
        let value = serde_json::to_value(McpContentItem::Text {
            text: "hi".to_string(),
        })
        .unwrap();
        assert_eq!(value, serde_json::json!({ "type": "text", "text": "hi" }));
    }
}
