/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

use super::lexer::{
    advance, ascii_identifier, ascii_identifier_end, keyword_at, matching, opaque_end, pair,
    skip_trivia,
};
use super::lsp::lsp_span;
use super::types::{FunctionRange, Span};

pub fn function_at(
    text: &str,
    requested: usize,
    answer: &serde_json::Value,
) -> Result<FunctionRange> {
    let symbols = answer.as_array().with_context(|| {
        format!("textDocument/documentSymbol returned no usable list: {answer}")
    })?;
    let mut selected = Vec::new();
    collect_selected(text, requested, symbols, &mut selected)?;
    anyhow::ensure!(
        selected.len() == 1,
        "the position is on {} declaration names instead of exactly one",
        selected.len()
    );
    let (name, kind, range, selection) = selected.pop().expect("one selected symbol");
    anyhow::ensure!(
        kind == 12,
        "the position names a non-function declaration, not an ordinary function"
    );
    parse_function(text, &name, range, selection)
}

pub fn collect_selected(
    text: &str,
    requested: usize,
    symbols: &[serde_json::Value],
    selected: &mut Vec<(String, u64, Span, Span)>,
) -> Result<()> {
    for symbol in symbols {
        let name = symbol
            .get("name")
            .and_then(serde_json::Value::as_str)
            .context("a document symbol has no name")?;
        let kind = symbol
            .get("kind")
            .and_then(serde_json::Value::as_u64)
            .context("a document symbol has no kind")?;
        anyhow::ensure!(
            !name.is_empty() && (1..=26).contains(&kind),
            "a document symbol is malformed: {symbol}"
        );
        anyhow::ensure!(
            symbol.get("location").is_none(),
            "SymbolInformation without an exact selectionRange is not sufficient TypeScript declaration evidence"
        );
        let range = lsp_span(
            text,
            symbol
                .get("range")
                .context("a document symbol has no declaration range")?,
        )
        .context("a document symbol has a malformed declaration range")?;
        let selection = lsp_span(
            text,
            symbol
                .get("selectionRange")
                .context("a document symbol has no selection range")?,
        )
        .context("a document symbol has a malformed selection range")?;
        anyhow::ensure!(
            range.start <= selection.start
                && selection.start < selection.end
                && selection.end <= range.end,
            "a document symbol's selection range is outside its declaration range"
        );
        if selection.start <= requested && requested < selection.end {
            selected.push((name.to_string(), kind, range, selection));
        }
        match symbol.get("children") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Array(children)) => {
                collect_selected(text, requested, children, selected)?
            }
            Some(_) => anyhow::bail!("a document symbol has malformed children"),
        }
    }
    Ok(())
}

