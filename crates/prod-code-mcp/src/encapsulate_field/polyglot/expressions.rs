/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;
use anyhow::Result;

/// Setters return no assignment value. Only rewrite an expression statement whose
/// receiver starts at a statement boundary; nested assignments must block the plan.
pub(super) fn standalone_assignment(text: &str, field: usize, lang: Language) -> bool {
    let before = text[..field].trim_end();
    let separator = if before.ends_with("->") { 2 } else { 1 };
    let start = super::super::rust::chain_start(text, before.len() - separator);
    let mask = crate::caller_migration::lexical_code_mask(text, lang);
    // Python's staged setter expands to several statements. A semicolon inside
    // an inline compound suite cannot make those statements leave that suite.
    if lang == Language::Python && super::python_rhs::inline_suite(text, start, &mask) {
        return false;
    }
    // A semicolon/newline in an open header or call is not a statement boundary.
    let mut delimiters = Vec::new();
    for (_, ch) in text[..start].char_indices().filter(|(at, _)| mask[*at]) {
        match ch {
            '(' | '[' | '{' => delimiters.push(ch),
            ')' | ']' | '}' => {
                delimiters.pop();
            }
            _ => {}
        }
    }
    if !matches!(delimiters.last(), None | Some('{')) {
        return false;
    }
    if matches!(
        lang,
        Language::Cpp | Language::JavaScript | Language::TypeScript
    ) && super::cpp_statement::body_boundary(text, start, &mask)
    {
        return true;
    }
    let previous = text[..start]
        .char_indices()
        .rev()
        .find(|(at, c)| mask[*at] && !c.is_whitespace());
    match previous {
        None => true,
        Some((_, ';' | '{' | '}')) => true,
        // A Python suite colon terminates a header, not a continued expression.
        // Open expression delimiters were excluded above; an explicit '\\' still
        // reaches the continuation guard below rather than this suite boundary.
        Some((at, ':')) if lang == Language::Python => text[at..start].contains('\n'),
        Some((at, _)) => {
            let newline = text[at..start].contains('\n');
            newline
                && (lang == Language::Go
                    || (matches!(
                        lang,
                        Language::Python
                            | Language::Swift
                            | Language::JavaScript
                            | Language::TypeScript
                    ) && newline_statement_boundary(text, start, &mask)))
        }
    }
}

/// Swift and JavaScript/TypeScript permit newline-separated statements, but an
/// open call/parenthesis or preceding operator still consumes the assignment.
/// Braces start a new body even inside a call that takes a closure.
fn newline_statement_boundary(text: &str, start: usize, mask: &[bool]) -> bool {
    let mut delimiters = Vec::new();
    let mut previous = None;
    for (at, ch) in text[..start].char_indices() {
        if !mask[at] {
            if matches!(ch, '"' | '\'' | '`') && (at == 0 || mask[at - 1]) {
                previous = Some(ch);
            }
            continue;
        }
        if ch.is_whitespace() {
            continue;
        }
        match ch {
            '(' | '[' | '{' => delimiters.push(ch),
            ')' | ']' | '}' => {
                delimiters.pop();
            }
            _ => {}
        }
        previous = Some(ch);
    }
    if !matches!(delimiters.last(), None | Some('{')) {
        return false;
    }
    if previous.is_some_and(|ch| "=+-*/%&|^!<>?:,.\\".contains(ch)) {
        return false;
    }
    let prefix = text[..start].trim_end();
    let word = prefix
        .rsplit(|ch| !super::super::case::is_ident(ch))
        .next()
        .unwrap_or("");
    !matches!(word, "return" | "throw" | "yield")
}

