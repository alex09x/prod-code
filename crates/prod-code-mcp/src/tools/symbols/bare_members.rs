/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A member looked for by its bare name, in the outlines of the files that declare it.

use super::across_projects::names_word;
use super::declared_at::declared_at;
use super::matching::{bare_symbol_name, owner_segments};
use super::nested_projects::{MAX_SCANNED_FILES, source_files};
use super::type_members::member_at;
use super::types::{SymbolHit, symbol_kind_name};
use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;
use url::Url;

pub(crate) const UNINDEXED_MEMBERS_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the checkout is read for the files to outline, of the budget above.
const FILE_SCAN_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

/// The most files outlined for one name.
const MAX_OUTLINED_FILES: usize = 8;

/// Where the checkout's source files declare `member` as a struct/class/interface field or
/// method when the language server's `workspace/symbol` index omitted it (e.g. rust-analyzer
/// does not index struct fields).
pub(crate) async fn unindexed_members(
    remote: SocketAddr,
    root: &Path,
    member: &str,
    hint: Option<&Path>,
) -> Result<Vec<SymbolHit>> {
    // No file declares a member whose name is no identifier (an empty query, `::`, `a b`):
    // looking for one only read the checkout's files until the scan's budget ran out.
    if member.is_empty() || !member.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Ok(Vec::new());
    }
    let deadline = tokio::time::Instant::now() + UNINDEXED_MEMBERS_BUDGET;
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    if let Some(h) = hint {
        let p = if h.is_absolute() {
            h.to_path_buf()
        } else {
            root.join(h)
        };
        if p.is_file() {
            files.push(p);
        } else if p.is_dir() {
            for entry in source_files(&p).take(MAX_OUTLINED_FILES) {
                if std::fs::read_to_string(&entry).is_ok_and(|text| names_word(&text, member)) {
                    files.push(entry);
                }
            }
        }
    }
    // A file that declares the name is outlined before one that only uses it: the first files
    // to mention a field used all over a checkout are its users, not its struct (#2023).
    let scan_deadline = deadline.min(tokio::time::Instant::now() + FILE_SCAN_BUDGET);
    let mut users: Vec<std::path::PathBuf> = Vec::new();
    for path in source_files(root)
        .filter(|p| crate::sync::engine_for_file(p).is_some())
        .take(MAX_SCANNED_FILES)
    {
        if tokio::time::Instant::now() >= scan_deadline || files.len() >= MAX_OUTLINED_FILES {
            break;
        }
        if files.contains(&path) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !names_word(&text, member) {
            continue;
        }
        if text.lines().any(|line| declared_at(line, member).is_some()) {
            files.push(path);
        } else if users.len() < MAX_OUTLINED_FILES {
            users.push(path);
        }
    }
    let room = MAX_OUTLINED_FILES.saturating_sub(files.len());
    files.extend(users.into_iter().take(room));

    let mut members: Vec<SymbolHit> = Vec::new();
    for file in &files {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let Ok(uri) = Url::from_file_path(file) else {
            continue;
        };
        let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
        let query_fut =
            execute_lsp_query(remote, root, file, "textDocument/documentSymbol", params);
        let Ok(Ok(outline)) = tokio::time::timeout_at(
            deadline.min(tokio::time::Instant::now() + std::time::Duration::from_secs(3)),
            query_fut,
        )
        .await
        else {
            continue;
        };
        collect_unqualified_members(&outline, file, member, &[], &mut members)?;
    }
    Ok(members)
}

pub(crate) fn collect_unqualified_members(
    symbols: &serde_json::Value,
    path: &Path,
    member: &str,
    ancestors: &[String],
    out: &mut Vec<SymbolHit>,
) -> Result<()> {
    let is_member = |sym: &serde_json::Value| {
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        bare_symbol_name(name).eq_ignore_ascii_case(member)
    };
    for sym in symbols.as_array().into_iter().flatten() {
        let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
        let mut declared = ancestors.to_vec();
        declared.extend(owner_segments(name));

        if is_member(sym) && (!ancestors.is_empty() || matches!(kind, 6..=9 | 22)) {
            let (m_name, m_kind, line, col) = member_at(sym)?;
            let container = ancestors.last().cloned().or_else(|| {
                sym.get("containerName")
                    .and_then(|c| c.as_str())
                    .map(str::to_string)
            });
            let hit = SymbolHit {
                path: path.to_path_buf(),
                name: m_name,
                kind: symbol_kind_name(m_kind),
                container,
                line,
                col,
            };
            if !out
                .iter()
                .any(|m| m.path == hit.path && m.line == hit.line && m.col == hit.col)
            {
                out.push(hit);
            }
        }

        if let Some(children) = sym.get("children") {
            let nested_ancestors = if matches!(kind, 2..=5 | 10 | 11 | 23) {
                declared
            } else {
                ancestors.to_vec()
            };
            collect_unqualified_members(children, path, member, &nested_ancestors, out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::protocol::McpContentItem;
    use prod_code_testkit::{ScriptedGateway, Workspace, answers};

    /// `code_symbols retained_flag` answered "No symbols match" for a struct field: the index
    /// of rust-analyzer lists no field, and only `Demo::retained_flag` looked in the outline.
    #[tokio::test]
    async fn a_bare_field_name_is_listed_under_its_struct() {
        let ws = Workspace::new(&[
            (
                "Cargo.toml",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            // Path order puts the user before the struct: the declaring file is outlined first.
            (
                "src/a_user.rs",
                "pub fn read(demo: &crate::model::Demo) -> bool { demo.retained_flag }\n",
            ),
            (
                "src/model.rs",
                "pub struct Demo {\n    pub retained_flag: bool,\n}\n\nimpl Demo {\n    pub fn clear(&mut self) {\n        let retained_flag = false;\n        self.retained_flag = retained_flag;\n    }\n}\n",
            ),
        ]);
        let gateway = ScriptedGateway::start(|method, params| {
            let model = params["textDocument"]["uri"]
                .as_str()
                .is_some_and(|uri| uri.ends_with("model.rs"));
            match method {
                "workspace/symbol" => serde_json::json!([]),
                "textDocument/documentSymbol" if model => serde_json::json!([
                    answers::nested(
                        answers::document_symbol("Demo", 23, 1, 3, 12),
                        vec![answers::document_symbol("retained_flag", 8, 2, 2, 9)],
                    ),
                    answers::nested(
                        answers::document_symbol("impl Demo", 5, 5, 10, 6),
                        vec![answers::nested(
                            answers::document_symbol("clear", 6, 6, 9, 12),
                            vec![answers::document_symbol("retained_flag", 13, 7, 7, 13)],
                        )],
                    ),
                ]),
                _ => serde_json::Value::Null,
            }
        })
        .await;

        let result = crate::tools::execute_tool(
            gateway.addr(),
            &ws.root(),
            "code_symbols",
            serde_json::json!({ "query": "retained_flag" }),
        )
        .await
        .unwrap();
        let McpContentItem::Text { text } = &result.content[0];
        assert!(
            text.contains("[Field] Demo::retained_flag — src/model.rs:2:9"),
            "{text}"
        );
        // The local of the same name in a method is no member of the type.
        assert!(!text.contains("model.rs:7"), "{text}");
        assert!(!text.contains("No symbols match"), "{text}");
    }
}
