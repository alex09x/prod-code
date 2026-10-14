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

fn specifier_matches_decl(spec: &str, decl_segs: &[&str], is_index: bool) -> bool {
    let raw = spec
        .trim()
        .trim_end_matches(';')
        .trim_end_matches(')')
        .trim();
    let unquoted = if let Some((_, rest)) = raw.split_once(['\'', '"', '<']) {
        rest.split(['\'', '"', '>']).next().unwrap_or(raw)
    } else {
        raw
    };
    let mut path_str = unquoted.trim();
    for ext in [
        ".tsx", ".ts", ".jsx", ".js", ".mjs", ".cjs", ".astro", ".svelte", ".vue", ".py", ".rs",
        ".hpp", ".h",
    ] {
        if let Some(stripped) = path_str.strip_suffix(ext) {
            path_str = stripped;
            break;
        }
    }
    let spec_segs: Vec<&str> = path_str
        .split(['/', '.', ':'])
        .filter(|s| {
            !s.is_empty() && !matches!(*s, "." | ".." | "@" | "~" | "crate" | "super" | "self")
        })
        .collect();
    if spec_segs.is_empty() {
        return false;
    }
    if is_index && spec_segs.len() == 1 && matches!(spec_segs[0], "index" | "mod" | "__init__") {
        return false;
    }
    decl_segs.ends_with(&spec_segs)
        || (is_index
            && decl_segs.len() >= 2
            && decl_segs[..decl_segs.len() - 1].ends_with(&spec_segs))
}

pub(crate) fn imports_declaration(text: &str, decl_path: &Path, name: &str) -> bool {
    let file_stem = decl_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if file_stem.is_empty() {
        return false;
    }
    let is_index = matches!(file_stem, "index" | "mod" | "__init__");
    let without_ext = decl_path.with_extension("");
    let decl_segs: Vec<&str> = without_ext
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("#include ")
            || trimmed.starts_with("#include<")
            || trimmed.starts_with("#include\"")
        {
            let spec = trimmed.trim_start_matches("#include").trim();
            if specifier_matches_decl(spec, &decl_segs, is_index) {
                return true;
            }
            i += 1;
            continue;
        }
        if trimmed.starts_with("use ") || trimmed.starts_with("pub use ") {
            let mut stmt = String::from(trimmed);
            while !stmt.contains(';') && i + 1 < lines.len() {
                i += 1;
                stmt.push(' ');
                stmt.push_str(lines[i].trim());
            }
            let target_stem = if is_index && decl_segs.len() >= 2 {
                decl_segs[decl_segs.len() - 2]
            } else {
                file_stem
            };
            if (crate::tools::symbols::names_word(&stmt, name) || stmt.contains('*'))
                && crate::tools::symbols::names_word(&stmt, target_stem)
            {
                return true;
            }
            i += 1;
            continue;
        }
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
            let matched = if stmt.starts_with("from ") {
                if let Some((spec_part, binding_part)) =
                    stmt.trim_start_matches("from ").split_once(" import ")
                {
                    (crate::tools::symbols::names_word(binding_part, name)
                        || binding_part.contains('*'))
                        && specifier_matches_decl(spec_part, &decl_segs, is_index)
                } else {
                    false
                }
            } else if let Some((binding_part, spec_part)) = stmt
                .rsplit_once(" from ")
                .or_else(|| stmt.rsplit_once("}from "))
                .or_else(|| stmt.rsplit_once("}from'"))
                .or_else(|| stmt.rsplit_once("}from\""))
            {
                (crate::tools::symbols::names_word(binding_part, name)
                    || binding_part.contains('*'))
                    && specifier_matches_decl(spec_part, &decl_segs, is_index)
            } else {
                let spec_part = stmt.trim_start_matches("import").trim();
                specifier_matches_decl(spec_part, &decl_segs, is_index)
            };
            if matched {
                return true;
            }
            i = j + 1;
            continue;
        }
        for call in ["require(", "import("] {
            if let Some((_, after)) = trimmed.split_once(call)
                && specifier_matches_decl(after, &decl_segs, is_index)
            {
                return true;
            }
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