pub(super) fn rhs_end(text: &str, start: usize, lang: Language) -> Result<usize> {
    if lang == Language::Python {
        return super::python_rhs::end(text, start);
    }
    let mask = crate::caller_migration::lexical_code_mask(text, lang);
    let mut depth = 0;
    let mut template_end = 0;
    // In C++, ?: has an expression (including comma) as its middle operand.
    // Only unmatched '?' at this delimiter depth protect a comma from ending RHS.
    let mut middle_operands = Vec::new();
    for (off, ch) in text[start..].char_indices() {
        let at = start + off;
        if at < template_end {
            continue;
        }
        if depth == 0 && ch == '\n' && lang != Language::Cpp {
            return Ok(at);
        }
        if !mask[at] {
            let begins_opaque = at == start || mask[at - 1];
            let comment = text[at..].starts_with("//") || (lang == Language::Python && ch == '#');
            if depth == 0 && begins_opaque && comment && lang != Language::Cpp {
                return Ok(text[..at].trim_end().len());
            }
            continue;
        }
        if lang == Language::TypeScript && depth == 0 && ch == '<' {
            reject_ambiguous_type_arguments(text, at, &mask)?;
        }
        if lang == Language::Cpp
            && ch == '<'
            && let Some(end) = template_arguments(text, at, &mask)?
        {
            template_end = end;
            continue;
        }
        match ch {
            '?' if lang == Language::Cpp => middle_operands.push(depth),
            ':' if lang == Language::Cpp
                && !text[at..].starts_with("::")
                && !text[..at].ends_with(':')
                && middle_operands.last() == Some(&depth) =>
            {
                middle_operands.pop();
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return Ok(at),
            ')' | ']' | '}' => depth -= 1,
            ';' if depth == 0 => return Ok(at),
            ',' if depth == 0 && middle_operands.is_empty() => return Ok(at),
            _ => {}
        }
    }
    anyhow::ensure!(depth == 0, "assignment RHS does not close");
    Ok(text.len())
}

/// Without a TypeScript expression parser, a comma after `<` may belong to type
/// arguments or a relational/comma expression. Refuse that ambiguity before the
/// ordinary RHS scanner can split a generic call and emit a malformed setter.
fn reject_ambiguous_type_arguments(text: &str, open: usize, mask: &[bool]) -> Result<()> {
    if text[open..].starts_with("<<") || text[open..].starts_with("<=") {
        return Ok(());
    }
    let mut angles = 1usize;
    let mut brackets = 0usize;
    for (off, ch) in text[open + 1..].char_indices() {
        let at = open + 1 + off;
        if !mask[at] {
            continue;
        }
        match ch {
            '(' | '[' | '{' => brackets += 1,
            ')' | ']' | '}' if brackets == 0 => break,
            ')' | ']' | '}' => brackets -= 1,
            ';' if brackets == 0 => break,
            '<' if brackets == 0 => angles += 1,
            // A function type's arrow does not close the type argument list.
            '>' if brackets == 0 && text.as_bytes()[at - 1] != b'=' => {
                angles -= 1;
                if angles == 0 {
                    break;
                }
            }
            ',' if brackets == 0 => anyhow::bail!(
                "cannot prove TypeScript type-argument/comma expression boundary; nothing was written"
            ),
            _ => {}
        }
    }
    Ok(())
}

/// Recognize a template-id rather than treating every `<` as a delimiter (which
/// breaks comparisons and shifts). Parenthesized non-type arguments have their
/// own delimiter depth. A comma-bearing ambiguous identifier-only list refuses
/// rather than guessing between a template-id and comma/relational expressions.
fn template_arguments(text: &str, open: usize, mask: &[bool]) -> Result<Option<usize>> {
    let prefix = text[..open].trim_end();
    if !prefix.ends_with(super::super::case::is_ident)
        || text[open..].starts_with("<<")
        || text[open..].starts_with("<=")
    {
        return Ok(None);
    }
    let mut angles = 1usize;
    let mut brackets = 0usize;
    let mut comma = false;
    let mut first_end = None;
    for (off, ch) in text[open + 1..].char_indices() {
        let at = open + 1 + off;
        if !mask[at] {
            continue;
        }
        match ch {
            '(' | '[' | '{' => brackets += 1,
            ')' | ']' | '}' if brackets == 0 => break,
            ')' | ']' | '}' => brackets -= 1,
            ';' if brackets == 0 => break,
            '<' if brackets == 0 => angles += 1,
            '>' if brackets == 0 => {
                angles -= 1;
                if angles == 0 {
                    let suffix = text[at + 1..].trim_start();
                    let first = &text[open + 1..first_end.unwrap_or(at)];
                    let type_syntax = definite_type_argument(first);
                    anyhow::ensure!(
                        !comma || type_syntax,
                        "cannot prove C++ template/comma expression boundary; nothing was written"
                    );
                    if !type_syntax
                        && !suffix.starts_with(['(', '{', ';', ','])
                        && !suffix.starts_with("::")
                    {
                        return Ok(None);
                    }
                    return Ok(Some(at + 1));
                }
            }
            ',' if brackets == 0 => {
                comma = true;
                if angles == 1 && first_end.is_none() {
                    first_end = Some(at);
                }
            }
            _ => {}
        }
    }
    anyhow::ensure!(
        !first_end.is_some_and(|end| definite_type_argument(&text[open + 1..end])),
        "cannot determine C++ template argument boundary; nothing was written"
    );
    Ok(None)
}

/// Built-in type-ids and these standard class templates cannot be relational
/// operands. A type-looking functional cast, e.g. `int(1)`, is an expression,
/// so checking only the first word would incorrectly swallow a comma operator.
/// Unknown typedef/template names require semantic type information; refuse
/// their ambiguous comma lists rather than treating spelling as that evidence.
fn definite_type_argument(argument: &str) -> bool {
    let argument = argument.trim();
    let words = |text: &str| {
        text.split(|c: char| c.is_whitespace() || matches!(c, '*' | '&'))
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    if ["std::pair<", "std::tuple<", "std::array<"]
        .iter()
        .any(|name| argument.starts_with(name))
    {
        return argument.rfind('>').is_some_and(|end| {
            words(&argument[end + 1..])
                .iter()
                .all(|word| matches!(word.as_str(), "const" | "volatile"))
        });
    }
    let words = words(argument);
    !words.is_empty()
        && words.iter().all(|word| {
            matches!(
                word.as_str(),
                "int"
                    | "long"
                    | "short"
                    | "unsigned"
                    | "signed"
                    | "char"
                    | "float"
                    | "double"
                    | "bool"
                    | "void"
                    | "wchar_t"
                    | "char8_t"
                    | "char16_t"
                    | "char32_t"
                    | "const"
                    | "volatile"
            )
        })
}
