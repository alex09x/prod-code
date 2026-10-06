/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Outline extraction, filtering and rendering.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};
use url::Url;

use super::{SKIPPED_DIRS, execute_lsp_query, resolve_file_path, source_files};
use crate::protocol::McpToolCallResult;

pub async fn handle_outline(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let file_path = resolve_file_path(workspace_root, path_str);
    let is_dir = file_path.is_dir();
    let limit = |key: &str| args.get(key).and_then(|v| v.as_u64()).map(|n| n as usize);
    let options = OutlineOptions {
        max_depth: args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(3)
            .max(1) as usize,
        include_locals: args
            .get("include_locals")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        hint: "pass include_locals: true".to_string(),
        kinds: args.get("kinds").and_then(|v| v.as_array()).map(|kinds| {
            kinds
                .iter()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect()
        }),
        exported_only: args
            .get("exported_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        max_bytes: match limit("max_bytes") {
            Some(0) => None,
            Some(bytes) => Some(bytes),
            None => is_dir.then_some(DIRECTORY_OUTLINE_BYTES),
        },
        max_items: limit("max_items").filter(|n| *n > 0),
    };
    let text = if is_dir {
        outline_directory(
            remote,
            workspace_root,
            &file_path,
            Path::new(path_str),
            &options,
        )
        .await?
    } else {
        outline_file(remote, workspace_root, &file_path, path_str, &options).await?
    };
    let has_symbols = text.lines().any(|l| l.trim_start().starts_with('['))
        || (is_dir && text.contains("subdirectories with sources:"));
    if !has_symbols {
        return Ok(McpToolCallResult::error(format!(
            "no outline symbols found for {path_str}"
        )));
    }
    Ok(McpToolCallResult::text(text))
}

/// What an outline lists, and how much of it (#368).
#[derive(Debug, Clone)]
pub struct OutlineOptions {
    /// The deepest nesting listed, 1 = top-level items only.
    pub max_depth: usize,
    /// Also the local variables inside function bodies.
    pub include_locals: bool,
    /// How to ask for the locals, for the line that says they were left out.
    pub hint: String,
    /// Only these kinds, as the outline names them (`function`, `struct`, ...).
    pub kinds: Option<Vec<String>>,
    /// Only what the language exports (see [`is_exported`]).
    pub exported_only: bool,
    /// The most bytes of outline listed. A directory's outline names the files after it
    /// instead of outlining them; a file's is cut after the symbols that fit.
    pub max_bytes: Option<usize>,
    /// The most symbols listed.
    pub max_items: Option<usize>,
}

impl OutlineOptions {
    /// Everything, to the depth given, with no budget.
    pub fn all(max_depth: usize, include_locals: bool, hint: &str) -> Self {
        Self {
            max_depth,
            include_locals,
            hint: hint.to_string(),
            kinds: None,
            exported_only: false,
            max_bytes: None,
            max_items: None,
        }
    }
}