pub fn parse_function(
    text: &str,
    symbol_name: &str,
    analyzer_range: Span,
    selection: Span,
) -> Result<FunctionRange> {
    let source_name = text
        .get(selection.start..selection.end)
        .context("the declaration selection range is stale")?;
    anyhow::ensure!(
        source_name == symbol_name,
        "the analyzer's symbol name {symbol_name:?} does not match {source_name:?}"
    );
    anyhow::ensure!(
        ascii_identifier(source_name),
        "{source_name:?} is not a supported ASCII TypeScript function name"
    );
    anyhow::ensure!(
        analyzer_range.start < analyzer_range.end && analyzer_range.end <= text.len(),
        "the declaration range is empty or outside the source"
    );
    let start = analyzer_range.start;
    anyhow::ensure!(
        keyword_at(text, start, "function"),
        "the declaration range does not begin at an ordinary function keyword"
    );
    ensure_top_level(text, start)?;
    refuse_export_or_modifier(text, start)?;

    let mut cursor = skip_trivia(text, start + "function".len())?;
    anyhow::ensure!(
        text.as_bytes().get(cursor) != Some(&b'*'),
        "generator functions are not supported"
    );
    let name_start = cursor;
    let name_end = ascii_identifier_end(text, name_start)
        .context("the function declaration has no ordinary ASCII name")?;
    anyhow::ensure!(
        name_start == selection.start
            && name_end == selection.end
            && &text[name_start..name_end] == source_name,
        "the analyzer's selection range does not match the current function declaration"
    );
    cursor = skip_trivia(text, name_end)?;
    anyhow::ensure!(
        text.as_bytes().get(cursor) != Some(&b'<'),
        "generic TypeScript functions are not supported"
    );
    anyhow::ensure!(
        text.as_bytes().get(cursor) == Some(&b'('),
        "the function has no parameter list"
    );
    cursor = matching(text, cursor)? + 1;
    loop {
        cursor = skip_trivia(text, cursor)?;
        let byte = *text
            .as_bytes()
            .get(cursor)
            .context("the function declaration has no complete body")?;
        match byte {
            b'{' => break,
            b';' | b'=' => {
                anyhow::bail!("ambient declarations and overload signatures are not supported")
            }
            b'<' => anyhow::bail!("generic TypeScript functions are not supported"),
            b'/' => anyhow::bail!(
                "regular-expression and division syntax is outside the safe-delete subset"
            ),
            b'(' | b'[' => cursor = matching(text, cursor)? + 1,
            b':' | b'?' | b'|' | b'&' | b'.' | b',' => cursor += 1,
            b'\'' | b'"' | b'\x60' => {
                cursor = opaque_end(text, cursor)?
                    .context("unterminated literal in the function signature")?
            }
            value if value.is_ascii_alphanumeric() || value == b'_' || value == b'$' => {
                cursor = ascii_identifier_end(text, cursor)
                    .context("the function signature contains a non-ASCII identifier")?;
            }
            _ => {
                anyhow::bail!("the function return type is outside the supported simple subset")
            }
        }
    }
    let body_end = matching(text, cursor)? + 1;
    anyhow::ensure!(
        analyzer_range.end == body_end,
        "the analyzer's declaration range does not exactly match the current function body"
    );
    Ok(FunctionRange {
        name: source_name.to_string(),
        start,
        end: body_end,
        name_start,
        name_end,
    })
}

pub fn refuse_export_or_modifier(text: &str, function_start: usize) -> Result<()> {
    let mut cursor = 0usize;
    while cursor < function_start {
        if text.as_bytes()[cursor].is_ascii_whitespace() {
            cursor += 1;
            continue;
        }
        if let Some(end) = opaque_end(text, cursor)? {
            cursor = end;
            continue;
        }
        if keyword_at(text, cursor, "export")
            || keyword_at(text, cursor, "declare")
            || keyword_at(text, cursor, "async")
        {
            let keyword_end = ascii_identifier_end(text, cursor).expect("known keyword");
            let mut next = skip_trivia(text, keyword_end)?;
            if keyword_at(text, cursor, "export") && keyword_at(text, next, "default") {
                next = skip_trivia(text, next + "default".len())?;
            }
            if next == function_start {
                anyhow::bail!("exported, ambient, and async functions are not supported");
            }
        }
        cursor = advance(text, cursor);
    }
    Ok(())
}

pub fn ensure_top_level(text: &str, end: usize) -> Result<()> {
    let mut stack = Vec::new();
    let mut cursor = 0usize;
    while cursor < end {
        if let Some(next) = opaque_end(text, cursor)? {
            anyhow::ensure!(
                next <= end,
                "a comment or literal overlaps the declaration range"
            );
            cursor = next;
            continue;
        }
        let byte = text.as_bytes()[cursor];
        if byte == b'/' {
            anyhow::bail!(
                "regular-expression and division syntax is outside the safe-delete subset"
            );
        }
        match byte {
            b'(' | b'[' | b'{' => stack.push(byte),
            b')' | b']' | b'}' => {
                let open = stack
                    .pop()
                    .context("unmatched closing delimiter before the function")?;
                anyhow::ensure!(pair(open, byte), "mismatched delimiter before the function");
            }
            _ => {}
        }
        cursor = advance(text, cursor);
    }
    anyhow::ensure!(
        stack.is_empty(),
        "the function is nested in another declaration or expression"
    );
    Ok(())
}
