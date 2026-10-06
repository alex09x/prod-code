/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::types::Occurrence;

/// What kind of file this is, which decides who is allowed to edit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A language with an engine on the gateway: identifiers are renamed by the analyzer.
    Code(&'static str),
    /// A schema, a query or a document: the text is all there is.
    Text(&'static str),
    Skip,
}

pub(crate) fn kind_of(path: &Path) -> Kind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => Kind::Code("rust"),
        "go" => Kind::Code("go"),
        "ts" | "tsx" | "mts" | "cts" => Kind::Code("typescript"),
        "js" | "jsx" | "mjs" | "cjs" => Kind::Code("javascript"),
        "py" | "pyi" => Kind::Code("python"),
        "swift" => Kind::Code("swift"),
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hxx" => Kind::Code("c/c++"),
        "proto" => Kind::Text("protobuf"),
        "sql" => Kind::Text("sql"),
        "graphql" | "gql" => Kind::Text("graphql"),
        "json" => Kind::Text("json"),
        "yaml" | "yml" => Kind::Text("yaml"),
        "toml" => Kind::Text("toml"),
        "md" => Kind::Text("markdown"),
        "sh" | "bash" | "fish" => Kind::Text("shell"),
        "txt" | "csv" | "env" => Kind::Text("text"),
        _ => Kind::Skip,
    }
}

/// A schema format whose structure decides which spellings are the field and which are prose
/// that mentions it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Schema {
    OpenApi,
    GraphQl,
    Json,
    Yaml,
}

/// Which schema format a file is, when it is one. An OpenAPI document is YAML or JSON with an
/// `openapi` (or Swagger's `swagger`) key: at the start of a line in YAML, anywhere in JSON.
pub(crate) fn schema_of(path: &Path, text: &str) -> Option<Schema> {
    let keyed = |line: &str, key: &str| {
        line.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with(':'))
    };
    match kind_of(path) {
        Kind::Text("graphql") => Some(Schema::GraphQl),
        Kind::Text("yaml") => Some(
            if text
                .lines()
                .any(|l| keyed(l, "openapi") || keyed(l, "swagger"))
            {
                Schema::OpenApi
            } else {
                Schema::Yaml
            },
        ),
        Kind::Text("json") => Some(
            if text
                .lines()
                .map(|l| l.trim_start_matches(|c: char| c == '{' || c.is_whitespace()))
                .any(|l| keyed(l, "\"openapi\"") || keyed(l, "\"swagger\""))
            {
                Schema::OpenApi
            } else {
                Schema::Json
            },
        ),
        _ => None,
    }
}

/// What a file's occurrences are counted under: its schema format, or its language.
pub(crate) fn label(path: &Path, text: &str) -> Option<&'static str> {
    match (schema_of(path, text), kind_of(path)) {
        (Some(Schema::OpenApi), _) => Some("openapi"),
        (Some(Schema::GraphQl), _) => Some("graphql"),
        (Some(Schema::Json), _) => Some("json"),
        (Some(Schema::Yaml), _) => Some("yaml"),
        (None, Kind::Code(l) | Kind::Text(l)) => Some(l),
        (None, Kind::Skip) => None,
    }
}