/// The budget of a directory's outline when none is asked for: about ten thousand tokens. One
/// Go package's full outline was 79 KB and 1,667 symbols (#368).
pub const DIRECTORY_OUTLINE_BYTES: usize = 40_000;
/// A file's outline, for the MCP tool and the CLI alike (#362): a Markdown file's headings,
/// read here since no language server serves Markdown; an error for a file no language server
/// serves, rather than the empty answer the checkout's server gives for it; otherwise the file's
/// server's `textDocument/documentSymbol` answer. `hint` says how to list the locals.
pub async fn outline_file(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    path_str: &str,
    options: &OutlineOptions,
) -> Result<String> {
    let extension = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(extension.as_deref(), Some("md" | "markdown")) {
        let text = std::fs::read_to_string(file_path)
            .with_context(|| format!("reading {}", file_path.display()))?;
        let rendered = markdown_outline(&text, path_str, options);
        let (text, _) = cut_block(
            &rendered,
            options.max_bytes.unwrap_or(usize::MAX),
            options.max_items.unwrap_or(usize::MAX),
        );
        return Ok(text);
    }
    if matches!(extension.as_deref(), Some("proto")) {
        let text = std::fs::read_to_string(file_path)
            .with_context(|| format!("reading {}", file_path.display()))?;
        let rendered = protobuf_outline(&text, path_str, options);
        let (text, _) = cut_block(
            &rendered,
            options.max_bytes.unwrap_or(usize::MAX),
            options.max_items.unwrap_or(usize::MAX),
        );
        return Ok(text);
    }
    if crate::sync::engine_for_file(file_path).is_none() {
        let kind = extension.map_or_else(
            || "files without an extension".to_string(),
            |e| format!("`.{e}` files"),
        );
        anyhow::bail!(
            "no outline for {path_str}: language not supported (no language server serves {kind}). \
             prod-code serves Rust, Go, C, C++ and Objective-C, TypeScript and JavaScript, Python \
             and Swift, and outlines Markdown by its headings and Protobuf declarations"
        );
    }
    let file_uri = Url::from_file_path(file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        file_path,
        "textDocument/documentSymbol",
        params,
    )
    .await?;
    let source = std::fs::read_to_string(file_path).ok();
    let (text, _) = render_outline_with(&res, path_str, options, source.as_deref());
    let (text, _) = cut_block(
        &text,
        options.max_bytes.unwrap_or(usize::MAX),
        options.max_items.unwrap_or(usize::MAX),
    );
    Ok(text)
}

/// A Markdown file's headings as an outline: `#` to `######`, the level giving the depth, with
/// front matter and fenced code left out.
fn markdown_outline(text: &str, path: &str, options: &OutlineOptions) -> String {
    let mut out = format!("Outline for {path}:\n");
    let mut fence: Option<&str> = None;
    let trimmed_start = text.trim_start_matches('\u{feff}');
    let (mut front_matter, front_matter_marker) = if trimmed_start.starts_with("---")
        && (trimmed_start[3..].starts_with('\n') || trimmed_start[3..].starts_with("\r\n"))
    {
        (true, "---")
    } else if trimmed_start.starts_with("+++")
        && (trimmed_start[3..].starts_with('\n') || trimmed_start[3..].starts_with("\r\n"))
    {
        (true, "+++")
    } else {
        (false, "")
    };
    let mut headings = 0usize;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if front_matter {
            if i > 0 && line.trim() == front_matter_marker {
                front_matter = false;
            }
            continue;
        }
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m)) {
            fence = Some(marker);
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&level) || !trimmed[level..].starts_with([' ', '\t']) {
            continue;
        }
        let title = trimmed[level..].trim().trim_end_matches('#').trim_end();
        if level > options.max_depth || title.is_empty() {
            continue;
        }
        if let Some(kinds) = &options.kinds {
            if !kinds.iter().any(|k| k.eq_ignore_ascii_case("heading")) {
                continue;
            }
        }
        headings += 1;
        out.push_str(&format!("  [Heading {level}] {title} (line {})\n", i + 1));
    }
    if headings == 0 {
        out.push_str("  (no headings)");
    }
    out.trim_end().to_string()
}

