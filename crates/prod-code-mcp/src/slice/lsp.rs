/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::PathBuf;

/// A 0-based protocol position; the character counts UTF-16 code units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Pos {
    pub(crate) line: u32,
    pub(crate) character: u32,
}

#[cfg(test)]
impl Pos {
    pub(crate) fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// A range whose end is not before its start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: Pos,
    pub(crate) end: Pos,
}

impl Span {
    #[cfg(test)]
    pub(crate) fn new(start: Pos, end: Pos) -> Self {
        Self { start, end }
    }

    pub(crate) fn contains(&self, pos: Pos) -> bool {
        self.start <= pos && pos <= self.end
    }

    pub(crate) fn lines(&self) -> u32 {
        self.end.line - self.start.line
    }
}

/// An item declared in a file, as `textDocument/documentSymbol` reports it.
#[derive(Debug, Clone)]
pub(crate) struct Decl {
    pub(crate) name: String,
    pub(crate) kind: &'static str,
    pub(crate) range: Span,
    /// Where its name is; the whole range for a flat `SymbolInformation`.
    pub(crate) selection: Span,
}

impl Decl {
    /// 1-based; coordinates are below `u32::MAX`, so this cannot overflow.
    pub(crate) fn start_line(&self) -> u32 {
        self.range.start.line + 1
    }

    pub(crate) fn end_line(&self) -> u32 {
        self.range.end.line + 1
    }
}

/// What kind of JSON value this is, for a message about an answer of the wrong shape.
pub(crate) fn shape(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "a list",
        serde_json::Value::Object(_) => "an object",
    }
}

/// A line or character: a non-negative integer small enough that its 1-based form fits a `u32`.
pub(crate) fn coordinate(value: &serde_json::Value, key: &str) -> Result<u32, String> {
    let raw = value
        .get(key)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| format!("`{key}` is not a non-negative integer"))?;
    u32::try_from(raw)
        .ok()
        .filter(|v| *v < u32::MAX)
        .ok_or_else(|| format!("`{key}` {raw} is out of range"))
}

pub(crate) fn parse_pos(value: Option<&serde_json::Value>) -> Result<Pos, String> {
    let value = value.ok_or("a position is missing")?;
    Ok(Pos {
        line: coordinate(value, "line")?,
        character: coordinate(value, "character")?,
    })
}

pub(crate) fn parse_symbol_span(value: Option<&serde_json::Value>) -> Result<Span, String> {
    let value = value.ok_or("a range is missing")?;
    let mut start = parse_pos(value.get("start"))?;
    let mut end = parse_pos(value.get("end"))?;
    if end < start {
        std::mem::swap(&mut start, &mut end);
    }
    Ok(Span { start, end })
}

pub(crate) fn parse_span(value: Option<&serde_json::Value>) -> Result<Span, String> {
    let value = value.ok_or("a range is missing")?;
    let span = Span {
        start: parse_pos(value.get("start"))?,
        end: parse_pos(value.get("end"))?,
    };
    if span.end < span.start {
        return Err("a range ends before it starts".to_string());
    }
    Ok(span)
}

pub(crate) fn kind_name(kind: u64) -> &'static str {
    match kind {
        2 => "module",
        5 => "class",
        6 => "method",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        22 => "enum member",
        23 => "struct",
        26 => "type parameter",
        _ => "item",
    }
}

/// Declarations worth slicing: things with a body or a shape, not fields or locals.
pub(crate) fn is_sliceable(kind: u64) -> bool {
    matches!(kind, 5 | 6 | 9 | 10 | 11 | 12 | 14 | 23)
}

/// The declarations of a `textDocument/documentSymbol` answer: null is a file with none, a list
/// is flattened with nested items (methods in an impl) as their own entries, and anything else,
/// a symbol without a name or kind, or a sliceable one without a valid range, is malformed. The
/// ranges of symbols the slicer does not slice are never used, so they are not judged: the real
/// Rust engine answers `mod name;` with a range that ends before it starts.
pub(crate) fn parse_decls(answer: &serde_json::Value) -> Result<Vec<Decl>, String> {
    let mut decls = Vec::new();
    match answer {
        serde_json::Value::Null => {}
        serde_json::Value::Array(symbols) => collect_decls(symbols, &mut decls)?,
        other => {
            return Err(format!(
                "expected a list of symbols or null, got {}",
                shape(other)
            ));
        }
    }
    Ok(decls)
}