/// Is this occurrence the field itself, rather than a mention of it in prose?
///
/// In OpenAPI the field is a whole key or a whole scalar: what comes before it (past a quote
/// and spaces) opens a key or a value (`:`, `-`, `[`, `,`, `{`, or nothing), and what comes after
/// closes one (`:`, `,`, `]`, `}`, a comment, or nothing). `order_id:`, `- order_id`,
/// `required: [id, order_id]` and `"order_id": {` qualify; `description: The order_id of…` does
/// not. In GraphQL it is any name outside a `#` comment and a string or `"""` description.
pub(crate) fn is_structural(schema: Schema, text: &str, o: &Occurrence) -> bool {
    let line = text.lines().nth(o.line as usize - 1).unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    let start = (o.col as usize - 1).min(chars.len());
    let end = (start + o.len).min(chars.len());
    let before: String = chars[..start].iter().collect();
    let after: String = chars[end..].iter().collect();
    match schema {
        Schema::OpenApi => {
            let mut before = before.trim_end();
            let mut after = after.as_str();
            if let (Some(q @ ('"' | '\'')), Some(c)) = (before.chars().last(), after.chars().next())
                && c == q
            {
                before = &before[..before.len() - 1];
                after = &after[1..];
            }
            let before = before.trim();
            let after = after.trim_start();
            let opens = before.is_empty() || before.ends_with([':', '-', '[', ',', '{']);
            let closes = after.is_empty() || after.starts_with([':', ',', ']', '}', '#']);
            opens && closes
        }
        Schema::GraphQl => !o.in_string && !before.contains('#') && !in_block_string(text, o),
        Schema::Json | Schema::Yaml => is_mapping_key(line, o),
    }
}

pub(crate) fn is_mapping_key(line: &str, occurrence: &Occurrence) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let end = (occurrence.col as usize - 1 + occurrence.len).min(chars.len());
    let after = chars[end..].iter().collect::<String>();
    let after = after.trim_start();
    if after.starts_with(':') {
        return true;
    }
    for quote in ['"', '\''] {
        if let Some(rest) = after.strip_prefix(quote) {
            return rest.trim_start().starts_with(':');
        }
    }
    false
}

pub(crate) fn text_comment_before(path: &Path, text: &str, occurrence: &Occurrence) -> bool {
    let (yaml, json) = match kind_of(path) {
        Kind::Text("yaml") => (true, false),
        Kind::Text("json") => (false, true),
        _ => return false,
    };
    let mut line_start = 0usize;
    for (index, raw_line) in text.split_inclusive('\n').enumerate() {
        if index + 1 == occurrence.line as usize {
            let line = raw_line
                .strip_suffix('\n')
                .unwrap_or(raw_line)
                .strip_suffix('\r')
                .unwrap_or_else(|| raw_line.strip_suffix('\n').unwrap_or(raw_line));
            let target_col = occurrence.col.saturating_sub(1) as usize;
            let target_byte = line
                .char_indices()
                .nth(target_col)
                .map_or(line.len(), |(byte, _)| byte);
            let end = line_start + target_byte;
            let bytes = text.as_bytes();
            let mut i = 0usize;
            let mut quote = None;
            let mut escaped = false;
            let mut line_comment = false;
            let mut block_comment = false;
            while i < end {
                let byte = bytes[i];
                if line_comment {
                    if byte == b'\n' {
                        line_comment = false;
                    }
                    i += 1;
                    continue;
                }
                if block_comment {
                    if byte == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        block_comment = false;
                        i += 2;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                if let Some(delimiter) = quote {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == delimiter {
                        quote = None;
                    }
                    i += 1;
                    continue;
                }
                if (yaml && byte == b'#')
                    || (json && byte == b'/' && bytes.get(i + 1) == Some(&b'/'))
                {
                    line_comment = true;
                    i += if yaml { 1 } else { 2 };
                    continue;
                }
                if json && byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    block_comment = true;
                    i += 2;
                    continue;
                }
                if matches!(byte, b'\'' | b'"' | b'`') {
                    quote = Some(byte);
                }
                i += 1;
            }
            return line_comment || block_comment;
        }
        line_start += raw_line.len();
    }
    false
}

/// Is this occurrence inside a GraphQL block string (`"""…"""`, a description)?
pub(crate) fn in_block_string(text: &str, o: &Occurrence) -> bool {
    let mut quotes = 0;
    for (n, line) in text.lines().enumerate() {
        if n + 1 == o.line as usize {
            let prefix: String = line.chars().take(o.col as usize - 1).collect();
            quotes += prefix.matches("\"\"\"").count();
            break;
        }
        quotes += line.matches("\"\"\"").count();
    }
    quotes % 2 == 1
}
