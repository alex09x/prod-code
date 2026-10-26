/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::extract_delegate::common::is_ident;
use crate::extract_trait::lex::lexical_code;

// Read only method modifiers, receiver expressions and simple local bindings. Raw
// identifiers remain single tokens; comments and literal contents cannot be keywords.
fn tokens(text: &str) -> Vec<&str> {
    let code = lexical_code(text);
    let mut out = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let ch = text[at..].chars().next().unwrap();
        if !code[at] || ch.is_whitespace() {
            at += ch.len_utf8();
            continue;
        }
        let start = at;
        if text[at..].starts_with("r#") {
            at += 2;
        }
        if is_ident(ch) {
            while at < text.len() {
                let next = text[at..].chars().next().unwrap();
                if !code[at] || !is_ident(next) {
                    break;
                }
                at += next.len_utf8();
            }
        } else {
            at += ch.len_utf8();
        }
        out.push(&text[start..at]);
    }
    out
}

pub(super) fn is_async(signature: &str) -> bool {
    let mut depth = 0usize;
    let mut asynchronous = false;
    for token in tokens(signature) {
        match token {
            "(" | "[" => depth += 1,
            ")" | "]" => depth = depth.saturating_sub(1),
            "async" if depth == 0 => asynchronous = true,
            "fn" if depth == 0 => return asynchronous,
            _ => {}
        }
    }
    false
}