pub(crate) fn collect_decls(
    symbols: &[serde_json::Value],
    out: &mut Vec<Decl>,
) -> Result<(), String> {
    for sym in symbols {
        let name = sym
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| format!("a symbol without a name: {}", shape(sym)))?;
        let kind = sym
            .get("kind")
            .and_then(|k| k.as_u64())
            .ok_or_else(|| format!("symbol `{name}` has no kind"))?;
        if is_sliceable(kind) && !name.is_empty() {
            let range = parse_symbol_span(
                sym.get("range")
                    .or_else(|| sym.get("location").and_then(|l| l.get("range"))),
            )
            .map_err(|e| format!("symbol `{name}`: {e}"))?;
            let selection = match sym.get("selectionRange") {
                Some(sel) => {
                    parse_symbol_span(Some(sel)).map_err(|e| format!("symbol `{name}`: {e}"))?
                }
                None => range,
            };
            out.push(Decl {
                name: name.to_string(),
                kind: kind_name(kind),
                range,
                selection,
            });
        }
        match sym.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => collect_decls(children, out)?,
            Some(other) => {
                return Err(format!(
                    "symbol `{name}` has children that are {}, not a list",
                    shape(other)
                ));
            }
        }
    }
    Ok(())
}

/// What a location's URI names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// A local file.
    File(PathBuf),
    /// A document of another scheme, such as a class inside a jar: outside the workspace and
    /// not a file the slicer can read.
    Other { scheme: String },
}

/// A place a definition answer points at: the source and the range the answer gives there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) source: Source,
    pub(crate) range: Span,
}

/// Every location of a `textDocument/definition` answer. Null and an empty list are the
/// analyzer's "nothing here"; a `Location`, a `LocationLink` or a list of them is followed in
/// full; anything else is malformed.
pub(crate) fn parse_locations(answer: &serde_json::Value) -> Result<Vec<Target>, String> {
    match answer {
        serde_json::Value::Null => Ok(Vec::new()),
        serde_json::Value::Array(items) => items.iter().map(parse_location).collect(),
        serde_json::Value::Object(_) => parse_location(answer).map(|t| vec![t]),
        other => Err(format!(
            "expected a location, a list of locations or null, got {}",
            shape(other)
        )),
    }
}

pub(crate) fn parse_location(value: &serde_json::Value) -> Result<Target, String> {
    let (uri, range) = match value.get("targetUri") {
        Some(uri) => (
            uri,
            value
                .get("targetSelectionRange")
                .or_else(|| value.get("targetRange")),
        ),
        None => (
            value
                .get("uri")
                .ok_or_else(|| format!("{} without `uri` or `targetUri`", shape(value)))?,
            value.get("range"),
        ),
    };
    let uri = uri.as_str().ok_or("a location's uri is not a string")?;
    Ok(Target {
        source: parse_source(uri)?,
        range: parse_span(range)?,
    })
}

/// The source a location's URI names. It must be an absolute URI. A `file:` URI must spell an
/// absolute path (`file:///a.rs`, `file:/a.rs`, `file://localhost/a.rs`) of a local file, with
/// no query or fragment; the URL parser would otherwise read `file:a.rs` as `/a.rs` and drop a
/// query, turning a malformed answer into a real-looking path.
pub(crate) fn parse_source(uri: &str) -> Result<Source, String> {
    let url =
        url::Url::parse(uri).map_err(|e| format!("uri `{uri}` is not an absolute URI: {e}"))?;
    if url.scheme() != "file" {
        if url.path().is_empty() {
            return Err(format!("uri `{uri}` names nothing after its scheme"));
        }
        return Ok(Source::Other {
            scheme: url.scheme().to_string(),
        });
    }
    let spelled_absolute = uri
        .get(..6)
        .is_some_and(|head| head.eq_ignore_ascii_case("file:/"));
    if !spelled_absolute {
        return Err(format!("uri `{uri}` is not an absolute file URI"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!(
            "file uri `{uri}` has a query or fragment, which no file path has"
        ));
    }
    if url.path().ends_with('/') {
        return Err(format!("file uri `{uri}` names a directory, not a file"));
    }
    url.to_file_path()
        .map(Source::File)
        .map_err(|()| format!("file uri `{uri}` does not name a path on this machine"))
}
