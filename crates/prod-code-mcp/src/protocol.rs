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
pub const AGENT_INSTRUCTIONS: &str = "prod-code is a remote code-intelligence gateway: the checkout is mirrored to a LAN server that keeps a warm analyzer (rust-analyzer in memory, gopls, clangd, TypeScript, pyright, sourcekit-lsp) and runs builds and tests there. Your local edits are picked up automatically before every call (a file watcher syncs the delta); never run sync by hand.\n\nUse the semantic tools instead of text search: code_definition / code_references / code_callers / code_callees / code_implementations resolve symbols exactly (an identifier can be found by grep, but only these tell you what it is); code_hover gives the signature and docs; code_outline lists a file's symbols; code_source shows library and SDK sources that live only on the server. Every position tool also takes `symbol` (a name, optionally qualified: `Metrics::record`, `pkg.Func`, `Class.method`) instead of path/line/character, and code_symbols searches the workspace index by name — never grep for a line number to feed a tool.\n\nBefore writing a file, call code_validate_edit with the complete proposed content: it returns the analyzer's errors (type errors, unresolved names, hallucinated APIs) in well under a second without touching disk. Write only after it is clean; use code_diagnostics on a file you did not write yourself.\n\nFor refactors use code_rename (workspace-wide, applied to the checkout), code_assists / code_assist (quick fixes and refactorings, including compiler fix-its) and code_safe_delete.\n\nBuilds and tests run on the server: code_check (compile), code_lint, code_test (parsed results; `path` runs only that crate, package or directory) and code_exec for any command (formatters, generators and lockfile changes are written back). After a change, code_impact tells which tests are affected and gives the command that runs only them; when tests fail, code_diagnose_failure explains each failure with the code at the failing site, its callers and the diff. code_dead_code lists unreferenced symbols. Never build or test on the local machine when these tools are available.\n\nWhen a prod-code tool itself is at fault (a wrong or empty answer, a hang, a crash, an error that does not say what to do, a call far slower than it should be, or a capability you needed and it lacks), report it with code_report_issue as soon as you are sure, then carry on with your task. The title says what went wrong in which tool, for which language; the body gives the exact call with its arguments, what came back, what you expected and how to reproduce it. The issue is public: never put private details in it, such as host names, IP addresses, internal paths or repository names, credentials, or logs with internal data (addresses, the host name and home paths are also removed automatically). When reproducing needs them, write them into your own private record first, such as an incident in your team's knowledge base, and pass its id as `private_ref`; without one, file the issue without them. When a similar issue is listed, add to it with a comment instead of filing another.";

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