fn grouped_end(tokens: &[&str], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, token) in tokens.iter().enumerate().skip(start) {
        match *token {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn unwrapped<'a>(mut expression: &'a [&'a str]) -> &'a [&'a str] {
    loop {
        match expression.first() {
            Some(&"(") if grouped_end(expression, 0) == Some(expression.len() - 1) => {
                expression = &expression[1..expression.len() - 1];
            }
            Some(&"&" | &"*" | &"mut") => expression = &expression[1..],
            _ => return expression,
        }
    }
}

fn origin(expression: &[&str], bindings: &BTreeMap<&str, bool>) -> Result<bool> {
    let expression = unwrapped(expression);
    if let [name] = expression {
        return Ok(bindings.get(name).copied().unwrap_or(false));
    }
    anyhow::ensure!(
        !expression.iter().enumerate().any(|(i, name)| {
            bindings.get(name) == Some(&true)
                && expression.get(i.wrapping_sub(1)) != Some(&".")
                && expression.get(i + 1) == Some(&"=")
                && !matches!(expression.get(i + 2), Some(&"=" | &">"))
                && !matches!(
                    i.checked_sub(1).and_then(|j| expression.get(j)),
                    Some(&"let" | &"mut")
                )
        }),
        "unsupported self-alias assignment in moved method; nothing was planned"
    );
    // Member values such as self.city are ordinary values, not self aliases. A
    // helper used as a value in a more complex expression needs semantic analysis.
    anyhow::ensure!(
        !expression.iter().enumerate().any(|(i, name)| {
            bindings.get(name) == Some(&true)
                && expression.get(i.wrapping_sub(1)) != Some(&".")
                && !matches!(expression.get(i + 1), Some(&"." | &"=" | &":"))
        }),
        "unsupported self-alias expression in moved method; nothing was planned"
    );
    Ok(false)
}

/// Resolve identity bindings up to this field receiver. Scope snapshots preserve
/// shadowing; assignments and complex alias patterns refuse rather than guess.
pub(super) fn helper_receiver(prefix: &str) -> Result<bool> {
    let tokens = tokens(prefix);
    let mut bindings = BTreeMap::from([("self", true)]);
    let mut scopes = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        match tokens[at] {
            "{" => scopes.push(bindings.clone()),
            "}" => bindings = scopes.pop().context("unbalanced moved method")?,
            "let" => {
                let mut end = at + 1;
                while end < tokens.len() && tokens[end] != ";" {
                    if matches!(tokens[end], "(" | "[" | "{") {
                        let Some(close) = grouped_end(&tokens, end) else {
                            break;
                        };
                        end = close;
                    }
                    end += 1;
                }
                if tokens.get(end) == Some(&";") {
                    let statement = &tokens[at + 1..end];
                    if let Some(equal) = statement.iter().position(|t| *t == "=") {
                        let helper = origin(&statement[equal + 1..], &bindings)?;
                        let pattern = statement[..equal]
                            .strip_prefix(&["mut"])
                            .unwrap_or(&statement[..equal]);
                        let name = pattern.first().copied().context("missing local binding")?;
                        if pattern.len() == 1 || pattern.get(1) == Some(&":") {
                            bindings.insert(name, helper);
                        } else {
                            anyhow::ensure!(
                                !helper && !pattern.iter().any(|t| bindings.get(t) == Some(&true)),
                                "unsupported self-alias pattern in moved method; nothing was planned"
                            );
                        }
                    }
                    at = end + 1;
                    continue;
                }
            }
            name if name.chars().all(is_ident)
                && matches!(
                    at.checked_sub(1).and_then(|i| tokens.get(i)),
                    Some(&";" | &"{" | &"}")
                )
                && tokens.get(at + 1) == Some(&"=")
                && !matches!(tokens.get(at + 2), Some(&"=" | &">")) =>
            {
                let helper = if let Some(end) = tokens[at + 2..].iter().position(|t| *t == ";") {
                    origin(&tokens[at + 2..at + 2 + end], &bindings)?
                } else {
                    false
                };
                anyhow::ensure!(
                    !helper && bindings.get(name) != Some(&true),
                    "unsupported self-alias assignment in moved method; nothing was planned"
                );
            }
            // These binding forms can shadow an existing alias. Do not let a
            // textual match silently treat the new variable as helper-owned.
            "for" | "|" => {
                let end = tokens[at + 1..]
                    .iter()
                    .position(|t| matches!(*t, "in" | "|"));
                if let Some(end) = end {
                    anyhow::ensure!(
                        !tokens[at + 1..at + 1 + end]
                            .iter()
                            .any(|t| bindings.get(t) == Some(&true)),
                        "unsupported self-alias shadowing in moved method; nothing was planned"
                    );
                }
            }
            name if !scopes.is_empty() && bindings.get(name) == Some(&true) => {
                // Identity lets were handled above. Other uses as values could
                // propagate the helper through patterns/iterators/closures whose
                // bindings this narrow reader cannot resolve. Refuse those uses.
                let mut after = at + 1;
                while tokens.get(after) == Some(&")") {
                    after += 1;
                }
                anyhow::ensure!(
                    tokens.get(at.wrapping_sub(1)) == Some(&".")
                        || tokens.get(after) == Some(&".")
                        || after == tokens.len(),
                    "unsupported self-alias value flow in moved method; nothing was planned"
                );
            }
            _ => {}
        }
        at += 1;
    }
    let end = tokens.len();
    anyhow::ensure!(end > 0, "missing field receiver");
    let start = receiver_start(&tokens, end)?;
    origin(&tokens[start..end], &bindings)
}

// Walk complete postfix receivers, including member chains, calls and indexing.
// A member token never acquires the identity of an unrelated local with its name.
fn receiver_start(tokens: &[&str], end: usize) -> Result<usize> {
    let mut start = end.checked_sub(1).context("missing field receiver")?;
    if matches!(tokens[start], ")" | "]") {
        let mut depth = 1;
        while start > 0 && depth > 0 {
            start -= 1;
            match tokens[start] {
                ")" | "]" => depth += 1,
                "(" | "[" => depth -= 1,
                _ => {}
            }
        }
        anyhow::ensure!(depth == 0, "unbalanced field receiver");
        if start > 0
            && ((tokens[start - 1].chars().all(is_ident)
                && !matches!(
                    tokens[start - 1],
                    "return" | "break" | "if" | "match" | "while" | "in"
                ))
                || matches!(tokens[start - 1], ")" | "]"))
        {
            start = receiver_start(tokens, start)?;
        }
    }
    if start >= 2 && tokens[start - 1] == "." {
        start = receiver_start(tokens, start - 1)?;
    }
    Ok(start)
}
