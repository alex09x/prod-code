use crate::protocol::{McpTool, McpToolCallResult};
use crate::sync::scan_workspace_files;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, SyncRequest, WireMessage};
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use url::Url;

/// Return list of tools exposed by the MCP server.
pub fn list_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "code_exec".to_string(),
            description: "Run a build, test, lint or format command on the remote gateway inside this workspace's server copy (warm per-worktree caches, 32-core server). The checkout is synced first; files the command changes (formatters, generators, lockfiles) are written back. Returns the exit code and the tail of the combined output."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "argv": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Command and arguments, e.g. [\"cargo\", \"test\", \"-p\", \"my-crate\"]"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Kill the command after this many seconds (default 3600)"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Directory inside the workspace to run in (relative to the workspace root); default: the root"
                    },
                    "tail_bytes": {
                        "type": "integer",
                        "description": "How much of the output tail to return (default 16384)"
                    }
                },
                "required": ["argv"]
            }),
        },
        McpTool {
            name: "code_check".to_string(),
            description: "Compile-check the whole workspace on the remote gateway (cargo check / go build) and return structured compiler errors and warnings with file:line:col. Nothing runs on the local machine."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 3600)" },
                    "path": { "type": "string", "description": "A file or directory inside a nested project (e.g. a SwiftPM package in a Rust repo) to verify that project instead of the root" }
                }
            }),
        },
        McpTool {
            name: "code_lint".to_string(),
            description: "Lint the whole workspace on the remote gateway (cargo clippy -D warnings / go vet) and return structured findings with file:line:col."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 3600)" },
                    "path": { "type": "string", "description": "A file or directory inside a nested project (e.g. a SwiftPM package in a Rust repo) to verify that project instead of the root" }
                }
            }),
        },
        McpTool {
            name: "code_test".to_string(),
            description: "Run tests on the remote gateway (cargo test / go test -json), optionally filtered by test name, and return pass/fail counts plus the output of each failed test."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "Test name filter (cargo test TESTNAME / go test -run)" },
                    "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 3600)" },
                    "path": { "type": "string", "description": "A file or directory inside a nested project (e.g. a SwiftPM package in a Rust repo) to verify that project instead of the root" }
                }
            }),
        },
        McpTool {
            name: "code_assists".to_string(),
            description: "List the code actions the remote analyzer offers at a 1-based position or selection: inline, extract function/variable/constant, introduce parameter, generate impl/getters, convert/rewrite forms, quick fixes. Each entry has an id (and optional subtype) for code_assist."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line" },
                    "character": { "type": "integer", "description": "1-based column" },
                    "end_line": { "type": "integer", "description": "1-based end line of a selection (optional)" },
                    "end_character": { "type": "integer", "description": "1-based end column of a selection (optional)" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_assist".to_string(),
            description: "Apply one code action from code_assists (by id, optional subtype) at the same position or selection; the resulting edits are written into the checkout and the changed paths reported."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line" },
                    "character": { "type": "integer", "description": "1-based column" },
                    "id": { "type": "string", "description": "Assist id from code_assists, e.g. extract_function" },
                    "subtype": { "type": "integer", "description": "Assist subtype from code_assists when several share an id" },
                    "end_line": { "type": "integer", "description": "1-based end line of a selection (optional)" },
                    "end_character": { "type": "integer", "description": "1-based end column of a selection (optional)" }
                },
                "required": ["path", "line", "character", "id"]
            }),
        },
        McpTool {
            name: "code_callers".to_string(),
            description: "Incoming call hierarchy: every function/method in the workspace that calls the function at a 1-based line/column, with the call sites. Semantic (resolved through the analyzer), not a text search."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_callees".to_string(),
            description: "Outgoing call hierarchy: every function/method the function at a 1-based line/column calls, with the call sites."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_implementations".to_string(),
            description: "All implementations of the trait/interface/abstract class at a 1-based line/column (or the impl blocks of a type), as locations."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_safe_delete".to_string(),
            description: "Delete the item (function, type, const, field, module) at a 1-based position only if nothing in the workspace references it; otherwise returns the list of usages that block the deletion. The edit is written into the checkout."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line of the item's name" },
                    "character": { "type": "integer", "description": "1-based column of the item's name" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_rename".to_string(),
            description: "Semantic rename of the symbol at a 1-based line/column (type, function, field, variable, module) across the whole workspace, driven by the remote analyzer. Rewrites every affected file in the checkout (and renames module files) and reports what changed."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" },
                    "new_name": { "type": "string", "description": "New identifier" }
                },
                "required": ["path", "line", "character", "new_name"]
            }),
        },
        McpTool {
            name: "code_definition".to_string(),
            description: "Find symbol definition (function, struct, type, variable, module) at specified file and 1-based line/column position."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_references".to_string(),
            description: "Find all reference locations of a symbol across the workspace at specified file and 1-based line/column position."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    },
                    "include_declarations": {
                        "type": "boolean",
                        "description": "Include the declaration site in the reference list (default: false)"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_impact".to_string(),
            description: "Blast radius of the uncommitted changes (or of the commits since a base ref): the functions the diff touches, every function that calls them (transitively, through the analyzer's call hierarchy) and the tests among those callers, plus the exact test command that runs only the affected tests."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "base": { "type": "string", "description": "Git ref to diff against (default: the working tree against HEAD)" },
                    "depth": { "type": "integer", "description": "How many caller levels to follow (default 4)" }
                }
            }),
        },
        McpTool {
            name: "code_diagnose_failure".to_string(),
            description: "Run the tests (optionally one filter) on the gateway and, for every failure, return a dossier: the failure output, the source around each location it mentions, the enclosing function and its callers, and the working-tree diff of that file. One call instead of test → grep → read → blame."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "Test name filter, as for code_test" },
                    "timeout_secs": { "type": "integer", "description": "Kill the run after this many seconds (default 3600)" }
                }
            }),
        },
        McpTool {
            name: "code_diagnostics".to_string(),
            description: "Analyzer diagnostics for one file, computed in memory without a build: syntax errors, unresolved names, type mismatches, unused items. Milliseconds, not a cargo/tsc run."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_validate_edit".to_string(),
            description: "Check a proposed new content for a file BEFORE writing it: the analyzer sees the proposed text as the document and reports errors and warnings. Nothing is written anywhere. Use it to catch hallucinated APIs, type errors and unresolved imports before touching the checkout."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute); may be a new file" },
                    "new_text": { "type": "string", "description": "The complete proposed content of the file" }
                },
                "required": ["path", "new_text"]
            }),
        },
        McpTool {
            name: "code_dead_code".to_string(),
            description: "Unreferenced functions, methods and types across the checkout, found through the analyzer's references (not text search). Exported/public symbols are counted separately unless include_exported is set; tests and entry points are skipped."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "include_exported": { "type": "boolean", "description": "Also list exported / public symbols nothing in the checkout uses" },
                    "max_files": { "type": "integer", "description": "Stop after this many source files (default 400)" }
                }
            }),
        },
        McpTool {
            name: "code_source".to_string(),
            description: "Read a source file that exists only on the gateway host: standard library sources, dependency registries (cargo, go mod cache, node_modules, site-packages) and SDK headers — the files that code_definition points at outside the checkout. Optionally a window of lines around one line."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path (or file:// URI) on the gateway, as returned by code_definition" },
                    "line": { "type": "integer", "description": "1-based line to centre on; omitted: the whole file (up to 2 MiB)" },
                    "context": { "type": "integer", "description": "Lines of context around `line` (default 30)" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_outline".to_string(),
            description: "Extract the structural symbol outline (functions, structs, enums, traits, classes) with line numbers from a source file."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum hierarchy depth (default: 3)"
                    }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_hover".to_string(),
            description: "Inspect the type signature, docstring, and documentation for a symbol at specified file and line/column position."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_type_at".to_string(),
            description: "Alias for code_hover: inspect the type and documentation of a code expression."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_status".to_string(),
            description: "Check remote prod-code gateway health, memory RSS, active engines, loaded workspaces, and network RTT."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "code_sync".to_string(),
            description: "Push local modified workspace files to the remote server over 10G LAN for immediate compilation and analysis."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional specific subfolder or file to sync. Defaults to entire workspace root."
                    }
                }
            }),
        },
    ]
}

