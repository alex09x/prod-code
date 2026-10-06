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

use anyhow::{Context, Result, anyhow};

use super::directives::refuse_attached_directives;
use super::lex::{
    ascii_identifier, ascii_identifier_end, ascii_unexported_name, ensure_top_level, matching,
    opaque_end, pair, skip_trivia, token_boundary, trim_ascii_end,
};
use super::types::{FunctionRange, Receiver, Span, lsp_span, same_file};

pub(crate) fn function_at(
    file: &Path,
    text: &str,
    requested: usize,
    answer: &serde_json::Value,
) -> Result<FunctionRange> {
    let symbols = answer.as_array().with_context(|| {
        format!("textDocument/documentSymbol returned no usable list: {answer}")
    })?;
    let mut selected = Vec::new();
    collect_selected(file, text, requested, symbols, &mut selected)?;
    anyhow::ensure!(
        selected.len() == 1,
        "the position is on {} declaration names instead of exactly one",
        selected.len()
    );
    let (name, kind, range, selection) = selected.pop().expect("one selected symbol");
    anyhow::ensure!(
        matches!(kind, 6 | 12),
        "the position names a non-function declaration, not an ordinary function or receiver method"
    );
    let function = parse_function(text, &name, range, selection)?;
    anyhow::ensure!(
        matches!((kind, function.receiver.is_some()), (6, true) | (12, false)),
        "the analyzer's declaration kind does not match the current function declaration"
    );
    Ok(function)
}

fn collect_selected(
    file: &Path,
    text: &str,
    requested: usize,
    symbols: &[serde_json::Value],
    selected: &mut Vec<(String, u64, Span, Span)>,
) -> Result<()> {
    for symbol in symbols {
        let (Some(name), Some(kind)) = (
            symbol.get("name").and_then(serde_json::Value::as_str),
            symbol.get("kind").and_then(serde_json::Value::as_u64),
        ) else {
            anyhow::bail!("a document symbol is malformed: {symbol}");
        };
        anyhow::ensure!(
            !name.is_empty() && (1..=26).contains(&kind),
            "a document symbol is malformed: {symbol}"
        );

        let location_range = symbol.pointer("/location/range");
        if let Some(location) = symbol.get("location") {
            let uri = location
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .context("a symbol location has no URI")?;
            let uri = url::Url::parse(uri).context("a symbol location has an invalid URI")?;
            anyhow::ensure!(
                uri.scheme() == "file" && uri.query().is_none() && uri.fragment().is_none(),
                "a symbol location is not a plain local file URI"
            );
            let reported = uri
                .to_file_path()
                .map_err(|_| anyhow!("a symbol location is not a local file URI"))?;
            anyhow::ensure!(
                same_file(file, &reported),
                "a document symbol points at another file"
            );
        }

        let range = symbol
            .get("range")
            .or(location_range)
            .context("a document symbol has no declaration range")
            .and_then(|value| lsp_span(text, value))
            .context("a document symbol has a malformed range")?;
        let selection = match symbol.get("selectionRange") {
            Some(value) => Some(
                lsp_span(text, value).context("a document symbol has a malformed name range")?,
            ),
            None => match range {
                range if range.start <= requested && requested < range.end => Some(
                    infer_name_span(text, name, range)
                        .context("a document symbol has no usable name range")?,
                ),
                _ => None,
            },
        };
        if let Some(selection) = selection {
            anyhow::ensure!(
                range.start <= selection.start && selection.end <= range.end,
                "a document symbol's name is outside its declaration range"
            );
            anyhow::ensure!(
                selection.start < selection.end,
                "a document symbol has an empty name range"
            );
            if selection.start <= requested && requested < selection.end {
                selected.push((name.to_string(), kind, range, selection));
            }
        }
        match symbol.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => {
                collect_selected(file, text, requested, children, selected)?
            }
            Some(_) => anyhow::bail!("a document symbol has malformed children"),
        }
    }
    Ok(())
}

fn infer_name_span(text: &str, symbol_name: &str, range: Span) -> Result<Span> {
    anyhow::ensure!(
        range.start < range.end && range.end <= text.len(),
        "the declaration range is empty or outside the source"
    );
    anyhow::ensure!(
        text.get(range.start..range.start + 4) == Some("func")
            && token_boundary(text.as_bytes(), range.start, 4),
        "the declaration range does not begin at the func keyword"
    );
    let mut cursor = skip_trivia(text, range.start + 4)?;
    if text.as_bytes().get(cursor) == Some(&b'(') {
        cursor = skip_trivia(text, matching(text, cursor)? + 1)?;
    }
    let end = ascii_identifier_end(text, cursor)
        .context("the declaration range has no ordinary ASCII function name")?;
    let source_name = &text[cursor..end];
    anyhow::ensure!(
        symbol_matches(symbol_name, source_name),
        "the analyzer's symbol name {symbol_name} does not match {source_name}"
    );
    Ok(Span { start: cursor, end })
}

