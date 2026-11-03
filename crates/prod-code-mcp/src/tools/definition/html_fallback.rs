/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Confined, bounded source navigation when the optional HTML server is missing. This is
//! a syntax-only local function lookup, never semantic cross-file resolution or a write.

use anyhow::Result;
use oxc_allocator::Allocator;
use oxc_ast::ast::Statement;
use oxc_parser::Parser;
use oxc_span::SourceType;
use serde_json::{Value, json};
use std::future::Future;
use std::io::Read;
use std::path::Path;

use crate::protocol::McpToolCallResult;
use crate::tools::references::{mask_executable_code, masked_code_string};
use crate::tools::symbols::declared_at;

const MAX_CANDIDATES: usize = 8;
const MAX_BODY_LINES: usize = 300;
const MAX_BODY_BYTES: usize = 24 * 1024;

fn path_argument(args: &Value) -> Option<&str> {
    ["path", "file_path", "file"]
        .iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str))
}

pub(crate) fn request(tool: &str, args: &Value) -> Option<Value> {
    if tool != "code_definition" || args.get("symbol").and_then(Value::as_str).is_none() {
        return None;
    }
    let raw = crate::remote_fs::uri_to_path(path_argument(args)?);
    matches!(
        Path::new(&raw)
            .extension()?
            .to_str()?
            .to_ascii_lowercase()
            .as_str(),
        "html" | "htm"
    )
    .then(|| args.clone())
}

fn missing_html_server(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_lowercase();
    ![
        "tls",
        "certificate",
        "authentication",
        "pin confirmation",
        "pinned target",
    ]
    .iter()
    .any(|word| message.contains(word))
        && ((message.contains("html language server did not start")
            && (message.contains("no such file or directory") || message.contains("os error 2")))
            || message.contains("no reachable gateway serves html"))
}

/// Keep fallback and its read-lock wait inside the original caller's navigation deadline.
pub(crate) async fn with_fallback<F>(
    configured: &Path,
    args: Option<Value>,
    run: F,
) -> Result<McpToolCallResult>
where
    F: Future<Output = Result<McpToolCallResult>>,
{
    let error = match run.await {
        Ok(reply) => return Ok(reply),
        Err(error) => error,
    };
    let Some(mut args) = args.filter(|_| missing_html_server(&error)) else {
        return Err(error);
    };
    let root = crate::tools::resolve_workspace_root(configured, "code_definition", &mut args)?;
    let _read = crate::tools::workspace_writer::workspace_mutation_lock(&root)
        .read_owned()
        .await;
    match local_definition(&root, &args)? {
        Some(reply) => Ok(reply),
        None => Err(error),
    }
}

fn local_definition(root: &Path, args: &Value) -> Result<Option<McpToolCallResult>> {
    let name = args["symbol"]
        .as_str()
        .unwrap_or("")
        .trim()
        .trim_end_matches("()");
    if name.is_empty() || name.len() > 256 || name.contains(['.', ':', '/', '#']) {
        return Ok(None);
    }
    let Some(raw) = path_argument(args) else {
        return Ok(None);
    };
    let Some(path) = crate::remote_fs::checkout_source_path(root, raw)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    std::fs::File::open(&path)?
        .take(crate::remote_fs::MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > crate::remote_fs::MAX_SOURCE_BYTES {
        return Ok(None);
    }
    let text = String::from_utf8(bytes)?;
    // The existing markup-aware mask excludes HTML comments, script strings and prose.
    let masked = masked_code_string(
        &text,
        &mask_executable_code(&text, Path::new("inline.html")),
    );
    let declarations = declarations(&masked, name);
    let mut locations = Vec::new();
    for (line, column) in declarations.into_iter().take(MAX_CANDIDATES + 1) {
        let line_start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        let Some(original) = text.lines().nth(line) else {
            continue;
        };
        let byte_column = column;
        let identifier = line_start + byte_column;
        let Some(end) = function_end(&text, &masked, line_start, identifier, name) else {
            continue;
        };
        let utf16_column = original[..byte_column].encode_utf16().count() + 1;
        locations.push((line, utf16_column, end));
    }
    if locations.is_empty() {
        return Ok(None);
    }
    let uri =
        url::Url::from_file_path(&path).map_err(|_| anyhow::anyhow!("invalid local HTML path"))?;
    if locations.len() > 1 {
        let choices = locations
            .iter()
            .take(MAX_CANDIDATES)
            .map(|(line, column, _)| format!("  {uri}:{}:{column}", line + 1))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(Some(McpToolCallResult::error(format!(
            "ambiguous syntax-only HTML declarations for `{name}`; no semantic candidate was chosen:\n{choices}\nRead the selected source window with code_source."
        ))));
    }
    let (line, column, end) = locations[0];
    let last = text[..end].bytes().filter(|b| *b == b'\n').count();
    let mut out = format!(
        "HTML syntax-only fallback: local function declaration; the HTML language server is unavailable. This is not semantic cross-file navigation.\n📍 Definition: {uri}:{}:{column}\n",
        line + 1
    );
    let mut truncated = false;
    if args.get("body").and_then(Value::as_bool).unwrap_or(true) {
        for (index, source) in text
            .lines()
            .enumerate()
            .skip(line)
            .take((last - line + 1).min(MAX_BODY_LINES))
        {
            out.push_str(&format!("{:>4} | {source}\n", index + 1));
        }
        truncated = last - line + 1 > MAX_BODY_LINES || out.len() > MAX_BODY_BYTES;
        if out.len() > MAX_BODY_BYTES {
            out = crate::verify::truncate_to_boundary(&out, MAX_BODY_BYTES).to_string();
        }
        if truncated {
            out.push_str("\n[local declaration output truncated]\n");
        }
    }
    let mut result = McpToolCallResult::text(out);
    result.structured_content = Some(json!({"provenance":"syntax_only_html", "semantic":false,
        "truncated":truncated,"locations":[{"uri":uri.as_str(),"line":line+1,"character":column}]}));
    Ok(Some(result))
}

/// Reuse the declaration recognizer, including several declarations on a minified line.
fn declarations(masked: &str, name: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    for (line_index, line) in masked.lines().enumerate() {
        let mut consumed = 0;
        while let Some(column) = declared_at(&line[consumed..], name) {
            let Some((offset, _)) = line[consumed..].char_indices().nth(column) else {
                break;
            };
            let at = consumed + offset;
            found.push((line_index, at));
            if found.len() > MAX_CANDIDATES {
                return found;
            }
            consumed = at + name.len();
        }
    }
    found
}

/// Parse the declaration using the existing JavaScript/TypeScript parser rather than counting
/// braces in strings, regex literals or default parameter expressions.
fn function_end(
    text: &str,
    masked: &str,
    line_start: usize,
    identifier: usize,
    name: &str,
) -> Option<usize> {
    let prefix = &masked[line_start..identifier];
    let keyword = prefix.rfind("function")?;
    if !prefix[keyword + 8..].chars().all(char::is_whitespace) {
        return None;
    }
    let before = prefix[..keyword].trim_end();
    let start = line_start
        + if before.ends_with("async") {
            before.len() - 5
        } else {
            keyword
        };
    let close = text[start..].to_ascii_lowercase().find("</script")? + start;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &text[start..close], SourceType::ts()).parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        return None;
    }
    let Statement::FunctionDeclaration(function) = parsed.program.body.first()? else {
        return None;
    };
    if function.id.as_ref()?.name.as_str() != name || function.body.is_none() {
        return None;
    }
    Some(start + function.span.end as usize)
}