/// A Protobuf file's package, services, messages, enums, methods, fields, and enum members as an outline.
pub fn protobuf_outline(text: &str, path: &str, options: &OutlineOptions) -> String {
    let mut sanitized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut escape = false;

    while let Some(c) = chars.next() {
        if in_line_comment {
            if c == '\n' {
                in_line_comment = false;
                sanitized.push('\n');
            } else {
                sanitized.push(' ');
            }
        } else if in_block_comment {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block_comment = false;
                sanitized.push(' ');
                sanitized.push(' ');
            } else if c == '\n' {
                sanitized.push('\n');
            } else {
                sanitized.push(' ');
            }
        } else if in_double_quote {
            if escape {
                escape = false;
                sanitized.push(if c == '\n' { '\n' } else { ' ' });
            } else if c == '\\' {
                escape = true;
                sanitized.push(' ');
            } else if c == '"' {
                in_double_quote = false;
                sanitized.push(' ');
            } else if c == '\n' {
                sanitized.push('\n');
            } else {
                sanitized.push(' ');
            }
        } else if in_single_quote {
            if escape {
                escape = false;
                sanitized.push(if c == '\n' { '\n' } else { ' ' });
            } else if c == '\\' {
                escape = true;
                sanitized.push(' ');
            } else if c == '\'' {
                in_single_quote = false;
                sanitized.push(' ');
            } else if c == '\n' {
                sanitized.push('\n');
            } else {
                sanitized.push(' ');
            }
        } else {
            if c == '/' && chars.peek() == Some(&'/') {
                chars.next();
                in_line_comment = true;
                sanitized.push(' ');
                sanitized.push(' ');
            } else if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                in_block_comment = true;
                sanitized.push(' ');
                sanitized.push(' ');
            } else if c == '"' {
                in_double_quote = true;
                sanitized.push(' ');
            } else if c == '\'' {
                in_single_quote = true;
                sanitized.push(' ');
            } else {
                sanitized.push(c);
            }
        }
    }

    struct Token {
        line: usize,
        text: String,
    }

    let mut tokens: Vec<Token> = Vec::new();
    for (line_idx, line) in sanitized.lines().enumerate() {
        let line_num = line_idx + 1;
        let mut char_indices = line.char_indices().peekable();
        while let Some((_, c)) = char_indices.next() {
            if c.is_whitespace() {
                continue;
            }
            if matches!(c, '{' | '}' | ';' | '=' | '(' | ')') {
                tokens.push(Token {
                    line: line_num,
                    text: c.to_string(),
                });
                continue;
            }
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                let mut tok = String::new();
                tok.push(c);
                while let Some(&(_, next_c)) = char_indices.peek() {
                    if next_c.is_ascii_alphanumeric() || next_c == '_' || next_c == '.' {
                        tok.push(next_c);
                        char_indices.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    line: line_num,
                    text: tok,
                });
            }
        }
    }

    #[derive(Copy, Clone, PartialEq, Eq)]
    enum Container {
        Message,
        Enum,
        Service,
        Other,
    }

    let mut stack: Vec<Container> = Vec::new();
    let mut declarations: Vec<(usize, &'static str, String, usize)> = Vec::new();

    let mut idx = 0;
    let n = tokens.len();
    while idx < n {
        let tok = &tokens[idx];
        let current_depth = stack.len() + 1;

        if matches!(
            tok.text.as_str(),
            "option" | "reserved" | "extensions" | "syntax" | "import"
        ) {
            idx += 1;
            let mut nested = 0usize;
            while idx < n {
                match tokens[idx].text.as_str() {
                    "{" | "(" => nested += 1,
                    "}" | ")" if nested > 0 => nested -= 1,
                    "}" => break,
                    ";" if nested == 0 => {
                        idx += 1;
                        break;
                    }
                    _ => {}
                }
                idx += 1;
            }
            continue;
        }

        if tok.text == "{" {
            stack.push(Container::Other);
            idx += 1;
            continue;
        } else if tok.text == "}" {
            stack.pop();
            idx += 1;
            continue;
        } else if tok.text == "package" {
            if idx + 1 < n {
                let pkg_name = &tokens[idx + 1].text;
                if pkg_name != ";" && pkg_name != "{" {
                    declarations.push((current_depth, "Package", pkg_name.clone(), tok.line));
                }
                idx += 2;
                while idx < n && tokens[idx].text != ";" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == ";" {
                    idx += 1;
                }
                continue;
            }
        } else if tok.text == "message" {
            if idx + 1 < n {
                let name = tokens[idx + 1].text.clone();
                declarations.push((current_depth, "Message", name, tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != "{" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == "{" {
                    stack.push(Container::Message);
                    idx += 1;
                }
                continue;
            }
        } else if tok.text == "enum" {
            if idx + 1 < n {
                let name = tokens[idx + 1].text.clone();
                declarations.push((current_depth, "Enum", name, tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != "{" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == "{" {
                    stack.push(Container::Enum);
                    idx += 1;
                }
                continue;
            }
        } else if tok.text == "service" {
            if idx + 1 < n {
                let name = tokens[idx + 1].text.clone();
                declarations.push((current_depth, "Service", name, tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != "{" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == "{" {
                    stack.push(Container::Service);
                    idx += 1;
                }
                continue;
            }
        } else if tok.text == "rpc" && stack.last() == Some(&Container::Service) {
            if idx + 1 < n {
                let name = tokens[idx + 1].text.clone();
                declarations.push((current_depth, "Method", name, tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != ";" && tokens[idx].text != "{" {
                    idx += 1;
                }
                if idx < n {
                    if tokens[idx].text == "{" {
                        stack.push(Container::Other);
                    }
                    idx += 1;
                }
                continue;
            }
        } else if stack.last() == Some(&Container::Enum) {
            if !matches!(tok.text.as_str(), "option" | "reserved")
                && idx + 1 < n
                && tokens[idx + 1].text == "="
            {
                declarations.push((current_depth, "EnumMember", tok.text.clone(), tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != ";" && tokens[idx].text != "}" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == ";" {
                    idx += 1;
                }
                continue;
            }
        } else if stack.last() == Some(&Container::Message) {
            if tok.text == "oneof" {
                if idx + 1 < n {
                    let name = tokens[idx + 1].text.clone();
                    declarations.push((current_depth, "Field", name, tok.line));
                    idx += 2;
                    while idx < n && tokens[idx].text != "{" {
                        idx += 1;
                    }
                    if idx < n && tokens[idx].text == "{" {
                        stack.push(Container::Message);
                        idx += 1;
                    }
                    continue;
                }
            } else if !matches!(
                tok.text.as_str(),
                "option" | "reserved" | "extensions" | "syntax" | "import"
            ) {
                let mut scan = idx;
                let mut found_eq = false;
                while scan < n && !matches!(tokens[scan].text.as_str(), ";" | "{" | "}") {
                    if tokens[scan].text == "=" {
                        found_eq = true;
                        break;
                    }
                    scan += 1;
                }
                if found_eq && scan > idx {
                    let field_name = tokens[scan - 1].text.clone();
                    let field_line = tokens[scan - 1].line;
                    declarations.push((current_depth, "Field", field_name, field_line));
                    idx = scan + 1;
                    while idx < n && !matches!(tokens[idx].text.as_str(), ";" | "}") {
                        idx += 1;
                    }
                    if idx < n && tokens[idx].text == ";" {
                        idx += 1;
                    }
                    continue;
                }
            }
        }

        idx += 1;
    }

    let mut out = format!("Outline for {path}:\n");
    let mut count = 0usize;
    for (depth, kind, name, line) in declarations {
        if depth > options.max_depth {
            continue;
        }
        if let Some(kinds) = &options.kinds {
            let matched = kinds.iter().any(|k| {
                k.eq_ignore_ascii_case(kind)
                    || (kind == "Message"
                        && (k.eq_ignore_ascii_case("struct") || k.eq_ignore_ascii_case("class")))
                    || (kind == "Service" && k.eq_ignore_ascii_case("interface"))
                    || (kind == "Method"
                        && (k.eq_ignore_ascii_case("function") || k.eq_ignore_ascii_case("rpc")))
                    || (kind == "Package"
                        && (k.eq_ignore_ascii_case("module")
                            || k.eq_ignore_ascii_case("namespace")))
                    || (kind == "EnumMember"
                        && (k.eq_ignore_ascii_case("member") || k.eq_ignore_ascii_case("constant")))
                    || (kind == "Field"
                        && (k.eq_ignore_ascii_case("property")
                            || k.eq_ignore_ascii_case("variable")))
            });
            if !matched {
                continue;
            }
        }
        count += 1;
        out.push_str(&format!("  [{kind}] {name} (line {line})\n"));
    }
    if count == 0 {
        out.push_str("  (no declarations)");
    }
    out.trim_end().to_string()
}

/// Outlines every source file directly in `dir_path` using a single [`crate::session::LspSession`],
/// file by file, skipping files the gateway cannot outline, and ends with how many files were
/// outlined and how many skipped.
pub async fn outline_directory(
    remote: SocketAddr,
    workspace_root: &Path,
    dir_path: &Path,
    path_display_prefix: &Path,
    options: &OutlineOptions,
) -> Result<String> {
    let read_dir = std::fs::read_dir(dir_path)
        .with_context(|| format!("Failed to read directory {:?}", dir_path))?;
    let mut entries = Vec::new();
    let mut skipped = 0usize;
    let mut tests_left_out = 0usize;
    for entry in read_dir.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        // A package's exports are not its tests' (Go's `TestX` is capitalised all the same).
        if options.exported_only && is_test_file(&entry.file_name().to_string_lossy()) {
            tests_left_out += 1;
            continue;
        }
        // Only source files a language server outlines: a manifest or a README is not asked
        // for, since a server that was handed one could answer with a made-up outline (#247).
        if crate::sync::engine_for_file(&entry.path()).is_some() {
            entries.push(entry);
        } else {
            skipped += 1;
        }
    }
    if entries.is_empty() {
        return Ok(subdirectory_listing(dir_path, path_display_prefix, skipped));
    }
    entries.sort_by_key(|e| e.file_name());
    let mut session =
        crate::session::LspSession::open(remote, workspace_root, Some(dir_path)).await?;

    let max_bytes = options.max_bytes.unwrap_or(usize::MAX);
    let max_items = options.max_items.unwrap_or(usize::MAX);
    let mut blocks: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut listed = 0usize;
    let mut outlined = 0usize;
    let mut not_reached: Vec<String> = Vec::new();
    let mut stopped = false;
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if stopped {
            not_reached.push(name);
            continue;
        }
        let entry_file = entry.path();
        let file_uri = match Url::from_file_path(&entry_file) {
            Ok(u) => u.to_string(),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri }
        });
        match session
            .query(&entry_file, "textDocument/documentSymbol", params)
            .await
        {
            Ok(res) if res.is_array() => {
                let display_path = path_display_prefix.join(&name).display().to_string();
                let source = std::fs::read_to_string(&entry_file).ok();
                let (block, count) =
                    render_outline_with(&res, &display_path, options, source.as_deref());
                if used + block.len() + 2 <= max_bytes && listed + count <= max_items {
                    used += block.len() + 2;
                    listed += count;
                    outlined += 1;
                    blocks.push(block);
                    continue;
                }
                stopped = true;
                if blocks.is_empty() {
                    // The first file alone is over the budget: as much of it as fits.
                    let (partial, kept) = cut_block(&block, max_bytes, max_items);
                    blocks.push(partial);
                    listed += kept;
                    outlined += 1;
                } else {
                    not_reached.push(name);
                }
            }
            _ => {
                skipped += 1;
            }
        }
    }

    let mut summary =
        format!("{outlined} file(s) outlined, {skipped} skipped, {listed} symbol(s) listed");
    if tests_left_out > 0 {
        summary.push_str(&format!(", {tests_left_out} test file(s) left out"));
    }
    if stopped {
        let limit = if listed >= max_items {
            format!("the limit of {max_items} symbols")
        } else {
            format!("the budget of {max_bytes} bytes")
        };
        summary.push_str(&format!("\nThe listing stops at {limit}"));
        if not_reached.is_empty() {
            summary.push('.');
        } else {
            let shown: Vec<&str> = not_reached.iter().take(20).map(String::as_str).collect();
            summary.push_str(&format!(
                "; {} more file(s) not outlined: {}{}.",
                not_reached.len(),
                shown.join(", "),
                if not_reached.len() > shown.len() {
                    format!(" and {} more", not_reached.len() - shown.len())
                } else {
                    String::new()
                }
            ));
        }
        summary.push_str(
            " Narrow it with `kinds` or `exported_only`, raise `max_bytes`, or outline one file.",
        );
    }
    Ok(format!("{}\n\n{summary}", blocks.join("\n\n")))
}

/// Whether a file name is a test file by its language's convention: `_test.go`, `*.test.ts`,
/// `*.spec.js`, `test_*.py`, `*_test.py`.
fn is_test_file(name: &str) -> bool {
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    match extension {
        "go" => stem.ends_with("_test"),
        "py" => stem.starts_with("test_") || stem.ends_with("_test"),
        "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs" => {
            stem.ends_with(".test") || stem.ends_with(".spec")
        }
        _ => false,
    }
}

/// For a directory with no source files of its own (a Go module's `internal/`), its
/// subdirectories that have some, with how many: where an outline finds something (#368).
fn subdirectory_listing(dir: &Path, display: &Path, skipped: usize) -> String {
    let mut subdirs: Vec<(String, usize)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
                return None;
            }
            let sources = source_files(&entry.path())
                .filter(|path| crate::sync::engine_for_file(path).is_some())
                .take(10_000)
                .count();
            (sources > 0).then_some((name, sources))
        })
        .collect();
    subdirs.sort();
    let shown = display.display();
    if subdirs.is_empty() {
        return format!("0 file(s) outlined, {skipped} skipped: {shown} has no source files");
    }
    let mut out = format!(
        "{shown} has no source files of its own; outline one of its {} subdirectories with sources:",
        subdirs.len()
    );
    for (name, sources) in subdirs {
        out.push_str(&format!(
            "\n  {} ({sources} source file(s))",
            display.join(&name).display()
        ));
    }
    out
}

/// As much of one outline block as fits `max_bytes` and `max_items`, with a line saying how
/// many symbols were left out; and how many it keeps.
fn cut_block(block: &str, max_bytes: usize, max_items: usize) -> (String, usize) {
    let symbols = block
        .lines()
        .filter(|l| l.trim_start().starts_with('['))
        .count();
    if block.len() <= max_bytes && symbols <= max_items {
        return (block.to_string(), symbols);
    }
    let mut lines = block.lines();
    let mut out = lines.next().unwrap_or_default().to_string();
    let mut kept = 0usize;
    for line in lines.filter(|l| l.trim_start().starts_with('[')) {
        if kept >= max_items || out.len() + line.len() + 1 > max_bytes {
            break;
        }
        out.push('\n');
        out.push_str(line);
        kept += 1;
    }
    out.push_str(&format!(
        "\n  … {} more symbol(s) of this file not listed",
        symbols - kept
    ));
    (out, kept)
}
/// The start and end line (0-based) of a symbol from `textDocument/documentSymbol`.
fn symbol_lines(sym: &serde_json::Value) -> (u64, u64) {
    let range = sym
        .get("range")
        .or_else(|| sym.get("location").and_then(|l| l.get("range")));
    let at = |edge: &str| {
        range
            .and_then(|r| r.get(edge))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
    };
    let start = at("start").unwrap_or(0);
    (start, at("end").unwrap_or(start).max(start))
}

/// A file's outline from its `textDocument/documentSymbol` answer, for the MCP tool and the
/// CLI alike. A variable inside a function or method is a local and is left out unless
/// `include_locals`; a top-level `static`, which the analyzer reports with the same kind, is not
/// inside one and stays. `hint` says how to list the locals anyway.
pub fn render_outline(
    res: &serde_json::Value,
    path: &str,
    max_depth: usize,
    include_locals: bool,
    hint: &str,
) -> String {
    render_outline_with(
        res,
        path,
        &OutlineOptions::all(max_depth, include_locals, hint),
        None,
    )
    .0
}

/// [`render_outline`] with the kind and export filters of `options`, reading `source` (the
/// file's text) to tell what is exported. Returns the text and how many symbols it lists.
pub fn render_outline_with(
    res: &serde_json::Value,
    path: &str,
    options: &OutlineOptions,
    source: Option<&str>,
) -> (String, usize) {
    let source_lines: Vec<&str> = source.map(|s| s.lines().collect()).unwrap_or_default();
    let language = crate::sync::engine_for_file(Path::new(path));
    let mut listed = 0usize;
    let mut out = String::new();
    if let Some(arr) = res.as_array() {
        out.push_str(&format!("Outline for {path}:\n"));
        let mut entries = Vec::new();
        outline_entries(arr, 1, false, &mut entries);
        let bodies: Vec<(u64, u64)> = entries
            .iter()
            .map(|(_, sym, _)| *sym)
            .filter(|s| matches!(s.get("kind").and_then(|k| k.as_u64()), Some(6 | 12)))
            .map(symbol_lines)
            .collect();
        let mut skipped_locals = 0usize;
        for (depth, sym, in_body) in entries {
            let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            // Locals (LSP kind 13, Variable, inside a body) are noise for a structural
            // outline: a 2000-line file lists hundreds of them.
            let (line, _) = symbol_lines(sym);
            let local =
                kind == 13 && (in_body || bodies.iter().any(|(s, e)| *s < line && line <= *e));
            if local && !options.include_locals {
                skipped_locals += 1;
                continue;
            }
            if depth > options.max_depth {
                continue;
            }
            let kind_str = match kind {
                2 => "Module",
                5 => "Class",
                6 => "Method",
                7 => "Property",
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
            if let Some(kinds) = &options.kinds
                && !kinds.iter().any(|k| k.eq_ignore_ascii_case(kind_str))
            {
                continue;
            }
            let declaration = source_lines.get(line as usize).copied().unwrap_or("");
            if options.exported_only && !is_exported(language, name, declaration, depth) {
                continue;
            }
            listed += 1;
            out.push_str(&format!("  [{kind_str}] {name} (line {})\n", line + 1));
        }
        if skipped_locals > 0 {
            out.push_str(&format!(
                "  ({skipped_locals} local variable(s) hidden; {} to list them)\n",
                options.hint
            ));
        }
    } else {
        out.push_str("No outline symbols available.");
    }
    (out.trim_end().to_string(), listed)
}

/// Whether a symbol is part of what its file exports, by its language's rule (#368): Go's
/// capital letter (a method's receiver type too), Rust's `pub`, Swift's `public` and `open`,
/// TypeScript's `export` at the top level and no `private`/`protected` below it, Python's names
/// without a leading underscore (dunder methods count). `declaration` is the text of the
/// symbol's line; `depth` its nesting, 1 for the top level.
fn is_exported(language: Option<&str>, name: &str, declaration: &str, depth: usize) -> bool {
    let decl = declaration.trim_start();
    let capital = |s: &str| {
        s.trim_start_matches(['*', '&', '(', '.'])
            .chars()
            .next()
            .is_some_and(char::is_uppercase)
    };
    match language {
        Some("go") => match name.strip_prefix('(').and_then(|r| r.split_once(')')) {
            Some((receiver, method)) => capital(receiver) && capital(method),
            None => capital(name),
        },
        Some("rust") => decl.starts_with("pub ") || decl.starts_with("pub("),
        Some("python") => {
            !name.starts_with('_') || (name.starts_with("__") && name.ends_with("__"))
        }
        Some("swift") => decl
            .split_whitespace()
            .any(|w| w == "public" || w == "open"),
        Some("typescript") => {
            if depth <= 1 {
                decl.starts_with("export ")
            } else {
                !name.starts_with('#')
                    && !decl
                        .split_whitespace()
                        .any(|w| w == "private" || w == "protected")
            }
        }
        _ => !decl.starts_with("static "),
    }
}

/// The symbols of a `textDocument/documentSymbol` answer in document order, each with its depth
/// and whether it sits in a function's body. A flat answer (the gateway's, for Rust) gives the
/// depth as a container chain ("a > b"); a nested one (`children`, as sourcekit-lsp, clangd,
/// pyright and the TypeScript server answer) by its nesting, whose members an outline used to
/// leave out (#358).
fn outline_entries<'a>(
    symbols: &'a [serde_json::Value],
    depth: usize,
    in_body: bool,
    out: &mut Vec<(usize, &'a serde_json::Value, bool)>,
) {
    for symbol in symbols {
        let chained = symbol
            .get("containerName")
            .and_then(|c| c.as_str())
            .filter(|c| c.contains(" > "))
            .map(|c| c.split(" > ").count() + 1);
        out.push((chained.unwrap_or(depth), symbol, in_body));
        if let Some(children) = symbol.get("children").and_then(|c| c.as_array()) {
            let body = in_body
                || matches!(
                    symbol.get("kind").and_then(|k| k.as_u64()),
                    Some(6 | 9 | 12)
                );
            outline_entries(children, depth + 1, body, out);
        }
    }
}
