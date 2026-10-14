/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};

use crate::protocol::McpToolCallResult;
use crate::tools::{execute_tool, resolve_file_path};

/// `code_references` in this checkout and in each directory of `also_in`, the name resolved in
/// each, every answer under its checkout's directory; a checkout that fails says why without
/// hiding the others (#375).
pub(crate) async fn references_across(
    remote: SocketAddr,
    workspace_root: &Path,
    args: serde_json::Value,
    dirs: &[serde_json::Value],
) -> Result<McpToolCallResult> {
    if args
        .get("symbol")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.trim().is_empty())
    {
        anyhow::bail!(
            "`also_in` goes with `symbol`: a position is a place in one checkout, a name is \
             resolved in each"
        );
    }
    let mut roots = vec![workspace_root.to_path_buf()];
    for dir in dirs {
        let dir = dir
            .as_str()
            .context("`also_in` lists the directories of other checkouts")?;
        let dir = resolve_file_path(workspace_root, dir);
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        if !roots.contains(&dir) {
            roots.push(dir);
        }
    }
    let mut out = String::new();
    let mut found = 0usize;
    for (index, root) in roots.iter().enumerate() {
        let mut asked = args.clone();
        if let Some(obj) = asked.as_object_mut() {
            obj.remove("also_in");
            // A file hint names a file of the first checkout.
            if index > 0 {
                obj.remove("path");
            }
        }
        // Each checkout is asked on the node its own workspace is placed on.
        let node = if index == 0 || !root.is_dir() {
            Ok(remote)
        } else {
            crate::cluster::route_for_checkout(remote, root).await
        };
        let text = match node {
            _ if !root.is_dir() => "not a directory".to_string(),
            Err(err) => format!("{err:#}"),
            Ok(node) => match Box::pin(execute_tool(node, root, "code_references", asked)).await {
                Ok(result) => result
                    .content
                    .iter()
                    .map(|item| {
                        let crate::protocol::McpContentItem::Text { text } = item;
                        text.as_str()
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(err) => format!("{err:#}"),
            },
        };
        found += text.lines().filter(|l| l.starts_with("  • ")).count();
        out.push_str(&format!(
            "== {} ==\n{}\n\n",
            root.display(),
            text.trim_end()
        ));
    }
    out.push_str(&format!(
        "{found} reference(s) in {} checkout(s)",
        roots.len()
    ));
    Ok(McpToolCallResult::text(out))
}

/// The text of 1-based `line` of `file`: read here, or from the node for a file only the node
/// has (a dependency's source). `None` when it cannot be read.
pub(crate) async fn position_line(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
) -> Option<String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(_) if crate::remote_fs::is_external(root, &file.to_string_lossy()) => {
            let (bytes, _) = crate::remote_fs::read_remote_file(remote, &file.to_string_lossy(), 0)
                .await
                .ok()?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(_) => return None,
    };
    text.lines()
        .nth((line as usize).checked_sub(1)?)
        .map(str::to_string)
}

/// The name a 1-based `character` of a line stands on, or just after (where an editor's cursor
/// sits at the end of a word). `None` for a position on no name.
pub(crate) fn name_at(line: &str, character: u32) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let is_name = |c: &char| c.is_alphanumeric() || *c == '_';
    let at = (character as usize).checked_sub(1)?;
    let at = if chars.get(at).is_some_and(is_name) {
        at
    } else if at > 0 && chars.get(at - 1).is_some_and(is_name) {
        at - 1
    } else {
        return None;
    };
    let start = chars[..at]
        .iter()
        .rposition(|c| !is_name(c))
        .map_or(0, |i| i + 1);
    let end = chars[at..]
        .iter()
        .position(|c| !is_name(c))
        .map_or(chars.len(), |i| at + i);
    Some(chars[start..end].iter().collect())
}

/// Whether `text` writes `name(` with `name` as a whole word, a call or a declaration.
pub(crate) fn writes_call(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(ident)
            && text[at + name.len()..].trim_start().starts_with('(')
    })
}

pub(crate) fn engine_can_reference(caller_engine: Option<&str>, decl_engine: &str) -> bool {
    match (caller_engine, decl_engine) {
        (Some(c), d) if c == d => true,
        (Some("astro" | "svelte" | "vue" | "html" | "javascript"), "typescript") => true,
        (Some("astro" | "svelte" | "vue" | "html" | "typescript"), "javascript") => true,
        (Some("cpp"), "c") | (Some("c"), "cpp") => true,
        _ => false,
    }
}

pub(crate) fn imports_declaration(text: &str, decl_path: &Path, name: &str) -> bool {
    let file_stem = decl_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut stems = Vec::new();
    if !file_stem.is_empty() {
        stems.push(file_stem);
    }
    if matches!(file_stem, "index" | "mod" | "__init__")
        && let Some(parent_stem) = decl_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
        && !parent_stem.is_empty()
    {
        stems.push(parent_stem);
    }
    let matches_stem = |s: &str| stems.is_empty() || stems.iter().any(|stem| s.contains(stem));
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("import ")
            || trimmed.starts_with("import{")
            || trimmed.starts_with("export ")
            || trimmed.starts_with("export{")
            || trimmed.starts_with("from ")
        {
            let mut stmt = String::from(trimmed);
            let mut j = i;
            let is_multiline = (trimmed.contains('{') && !trimmed.contains('}'))
                || (trimmed.starts_with("from ") && trimmed.ends_with('('));
            if is_multiline {
                while j + 1 < lines.len() && j - i < 25 {
                    j += 1;
                    let next = lines[j].trim();
                    stmt.push(' ');
                    stmt.push_str(next);
                    if next.contains(';')
                        || next.contains("from ")
                        || next.starts_with('}')
                        || next.ends_with(')')
                    {
                        break;
                    }
                }
            }
            if (crate::tools::symbols::names_word(&stmt, name) || stmt.contains('*'))
                && matches_stem(&stmt)
            {
                return true;
            }
            i = j + 1;
            continue;
        }
        if (trimmed.contains("require(") || trimmed.contains("import(")) && matches_stem(trimmed) {
            return true;
        }
        i += 1;
    }
    false
}

/// Whether a `textDocument/definition` answer (a location, a list of them, or of links) names
/// 0-based `line` of the file at `path`.
pub(crate) fn definition_is(found: &serde_json::Value, path: &str, line: u32) -> bool {
    let locations: Vec<&serde_json::Value> = match found {
        serde_json::Value::Array(all) => all.iter().collect(),
        serde_json::Value::Object(_) => vec![found],
        _ => Vec::new(),
    };
    locations.iter().any(|location| {
        let uri = location
            .get("uri")
            .or_else(|| location.get("targetUri"))
            .and_then(|u| u.as_str())
            .unwrap_or("");
        let start = location
            .pointer("/range/start/line")
            .or_else(|| location.pointer("/targetSelectionRange/start/line"))
            .and_then(|l| l.as_u64());
        crate::remote_fs::uri_to_path(uri) == path && start == Some(line as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn imports_declaration_matches_multiline_astro_and_index_imports() {
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
