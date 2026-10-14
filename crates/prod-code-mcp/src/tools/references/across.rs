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

pub(crate) use super::lexical::{ImportMatch, imports_declaration, is_occurrence_shadowed};

fn apply_ts_mask(slice: &str, offset: usize, mask: &mut [bool]) {
    let sub = crate::caller_migration::mask::lexical_code_mask(
        slice,
        crate::parameter_object::Language::TypeScript,
    );
    mask[offset..offset + sub.len()].copy_from_slice(&sub);
}

fn scan_template_expr(text: &str, open_brace: usize, mask: &mut [bool]) -> usize {
    crate::caller_migration::mask::scan_ts_template_expr(text, open_brace, mask)
}

fn skip_quoted_attr(text: &str, quote_idx: usize, is_code_expr: bool, mask: &mut [bool]) -> usize {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let q = bytes[quote_idx];
    let val_start = quote_idx + 1;
    let mut i = val_start;
    while i < len && bytes[i] != q {
        i = if bytes[i] == b'\\' {
            (i + 2).min(len)
        } else {
            i + 1
        };
    }
    if is_code_expr && val_start < i {
        apply_ts_mask(&text[val_start..i], val_start, mask);
    }
    (i + 1).min(len)
}

pub(crate) fn mask_executable_code(text: &str, path: &Path) -> Vec<bool> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = text.as_bytes();
    let len = bytes.len();

    if !matches!(ext.as_str(), "astro" | "svelte" | "vue" | "html") {
        let lang = crate::parameter_object::Language::of(path)
            .unwrap_or(crate::parameter_object::Language::TypeScript);
        return crate::caller_migration::mask::lexical_code_mask(text, lang);
    }

    let mut mask = vec![false; len];
    let mut i = 0;
    if ext == "astro" {
        let trimmed = text.trim_start_matches([' ', '\t', '\r', '\n', '\u{feff}']);
        if let Some(after) = trimmed.strip_prefix("---")
            && (after.starts_with('\n') || after.starts_with("\r\n"))
        {
            let fm_start = len - after.len();
            let full_ts = crate::caller_migration::mask::lexical_code_mask(
                &text[fm_start..],
                crate::parameter_object::Language::TypeScript,
            );
            let mut rel = 0;
            while rel < full_ts.len() {
                if (rel == 0 || bytes[fm_start + rel - 1] == b'\n') && full_ts[rel] {
                    let line_rest = &text[fm_start + rel..];
                    let ts_trim = line_rest.trim_start_matches([' ', '\t']);
                    if let Some(after_fence) = ts_trim.strip_prefix("---")
                        && (after_fence.is_empty()
                            || after_fence.starts_with('\n')
                            || after_fence.starts_with("\r\n"))
                    {
                        mask[fm_start..fm_start + rel].copy_from_slice(&full_ts[..rel]);
                        i = fm_start + (line_rest.len() - after_fence.len());
                        break;
                    }
                }
                rel += 1;
            }
        }
    }
    while i < len {
        if bytes[i..].starts_with(b"<!--") {
            i = text[i + 4..].find("-->").map_or(len, |rel| i + 4 + rel + 3);
            continue;
        }
        if bytes[i] == b'{' {
            i = scan_template_expr(text, i, &mut mask);
            continue;
        }
        if bytes[i] == b'<' {
            let rest_lower = text[i + 1..].to_ascii_lowercase();
            if rest_lower.starts_with("script")
                && rest_lower
                    .as_bytes()
                    .get(6)
                    .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>' || *b == b'/')
                && let Some(tag_close) = text[i..].find('>')
            {
                let body_start = i + tag_close + 1;
                if !text[i..body_start].ends_with("/>")
                    && let Some(end_rel) = text[body_start..].to_ascii_lowercase().find("</script")
                {
                    let body_end = body_start + end_rel;
                    apply_ts_mask(&text[body_start..body_end], body_start, &mut mask);
                    i = text[body_end..].find('>').map_or(len, |c| body_end + c + 1);
                    continue;
                }
            }
            if rest_lower.starts_with("style")
                && rest_lower
                    .as_bytes()
                    .get(5)
                    .is_some_and(|b| b.is_ascii_whitespace() || *b == b'>')
            {
                i = rest_lower
                    .find("</style>")
                    .map_or(len, |rel| i + 1 + rel + 8);
                continue;
            }
            let is_closing_tag = i + 1 < len && bytes[i + 1] == b'/';
            let mut t = i + 1 + usize::from(is_closing_tag);
            let name_start = t;
            while t < len
                && (bytes[t].is_ascii_alphanumeric() || matches!(bytes[t], b'_' | b'.' | b'-'))
            {
                t += 1;
            }
            if t > name_start && bytes[name_start].is_ascii_uppercase() {
                mask[name_start..t].fill(true);
            }
            i = t;
            while i < len && bytes[i] != b'>' {
                if bytes[i].is_ascii_whitespace() || bytes[i] == b'/' {
                    i += 1;
                } else if bytes[i] == b'{' {
                    i = scan_template_expr(text, i, &mut mask);
                } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                    i = skip_quoted_attr(text, i, false, &mut mask);
                } else {
                    let attr_start = i;
                    while i < len
                        && !bytes[i].is_ascii_whitespace()
                        && !matches!(bytes[i], b'=' | b'>' | b'/' | b'{' | b'"' | b'\'')
                    {
                        i += 1;
                    }
                    let attr_name = &text[attr_start..i];
                    let is_vue_expr = ext == "vue"
                        && !is_closing_tag
                        && (attr_name.starts_with("v-")
                            || attr_name.starts_with(':')
                            || attr_name.starts_with('@')
                            || attr_name.starts_with('#'));
                    if is_vue_expr
                        && let (Some(b_open), Some(b_close)) =
                            (attr_name.find('['), attr_name.rfind(']'))
                        && b_open + 1 < b_close
                    {
                        apply_ts_mask(
                            &attr_name[b_open + 1..b_close],
                            attr_start + b_open + 1,
                            &mut mask,
                        );
                    }
                    while i < len && bytes[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if i < len && bytes[i] == b'=' {
                        i += 1;
                        while i < len && bytes[i].is_ascii_whitespace() {
                            i += 1;
                        }
                        if i < len && (bytes[i] == b'"' || bytes[i] == b'\'') {
                            i = skip_quoted_attr(text, i, is_vue_expr, &mut mask);
                        } else if i < len && bytes[i] == b'{' {
                            i = scan_template_expr(text, i, &mut mask);
                        }
                    }
                }
            }
            i = (i + 1).min(len);
            continue;
        }
        i += 1;
    }
    mask
}

pub(crate) fn masked_code_string(text: &str, mask: &[bool]) -> String {
    let mut out = String::with_capacity(text.len());
    for (idx, ch) in text.char_indices() {
        if ch == '\n' || ch == '\r' || mask.get(idx).copied().unwrap_or(false) {
            out.push(ch);
        } else {
            for _ in 0..ch.len_utf8() {
                out.push(' ');
            }
        }
    }
    out
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
