/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::tokens::Token;
use anyhow::{Context, Result};

/// A brace at the start of a substitution or immediately inside parentheses starts
/// an object expression, not a statement block. Only classify its own closing
/// brace; method/function bodies still need conservative slash handling.
pub(super) fn ends_with_initial_object(ts: &[Token<'_>]) -> bool {
    if ts.last().is_none_or(|t| t.text != "}") {
        return false;
    }
    let mut depth = 0;
    for (i, token) in ts.iter().enumerate().rev() {
        match token.text {
            "}" => depth += 1,
            "{" => {
                depth -= 1;
                if depth == 0 {
                    return i == 0 || ts[i - 1].text == "(";
                }
            }
            _ => {}
        }
    }
    false
}

/// `satisfies` consumes a type operand, not the following runtime expression.
/// In particular, a template literal type has no executable substitutions.
pub(super) fn satisfies_type_end(
    ts: &[Token<'_>],
    pairs: &[Option<usize>],
    at: usize,
) -> Option<usize> {
    let previous = ts.get(at.checked_sub(1)?)?;
    // Reserved words following property access are values. Apply the statement
    // keyword exclusions only to bare keywords, including else/do boundaries.
    let member = at >= 2 && matches!(ts[at - 2].text, "." | "?." | "#");
    // A control condition closes before a statement, where `satisfies(...)`
    // names a runtime call. Calls/groupings, including private keyword-named
    // members such as `this.#if()`, still end value expressions.
    let after_condition = previous.text == ")"
        && pairs[at - 1].is_some_and(|open| {
            open > 0
                && (open < 2 || !matches!(ts[open - 2].text, "." | "?." | "#"))
                && matches!(
                    ts[open - 1].text,
                    "if" | "while" | "for" | "with" | "switch" | "catch"
                )
        });
    if ts[at].text != "satisfies"
        || after_condition
        || (!previous.literal
            && !matches!(previous.text, ")" | "]" | "}")
            && (!previous
                .text
                .chars()
                .next()
                .is_some_and(super::tokens::ident)
                || (!member
                    && matches!(
                        previous.text,
                        "return"
                            | "throw"
                            | "yield"
                            | "await"
                            | "new"
                            | "typeof"
                            | "void"
                            | "delete"
                            | "case"
                            | "in"
                            | "instanceof"
                            | "else"
                            | "do"
                    ))))
    {
        return None;
    }
    type_operand_end(ts, pairs, at + 1, 0)
}

fn type_operand_end(
    ts: &[Token<'_>],
    pairs: &[Option<usize>],
    at: usize,
    depth: usize,
) -> Option<usize> {
    if depth > 128 {
        return None;
    }
    let first = ts.get(at)?;
    let mut reference = false;
    let mut end = match first.text {
        "keyof" | "typeof" | "readonly" | "unique" | "infer" | "new" | "abstract" | "-" => {
            type_operand_end(ts, pairs, at + 1, depth + 1)?
        }
        "(" | "[" | "{" => pairs[at]? + 1,
        "<" => {
            let parameters = super::contexts::angle_end(ts, at)?;
            type_operand_end(ts, pairs, parameters, depth + 1)?
        }
        "any" | "unknown" | "number" | "bigint" | "object" | "boolean" | "string" | "symbol"
        | "void" | "undefined" | "null" | "never" | "this" | "true" | "false" | "intrinsic" => {
            at + 1
        }
        _ if first.literal => at + 1,
        _ if first.text.chars().next().is_some_and(super::tokens::ident) => {
            reference = true;
            at + 1
        }
        _ => return None,
    };
    loop {
        end = match ts.get(end).map(|t| t.text) {
            Some("[") => {
                reference = false;
                pairs[end]? + 1
            }
            // Literal/keyword/array types cannot take generic arguments. A `<`
            // following one is a runtime comparison, possibly with templates.
            Some("<") if reference => {
                reference = false;
                super::contexts::angle_end(ts, end)?
            }
            Some(".")
                if reference
                    && ts
                        .get(end + 1)?
                        .text
                        .chars()
                        .next()
                        .is_some_and(super::tokens::ident) =>
            {
                end + 2
            }
            Some("|" | "&" | "=>") => return type_operand_end(ts, pairs, end + 1, depth + 1),
            Some("extends") => {
                let constraint = type_operand_end(ts, pairs, end + 1, depth + 1)?;
                if ts.get(constraint)?.text != "?" {
                    return None;
                }
                let yes = type_operand_end(ts, pairs, constraint + 1, depth + 1)?;
                if ts.get(yes)?.text != ":" {
                    return None;
                }
                return type_operand_end(ts, pairs, yes + 1, depth + 1);
            }
            _ => return Some(end),
        };
    }
}

/// Keep a whole template opaque, including nested templates in substitutions.
/// Reuse the extraction lexer to cross strings, regexes and nested braces;
/// it does not establish expression/binding coverage for occurrence replacement.
pub(super) fn template_end(rest: &str) -> Result<Option<usize>> {
    if !rest.starts_with('`') {
        return Ok(None);
    }
    let bytes = rest.as_bytes();
    let mut at = 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => {
                at += 1;
                at += rest[at..]
                    .chars()
                    .next()
                    .context("unclosed template escape")?
                    .len_utf8();
            }
            b'`' => return Ok(Some(at + 1)),
            b'$' if bytes.get(at + 1) == Some(&b'{') => {
                at += 2;
                at += super::tokens::template_expression_end(&rest[at..])?;
            }
            _ => at += rest[at..].chars().next().unwrap().len_utf8(),
        }
    }
    anyhow::bail!("unclosed template literal")
}

/// Type aliases contain type grammar, including non-executable template types.
pub(super) fn type_alias_end(
    text: &str,
    ts: &[super::tokens::Token<'_>],
    pairs: &[Option<usize>],
    at: usize,
) -> Option<usize> {
    if ts.get(at)?.text != "type"
        || ts.get(at + 1)?.literal
        || !ts[at + 1]
            .text
            .chars()
            .next()
            .is_some_and(super::tokens::ident)
    {
        return None;
    }
    let mut equal = at + 2;
    if ts.get(equal)?.text == "<" {
        equal = super::contexts::angle_end(ts, equal)?;
    }
    if ts.get(equal)?.text != "=" {
        return None;
    }
    let mut end = equal + 1;
    while end < ts.len() && !matches!(ts[end].text, ";" | "}") {
        // A complete type followed by a newline ends the alias unless the next
        // token continues type grammar. This also preserves expression statements.
        if end > equal + 1
            && text[ts[end - 1].end..ts[end].start].contains('\n')
            && (ts[end - 1].literal
                || matches!(ts[end - 1].text, ")" | "]" | "}" | ">" | ">>")
                || (ts[end - 1]
                    .text
                    .chars()
                    .next()
                    .is_some_and(super::tokens::ident)
                    && !matches!(
                        ts[end - 1].text,
                        "keyof"
                            | "typeof"
                            | "readonly"
                            | "unique"
                            | "infer"
                            | "new"
                            | "abstract"
                            | "asserts"
                            | "is"
                            | "extends"
                    )))
            && !matches!(
                ts[end].text,
                "|" | "&"
                    | "["
                    | "<"
                    | ">"
                    | ">>"
                    | ","
                    | "."
                    | "extends"
                    | "?"
                    | ":"
                    | "is"
                    | "=>"
            )
        {
            break;
        }
        end = if ts[end].text == "<" {
            super::contexts::angle_end(ts, end)?
        } else if matches!(ts[end].text, "(" | "[" | "{") {
            pairs[end]? + 1
        } else {
            end + 1
        };
    }
    Some(end)
}

/// Only an unescaped `${` in template data introduces executable grammar.
/// Stop at the first substitution: its entire token has uncertain coverage even
/// when that expression consists of a string or another static template.
pub(super) fn interpolated(literal: &str) -> bool {
    if !literal.starts_with('`') {
        return false;
    }
    let bytes = literal.as_bytes();
    let mut at = 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'`' => return false,
            b'$' if bytes.get(at + 1) == Some(&b'{') => return true,
            _ => at += 1,
        }
    }
    false
}