fn parse_function(
    text: &str,
    symbol_name: &str,
    analyzer_range: Span,
    selection: Span,
) -> Result<FunctionRange> {
    let source_name = text
        .get(selection.start..selection.end)
        .context("the declaration name range is stale")?;
    anyhow::ensure!(
        symbol_matches(symbol_name, source_name),
        "the analyzer's symbol name {symbol_name} does not match {source_name}"
    );
    anyhow::ensure!(
        ascii_unexported_name(source_name),
        "{} is not a supported unexported ASCII Go function name",
        source_name
    );
    anyhow::ensure!(
        !matches!(source_name, "main" | "init"),
        "Go entry point {} cannot be safely deleted",
        source_name
    );

    let start = function_start_for_name(text, selection.start)?;
    ensure_top_level(text, start)?;
    refuse_attached_directives(text, start)?;
    let mut cursor = skip_trivia(text, start + 4)?;
    let receiver = if text.as_bytes().get(cursor) == Some(&b'(') {
        let close = matching(text, cursor)?;
        let receiver = parse_receiver(&text[cursor + 1..close])?;
        cursor = skip_trivia(text, close + 1)?;
        Some(receiver)
    } else {
        None
    };
    let name_start = cursor;
    let name_end = ascii_identifier_end(text, name_start)
        .context("the function declaration has no ordinary ASCII name")?;
    anyhow::ensure!(
        &text[name_start..name_end] == source_name
            && selection.start == name_start
            && selection.end == name_end,
        "the analyzer's name range does not match the current declaration"
    );
    cursor = skip_trivia(text, name_end)?;
    anyhow::ensure!(
        text.as_bytes().get(cursor) != Some(&b'['),
        "generic Go functions are not supported"
    );
    anyhow::ensure!(
        text.as_bytes().get(cursor) == Some(&b'('),
        "the function has no parameter list"
    );
    let parameters_close = matching(text, cursor)?;
    cursor = parameters_close + 1;
    let mut anonymous_composite = false;
    let body_close = loop {
        cursor = skip_trivia(text, cursor)?;
        let byte = *text
            .as_bytes()
            .get(cursor)
            .context("the declaration has no complete body")?;
        match byte {
            b'\n' | b';' => anyhow::bail!("the declaration has no body here"),
            b'(' | b'[' => cursor = matching(text, cursor)? + 1,
            b'{' => {
                let close = matching(text, cursor)?;
                if anonymous_composite {
                    cursor = close + 1;
                    anonymous_composite = false;
                } else {
                    break close;
                }
            }
            b'"' | b'\'' | b'\x60' => {
                cursor = opaque_end(text, cursor)?
                    .context("unterminated literal in the function signature")?
            }
            first if first.is_ascii_alphabetic() || first == b'_' => {
                let end = ascii_identifier_end(text, cursor)
                    .context("an identifier in the function signature is malformed")?;
                anonymous_composite = matches!(&text[cursor..end], "struct" | "interface");
                cursor = end;
            }
            _ => {
                anonymous_composite = false;
                cursor += 1;
            }
        }
    };
    let end = body_close + 1;
    anyhow::ensure!(
        analyzer_range.start == start
            && trim_ascii_end(text, analyzer_range.start, analyzer_range.end) == end,
        "the analyzer's declaration range does not match the current function body"
    );
    Ok(FunctionRange {
        name: source_name.to_string(),
        start,
        end,
        name_start,
        name_end,
        receiver,
    })
}

fn parse_receiver(source: &str) -> Result<Receiver> {
    anyhow::ensure!(
        !source.contains("//") && !source.contains("/*"),
        "receiver comments make the declaration ambiguous"
    );
    let pieces: Vec<&str> = source.split_ascii_whitespace().collect();
    anyhow::ensure!(
        pieces.len() == 2 && ascii_identifier(pieces[0]) && pieces[0] != "_",
        "a receiver method must bind exactly one ordinary named receiver"
    );
    let named = pieces[1].strip_prefix('*').unwrap_or(pieces[1]);
    anyhow::ensure!(
        ascii_identifier(named),
        "receiver type {} is generic, qualified, aliased, or otherwise ambiguous",
        pieces[1]
    );
    Ok(Receiver {
        type_name: named.to_string(),
    })
}

fn function_start_for_name(text: &str, name_start: usize) -> Result<usize> {
    let mut stack = Vec::new();
    let mut cursor = 0usize;
    while cursor < name_start {
        if let Some(next) = opaque_end(text, cursor)? {
            cursor = next;
            continue;
        }
        match text.as_bytes()[cursor] {
            open @ (b'(' | b'[' | b'{') => stack.push(open),
            close @ (b')' | b']' | b'}') => {
                let opened = stack
                    .pop()
                    .context("unmatched closing delimiter before the function")?;
                anyhow::ensure!(
                    pair(opened, close),
                    "mismatched delimiter before the function"
                );
            }
            b'f' if stack.is_empty()
                && text.as_bytes()[cursor..].starts_with(b"func")
                && token_boundary(text.as_bytes(), cursor, 4) =>
            {
                let mut candidate = skip_trivia(text, cursor + 4)?;
                if text.as_bytes().get(candidate) == Some(&b'(') {
                    candidate = skip_trivia(text, matching(text, candidate)? + 1)?;
                }
                if candidate == name_start {
                    return Ok(cursor);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    anyhow::bail!("the analyzer's name is not on a top-level Go function declaration")
}

fn symbol_matches(symbol: &str, source: &str) -> bool {
    symbol == source
        || symbol.strip_suffix("()") == Some(source)
        || symbol.rsplit('.').next() == Some(source)
}