/// Execute an MCP tool call against the remote gateway.
pub async fn execute_tool(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    match tool_name {
        "code_safe_delete" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;
            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();
            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
            });
            let edit = match execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "prodCode/safeDelete",
                params,
            )
            .await
            {
                Ok(edit) => edit,
                Err(e) => {
                    return Ok(McpToolCallResult::error(format!(
                        "safe delete refused: {e:#}"
                    )));
                }
            };
            let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
            Ok(McpToolCallResult::text(format!(
                "deleted; {} path(s) updated in the checkout:\n{}",
                touched.len(),
                touched.join("\n")
            )))
        }
        "code_assists" | "code_assist" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;
            let (end_line, end_char) = match (
                args.get("end_line").and_then(|v| v.as_u64()),
                args.get("end_character").and_then(|v| v.as_u64()),
            ) {
                (Some(l), Some(c)) => (l as u32, c as u32),
                _ => (line, character),
            };
            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();
            let mut params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "range": {
                    "start": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
                    "end": { "line": end_line.saturating_sub(1), "character": end_char.saturating_sub(1) }
                }
            });
            if tool_name == "code_assist" {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .context("Missing 'id' argument")?;
                params["id"] = serde_json::json!(id);
                if let Some(subtype) = args.get("subtype").and_then(|v| v.as_u64()) {
                    params["subtype"] = serde_json::json!(subtype);
                }
                let edit = match execute_lsp_query(
                    remote,
                    workspace_root,
                    &file_path,
                    "prodCode/applyAssist",
                    params,
                )
                .await
                {
                    Ok(edit) => edit,
                    Err(e) => {
                        return Ok(McpToolCallResult::error(format!("assist refused: {e:#}")));
                    }
                };
                let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
                return Ok(McpToolCallResult::text(format!(
                    "applied `{id}`; {} path(s) updated in the checkout:\n{}",
                    touched.len(),
                    touched.join("\n")
                )));
            }
            let list = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "prodCode/assists",
                params,
            )
            .await?;
            let mut out = String::new();
            if let Some(items) = list.as_array() {
                if items.is_empty() {
                    out.push_str("no code actions at this position\n");
                }
                for item in items {
                    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                    let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    match item.get("subtype").and_then(|v| v.as_u64()) {
                        Some(st) => {
                            out.push_str(&format!("{id} (subtype {st}) [{kind}]: {label}\n"))
                        }
                        None => out.push_str(&format!("{id} [{kind}]: {label}\n")),
                    }
                }
            }
            Ok(McpToolCallResult::text(out.trim_end().to_string()))
        }
        "code_check" | "code_lint" | "code_test" => {
            let kind = match tool_name {
                "code_check" => crate::verify::VerifyKind::Check,
                "code_lint" => crate::verify::VerifyKind::Lint,
                _ => crate::verify::VerifyKind::Test,
            };
            let filter = args
                .get("filter")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let timeout_secs = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            // `path` selects a nested project (any file or directory inside it).
            let hint = args
                .get("path")
                .and_then(|v| v.as_str())
                .map(|p| resolve_file_path(workspace_root, p));
            let report = crate::verify::run_verify(
                remote,
                workspace_root,
                hint.as_deref(),
                kind,
                filter.as_deref(),
                timeout_secs,
            )
            .await?;
            let text = report.render(40);
            Ok(if report.ok() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            })
        }
        "code_rename" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;
            let new_name = args
                .get("new_name")
                .and_then(|v| v.as_str())
                .context("Missing 'new_name' argument")?
                .to_string();
            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();
            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
                "newName": new_name
            });
            let edit = match execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/rename",
                params,
            )
            .await
            {
                Ok(edit) => edit,
                Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
            };
            if edit.is_null() {
                return Ok(McpToolCallResult::error(
                    "rename produced no edits".to_string(),
                ));
            }
            let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
            Ok(McpToolCallResult::text(format!(
                "renamed to `{new_name}`; {} path(s) updated in the checkout:\n{}",
                touched.len(),
                touched.join("\n")
            )))
        }
        "code_exec" => {
            let argv: Vec<String> = args
                .get("argv")
                .and_then(|v| v.as_array())
                .context("Missing 'argv' argument")?
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            let timeout_secs = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let tail_bytes = args
                .get("tail_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(16 * 1024) as usize;
            let mut tail = crate::exec::TailBuffer::new(tail_bytes);
            let subdir = args
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(|p| resolve_file_path(workspace_root, p))
                .and_then(|p| crate::exec::subdir_of(workspace_root, &p));
            let outcome = crate::exec::run_remote(
                remote,
                workspace_root,
                subdir.as_deref(),
                argv.clone(),
                vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
                timeout_secs,
                true,
                |_, data| tail.push(data),
            )
            .await?;
            let exit = outcome.exit;
            let status = match (&exit.error, exit.timed_out, exit.exit_code) {
                (Some(err), _, _) => format!("failed to start: {err}"),
                (None, true, _) => "timed out".to_string(),
                (None, false, Some(code)) => format!("exit code {code}"),
                (None, false, None) => "killed by signal".to_string(),
            };
            let mut text = format!(
                "$ {}\n[{status} in {:.1}s on {}; {} bytes of output{}]\n",
                argv.join(" "),
                exit.duration_ms as f64 / 1000.0,
                exit.server_workspace_root,
                tail.total,
                if tail.total > tail_bytes {
                    ", tail shown"
                } else {
                    ""
                }
            );
            if !outcome.pulled_files.is_empty() {
                text.push_str(&format!(
                    "[{} file(s) changed by the command were written back: {}]\n",
                    outcome.pulled_files.len(),
                    outcome.pulled_files.join(", ")
                ));
            }
            text.push_str(&tail.text());
            Ok(if matches!(exit.exit_code, Some(0)) {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            })
        }
        "code_definition" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;

            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
            });

            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/definition",
                params,
            )
            .await?;

            let mut out = String::new();
            if let Some(arr) = res.as_array() {
                if arr.is_empty() {
                    out.push_str("No definition found.");
                } else {
                    for (i, loc) in arr.iter().enumerate() {
                        let uri = loc
                            .get("uri")
                            .or_else(|| loc.get("targetUri"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        let range = loc.get("range").or_else(|| loc.get("targetSelectionRange"));
                        let start_line = range
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("line"))
                            .and_then(|l| l.as_u64())
                            .unwrap_or(0)
                            + 1;
                        let start_col = range
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("character"))
                            .and_then(|c| c.as_u64())
                            .unwrap_or(0)
                            + 1;
                        if i > 0 {
                            out.push('\n');
                        }
                        out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
                        // Outside the checkout the file exists only on the gateway: include
                        // the lines around the definition so the agent can read it.
                        let path = crate::remote_fs::uri_to_path(uri);
                        if i < 3 && crate::remote_fs::is_external(workspace_root, &path) {
                            match crate::remote_fs::read_remote_file(remote, &path, 0).await {
                                Ok((bytes, _)) => {
                                    let text = String::from_utf8_lossy(&bytes);
                                    out.push('\n');
                                    out.push_str(&crate::remote_fs::snippet(
                                        &text,
                                        start_line as u32,
                                        8,
                                    ));
                                }
                                Err(e) => out
                                    .push_str(&format!("\n   (external source not readable: {e})")),
                            }
                        }
                    }
                }
            } else if let Some(obj) = res.as_object() {
                let uri = obj.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                let start_line = obj
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = obj
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
            } else {
                out.push_str("No definition found.");
            }

            Ok(McpToolCallResult::text(out))
        }

        "code_callers" | "code_callees" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;
            let incoming = tool_name == "code_callers";
            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();
            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
            });
            let items = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/prepareCallHierarchy",
                params,
            )
            .await?;
            let Some(item) = items.as_array().and_then(|a| a.first()).cloned() else {
                return Ok(McpToolCallResult::text(format!(
                    "No function at {path_str}:{line}:{character}."
                )));
            };
            let fn_name = item
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("?")
                .to_string();
            let method = if incoming {
                "callHierarchy/incomingCalls"
            } else {
                "callHierarchy/outgoingCalls"
            };
            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                method,
                serde_json::json!({ "item": item }),
            )
            .await?;
            let edges = res.as_array().cloned().unwrap_or_default();
            let side = if incoming { "from" } else { "to" };
            let mut out = format!(
                "`{fn_name}`: {} {}\n",
                edges.len(),
                if incoming { "caller(s)" } else { "callee(s)" }
            );
            for edge in &edges {
                let other = edge.get(side).cloned().unwrap_or_default();
                let other_name = other.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                let uri = other.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                let start = other.get("selectionRange").and_then(|r| r.get("start"));
                let dl = start
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let dc = start
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                let sites: Vec<String> = edge
                    .get("fromRanges")
                    .and_then(|r| r.as_array())
                    .map(|ranges| {
                        ranges
                            .iter()
                            .filter_map(|r| r.get("start"))
                            .map(|s| {
                                format!(
                                    "{}:{}",
                                    s.get("line").and_then(|l| l.as_u64()).unwrap_or(0) + 1,
                                    s.get("character").and_then(|c| c.as_u64()).unwrap_or(0) + 1
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                out.push_str(&format!(
                    "  • {other_name}  {uri}:{dl}:{dc}  [call sites: {}]\n",
                    sites.join(", ")
                ));
            }
            Ok(McpToolCallResult::text(out.trim_end().to_string()))
        }
        "code_implementations" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;
            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();
            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
            });
            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/implementation",
                params,
            )
            .await?;
            let arr = match &res {
                serde_json::Value::Array(a) => a.clone(),
                serde_json::Value::Object(_) => vec![res.clone()],
                _ => Vec::new(),
            };
            if arr.is_empty() {
                return Ok(McpToolCallResult::text(
                    "No implementations found.".to_string(),
                ));
            }
            let mut out = format!("Found {} implementation(s):\n", arr.len());
            for loc in &arr {
                let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                let start = loc.get("range").and_then(|r| r.get("start"));
                let l = start
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let c = start
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                out.push_str(&format!("  • {uri}:{l}:{c}\n"));
            }
            Ok(McpToolCallResult::text(out.trim_end().to_string()))
        }
        "code_impact" => {
            let base = args.get("base").and_then(|v| v.as_str());
            let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
            let report = crate::impact::analyze(remote, workspace_root, base, depth).await?;
            Ok(McpToolCallResult::text(report.render()))
        }
        "code_diagnose_failure" => {
            let filter = args.get("filter").and_then(|v| v.as_str());
            let timeout_secs = args
                .get("timeout_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let report =
                crate::dossier::diagnose(remote, workspace_root, filter, timeout_secs).await?;
            let text = report.render();
            Ok(
                if report.tests_failed == 0 && report.build_errors.is_empty() {
                    McpToolCallResult::text(text)
                } else {
                    McpToolCallResult::error(text)
                },
            )
        }
        "code_diagnostics" | "code_validate_edit" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let file_path = resolve_file_path(workspace_root, path_str);
            let report = match args.get("new_text").and_then(|v| v.as_str()) {
                Some(text) if tool_name == "code_validate_edit" => {
                    crate::diagnostics::validate_text(remote, workspace_root, &file_path, text)
                        .await?
                }
                _ if tool_name == "code_validate_edit" => {
                    return Ok(McpToolCallResult::error(
                        "Missing 'new_text' argument".to_string(),
                    ));
                }
                _ => crate::diagnostics::diagnostics(remote, workspace_root, &file_path).await?,
            };
            let text = report.render();
            Ok(if report.ok() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            })
        }
        "code_dead_code" => {
            let include_exported = args
                .get("include_exported")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let max_files = args
                .get("max_files")
                .and_then(|v| v.as_u64())
                .unwrap_or(400) as usize;
            let report = crate::dead_code::find_dead_code(
                remote,
                workspace_root,
                include_exported,
                max_files,
            )
            .await?;
            Ok(McpToolCallResult::text(report.render()))
        }
        "code_source" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let path = crate::remote_fs::uri_to_path(path);
            let line = args.get("line").and_then(|v| v.as_u64()).map(|l| l as u32);
            let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
            let (bytes, truncated) = crate::remote_fs::read_remote_file(remote, &path, 0).await?;
            let text = String::from_utf8_lossy(&bytes);
            let mut out = match line {
                Some(line) => crate::remote_fs::snippet(&text, line, context),
                None => text.into_owned(),
            };
            if truncated {
                out.push_str("\n[truncated at 2 MiB]");
            }
            Ok(McpToolCallResult::text(out))
        }
        "code_references" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;
            let include_decl = args
                .get("include_declarations")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
                "context": { "includeDeclaration": include_decl }
            });

            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/references",
                params,
            )
            .await?;

            let mut out = String::new();
            if let Some(arr) = res.as_array() {
                if arr.is_empty() {
                    out.push_str("No references found.");
                } else {
                    out.push_str(&format!("Found {} reference(s):\n", arr.len()));
                    for loc in arr {
                        let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                        let start_line = loc
                            .get("range")
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("line"))
                            .and_then(|l| l.as_u64())
                            .unwrap_or(0)
                            + 1;
                        let start_col = loc
                            .get("range")
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("character"))
                            .and_then(|c| c.as_u64())
                            .unwrap_or(0)
                            + 1;
                        out.push_str(&format!("  • {uri}:{start_line}:{start_col}\n"));
                    }
                }
            } else {
                out.push_str("No references found.");
            }

            Ok(McpToolCallResult::text(out.trim_end()))
        }

        "code_outline" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri }
            });

            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/documentSymbol",
                params,
            )
            .await?;

            let mut out = String::new();
            if let Some(arr) = res.as_array() {
                out.push_str(&format!("Outline for {path_str}:\n"));
                for sym in arr {
                    let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
                    let kind_str = match kind {
                        2 => "Module",
                        5 => "Class",
                        6 => "Method",
                        8 => "Field",
                        9 => "Constructor",
                        10 => "Enum",
                        11 => "Interface",
                        12 => "Function",
                        13 => "Variable",
                        14 => "Constant",
                        22 => "EnumMember",
                        23 => "Struct",
                        _ => "Symbol",
                    };
                    let line = sym
                        .get("range")
                        .or_else(|| sym.get("location").and_then(|l| l.get("range")))
                        .and_then(|r| r.get("start"))
                        .and_then(|s| s.get("line"))
                        .and_then(|l| l.as_u64())
                        .unwrap_or(0)
                        + 1;
                    out.push_str(&format!("  [{kind_str}] {name} (line {line})\n"));
                }
            } else {
                out.push_str("No outline symbols available.");
            }

            Ok(McpToolCallResult::text(out.trim_end()))
        }

        "code_hover" | "code_type_at" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;

            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
            });

            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/hover",
                params,
            )
            .await?;

            if let Some(contents) = res.get("contents") {
                if let Some(val) = contents.get("value").and_then(|v| v.as_str()) {
                    return Ok(McpToolCallResult::text(val));
                } else if let Some(arr) = contents.as_array() {
                    let text = arr
                        .iter()
                        .filter_map(|i| i.get("value").and_then(|v| v.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    return Ok(McpToolCallResult::text(text));
                }
            }

            Ok(McpToolCallResult::text("No hover information available."))
        }

        "code_status" => {
            let start = std::time::Instant::now();
            let stream = TcpStream::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to gateway at {remote}"))?;
            let rtt = start.elapsed();

            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            framed.send(WireMessage::StatusRequest).await?;

            if let Some(msg_res) = framed.next().await {
                match msg_res? {
                    WireMessage::StatusResponse(resp) => {
                        let hours = resp.uptime_seconds / 3600;
                        let minutes = (resp.uptime_seconds % 3600) / 60;
                        let seconds = resp.uptime_seconds % 60;
                        let mem = resp.memory_rss_mb().unwrap_or(0.0);

                        let info = format!(
                            "⚡ prod-code Gateway Status\n\
                             • Address: {remote} ({rtt:.2?} RTT)\n\
                             • Server PID: {}\n\
                             • Uptime: {hours}h {minutes}m {seconds}s\n\
                             • Memory RSS: {mem:.2} MB\n\
                             • Active Sessions: {}\n\
                             • Loaded Workspaces: {}\n\
                             • Queries Handled: {} (in-flight: {})\n\
                             • Engines: {}\n\
                             • Status: HEALTHY",
                            resp.server_pid,
                            resp.active_sessions,
                            resp.loaded_workspaces,
                            resp.total_queries,
                            resp.active_queries,
                            resp.detected_engines.join(", ")
                        );
                        Ok(McpToolCallResult::text(info))
                    }
                    other => Ok(McpToolCallResult::error(format!(
                        "Unexpected response from gateway: {other:?}"
                    ))),
                }
            } else {
                Ok(McpToolCallResult::error(
                    "Gateway closed connection without status response",
                ))
            }
        }

        "code_sync" => {
            let subpath = args.get("path").and_then(|v| v.as_str()).map(Path::new);
            let deltas = scan_workspace_files(workspace_root, subpath)?;
            let file_count = deltas.len();

            let stream = TcpStream::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to gateway at {remote}"))?;
            let mut framed = Framed::new(stream, ProdCodeCodec::new());

            let req = SyncRequest {
                client_workspace_root: workspace_root.to_string_lossy().to_string(),
                files: deltas,
                clean_others: false,
                base_workspace_name: None,
            };

            framed.send(WireMessage::SyncRequest(req)).await?;

            if let Some(msg_res) = framed.next().await {
                match msg_res? {
                    WireMessage::SyncResponse(resp) => {
                        let kb = (resp.bytes_transferred as f64) / 1024.0;
                        let info = format!(
                            "⚡ Fast-Sync Completed in {}ms\n\
                             • Files scanned: {file_count}\n\
                             • Files updated: {}\n\
                             • Files deleted: {}\n\
                             • Data transferred: {kb:.1} KB\n\
                             • Remote workspace: {}",
                            resp.duration_ms,
                            resp.files_updated,
                            resp.files_deleted,
                            resp.server_workspace_root
                        );
                        Ok(McpToolCallResult::text(info))
                    }
                    other => Ok(McpToolCallResult::error(format!(
                        "Unexpected response: {other:?}"
                    ))),
                }
            } else {
                Ok(McpToolCallResult::error(
                    "Gateway closed connection without sync response",
                ))
            }
        }

        unknown => Ok(McpToolCallResult::error(format!("Unknown tool: {unknown}"))),
    }
}

fn resolve_file_path(workspace_root: &Path, path_str: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(path_str);
    if p.is_absolute() {
        p
    } else {
        workspace_root.join(p)
    }
}

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
pub async fn execute_lsp_query(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    // One long-lived session per checkout for the life of this process (see
    // crate::session::pooled_query): local edits are pushed before the query.
    crate::session::pooled_query(remote, workspace_root, file_path, method, params).await
}
