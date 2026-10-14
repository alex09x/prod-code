/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Finding and filtering references across projects and multi-target call sites.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};
use url::Url;

use super::{build_swift_index, execute_lsp_query, resolve_file_path};
use crate::protocol::McpToolCallResult;

pub(crate) mod across;
pub(crate) mod multi_target;

pub(crate) use across::*;
pub(crate) use multi_target::*;

pub(crate) async fn handle_references(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
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
    // A position on no name has no references: "No references found" read as "nothing uses
    // this" when it meant "you pointed at nothing" (#373).
    let line_text = position_line(remote, workspace_root, &file_path, line).await;
    if let Some(text) = &line_text
        && name_at(text, character).is_none()
    {
        anyhow::bail!(
            "{path_str}:{line}:{character} is on no name; the line reads `{}`. Give the position of \
             a use or a declaration, or pass `symbol`",
            text.trim()
        );
    }
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        "context": { "includeDeclaration": include_decl }
    });
    let mut res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/references",
        params.clone(),
    )
    .await?;
    let mut out = String::new();

    let mut all_locs = Vec::new();
    let mut seen_locs = HashSet::new();
    let mut seen_files = HashSet::new();
    let mut unindexed_callers = Vec::new();

    add_locations(
        workspace_root,
        &res,
        &mut seen_locs,
        &mut seen_files,
        &mut all_locs,
    );
    let initial_was_empty = all_locs.is_empty();
    if !initial_was_empty {
        seen_files.insert(std::fs::canonicalize(&file_path).unwrap_or_else(|_| file_path.clone()));
    }

    let name_opt = line_text.as_deref().and_then(|t| name_at(t, character));

    if let Some(ref name) = name_opt
        && let Some(note) = collect_multi_target_references(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            name,
            include_decl,
            initial_was_empty,
            &mut seen_locs,
            &mut seen_files,
            &mut all_locs,
            &mut unindexed_callers,
        )
        .await
    {
        out.push_str(&note);
    }

    let build_index = args
        .get("build_index")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if all_locs.is_empty()
        && let Some((built, note)) =
            build_swift_index(remote, workspace_root, &file_path, build_index).await
    {
        out.push_str(&note);
        out.push('\n');
        if built {
            res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/references",
                params,
            )
            .await?;
            add_locations(
                workspace_root,
                &res,
                &mut seen_locs,
                &mut seen_files,
                &mut all_locs,
            );
        }
    }

    // An empty answer says what the position stood on, so a position one line off (an
    // attribute above the function) shows as such (#373).
    let stood_on = line_text
        .as_deref()
        .and_then(|text| Some((name_at(text, character)?, text.trim())))
        .map(|(name, text)| format!("\n(the position is on `{name}`; the line reads `{text}`)"))
        .unwrap_or_default();

    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(0);

    if all_locs.is_empty() {
        out.push_str("No references found.");
        out.push_str(&stood_on);
    } else {
        let total = all_locs.len();
        out.push_str(&format!("Found {total} reference(s):\n"));
        let display_count = if limit > 0 { total.min(limit) } else { total };
        for loc in &all_locs[..display_count] {
            let uri = loc
                .get("uri")
                .or_else(|| loc.get("targetUri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let start_line = loc
                .pointer("/range/start/line")
                .or_else(|| loc.pointer("/targetRange/start/line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0)
                + 1;
            let start_col = loc
                .pointer("/range/start/character")
                .or_else(|| loc.pointer("/targetRange/start/character"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0)
                + 1;
            out.push_str(&format!("  • {uri}:{start_line}:{start_col}\n"));
        }
        if limit > 0 && total > limit {
            out.push_str(&format!(
                "  ... (display capped at first {display_count} of {total} references; pass `limit: 0` to display all)\n"
            ));
        }

        if total > 1 {
            let sym_name = args
                .get("symbol")
                .and_then(|v| v.as_str())
                .or(name_opt.as_deref());
            let sym_desc = match sym_name {
                Some(s) => format!("`{s}`"),
                None => "this symbol".to_string(),
            };
            out.push_str(&format!(
                "\n💡 Refactoring Tip: For workspace-wide renames or signature updates to {sym_desc}, prefer `code_rename` or `code_change_signature`. If the specialized tool reports unsupported or blocked, fallback to manual edits and inspect diffs carefully.\n"
            ));
        }
    }

    unindexed_callers.retain(|p| {
        let norm = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        !seen_files.contains(&norm)
    });
    unindexed_callers.sort();
    unindexed_callers.dedup();
    if !unindexed_callers.is_empty() {
        let list = unindexed_callers
            .iter()
            .map(|p| {
                p.strip_prefix(workspace_root)
                    .unwrap_or(p)
                    .display()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "\n(warning: references may have incomplete coverage across configured targets: unindexed call site(s) found in {list})"
        ));
    }

    Ok(McpToolCallResult::text(out.trim_end()))
}

#[cfg(test)]
mod tests {
    use super::across::imports_declaration;
    use std::path::Path;

    #[test]
    fn imports_declaration_matches_multiline_astro_and_rejects_same_identifier_from_other_module() {
        let astro_text = "---\nimport {\n  type CaseStudy,\n  caseStudies,\n} from '../data/caseStudies';\n---\n<div>{caseStudies.length}</div>\n";
        assert!(imports_declaration(
            astro_text,
            Path::new("/ws/src/data/caseStudies.ts"),
            "caseStudies"
        ));
        assert!(imports_declaration(
            astro_text,
            Path::new("/ws/src/data/caseStudies/index.ts"),
            "CaseStudy"
        ));
        assert!(!imports_declaration(
            astro_text,
            Path::new("/ws/src/data/caseStudies.ts"),
            "unrelatedSymbol"
        ));
        let other_module = "import { caseStudies } from '../otherData';\n";
        assert!(!imports_declaration(
            other_module,
            Path::new("/ws/src/data/caseStudies/index.ts"),
            "caseStudies"
        ));
        assert!(!imports_declaration(
            other_module,
            Path::new("/ws/src/data/caseStudies.ts"),
            "caseStudies"
        ));
    }

    #[test]
    fn nested_git_worktree_is_excluded_from_source_files_and_worktree_check() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".git"), "gitdir: /tmp/main\n").unwrap();
        let src_dir = root.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let main_file = src_dir.join("app.ts");
        std::fs::write(&main_file, "export const x = 1;\n").unwrap();

        let nested_wt = root.join(".prod/worktrees/wt-123");
        std::fs::create_dir_all(nested_wt.join("src")).unwrap();
        std::fs::write(nested_wt.join(".git"), "gitdir: /tmp/wt-123\n").unwrap();
        let wt_file = nested_wt.join("src/app.ts");
        std::fs::write(&wt_file, "export const x = 1;\n").unwrap();

        assert!(!crate::tools::symbols::is_in_nested_git_worktree(
            root, &main_file
        ));
        assert!(crate::tools::symbols::is_in_nested_git_worktree(
            root, &wt_file
        ));

        let files: Vec<_> = crate::tools::symbols::source_files(root).collect();
        assert!(files.contains(&main_file));
        assert!(!files.contains(&wt_file));
    }

    #[test]
    fn resolve_file_path_preserves_external_toolchain_and_maps_remote_mirror() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("my-repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

        let external =
            "/opt/rustup/toolchains/stable/lib/rustlib/src/rust/library/core/src/error.rs";
        assert_eq!(
            crate::tools::compile_gate::resolve_file_path(&root, external),
            std::path::PathBuf::from(external)
        );

        let mirror = "/srv/prod-code/workspaces/my-repo/src/lib.rs";
        assert_eq!(
            crate::tools::compile_gate::resolve_file_path(&root, mirror),
            root.join("src/lib.rs")
        );
    }

    #[test]
    fn nested_swift_project_and_swift_declarations_work_at_deep_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"root\"\n").unwrap();
        let swift_pkg = root.join("apps/macos/Sources/AppPkg");
        let deep_dir = swift_pkg.join("Features/Terminal/Views/Components/Rows");
        std::fs::create_dir_all(&deep_dir).unwrap();
        std::fs::write(
            swift_pkg.join("Package.swift"),
            "// swift-tools-version: 6.0\n",
        )
        .unwrap();
        let swift_file = deep_dir.join("GhosttyTerminalNSView.swift");
        std::fs::write(
            &swift_file,
            "import AppKit\nfinal class GhosttyTerminalNSView: NSView {}\nextension GhosttyTerminalNSView {}\n",
        )
        .unwrap();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let nested =
            crate::tools::symbols::projects_naming(root, "GhosttyTerminalNSView", deadline);
        assert!(nested.iter().any(|(rep, subpath, engine)| {
            rep == &swift_file && subpath == "apps/macos/Sources/AppPkg" && *engine == "swift"
        }));
        assert!(
            crate::tools::symbols::declared_at(
                "final class GhosttyTerminalNSView: NSView {",
                "GhosttyTerminalNSView"
            )
            .is_some()
        );
        assert!(
            crate::tools::symbols::declared_at(
                "extension GhosttyTerminalNSView {",
                "GhosttyTerminalNSView"
            )
            .is_some()
        );
    }
}
