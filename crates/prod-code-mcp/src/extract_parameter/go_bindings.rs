/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    Syntax,
    contexts::Binding,
    tokens::{Token, ident, pairs, tokens},
};
use anyhow::{Context, Result};

/// Find a literal body after its signature, without mistaking result-type braces
/// (struct/interface) or a function-type declaration for that body.
pub(super) fn function_literal_end(
    text: &str,
    ts: &[Token<'_>],
    paired: &[Option<usize>],
    at: usize,
) -> Option<usize> {
    let end = super::go_types::signature_end(ts, paired, at, 0)?;
    if ts.get(end)?.text != "{" || inserted_semicolon(text, ts, end) {
        return None;
    }
    Some(paired[end]? + 1)
}

/// Short declarations reuse names only in their own block (including parameters/results).
/// An initializer still reads the previous binding; the new binding starts afterwards.
pub(super) fn classify<'a>(
    text: &str,
    close: usize,
    results: &[Token<'a>],
    ts: &[Token<'a>],
    paired: &[Option<usize>],
    allowed: &mut [bool],
    bindings: &mut Vec<Binding<'a>>,
) -> Result<()> {
    let mut parameters = parameters(text, close)?;
    if results.first().is_some_and(|t| t.text == "(") {
        let paired = pairs(results)?;
        let end = paired[0].context("missing Go result-list closing")?;
        parameters.extend(parameter_names(results, &paired, 0, end));
    }
    let mut scopes = Vec::new();
    for i in 0..ts.len() {
        if !allowed[i] {
            continue;
        }
        match ts[i].text {
            "{" => scopes.push(paired[i].unwrap_or(ts.len())),
            "}" => {
                scopes.pop();
            }
            ":=" => {
                let mut start = i;
                while start > 0 && (name(&ts[start - 1]) || ts[start - 1].text == ",") {
                    // Newlines after commas continue the same declaration head.
                    if start < i && inserted_semicolon(text, ts, start) {
                        break;
                    }
                    start -= 1;
                }
                let boundary = start == 0
                    || matches!(ts[start - 1].text, ";" | "{" | "}")
                    || inserted_semicolon(text, ts, start);
                anyhow::ensure!(
                    boundary && start < i,
                    "incomplete Go binding coverage: unsupported short-declaration scope"
                );
                anyhow::ensure!(
                    ts[start..i].iter().enumerate().all(|(n, t)| if n % 2 == 0 {
                        name(t)
                    } else {
                        t.text == ","
                    }) && (i - start) % 2 == 1,
                    "incomplete Go binding coverage: unsupported short-declaration names"
                );
                let end = statement_end(text, ts, paired, i + 1);
                let scope_end = scopes.last().copied().unwrap_or(ts.len());
                for t in ts[start..i].iter().step_by(2).filter(|t| t.text != "_") {
                    let reused = (scope_end == ts.len() && parameters.contains(&t.text))
                        || bindings
                            .iter()
                            .any(|b| b.name == t.text && b.end == scope_end && b.from < i);
                    if !reused {
                        bindings.push(Binding {
                            name: t.text,
                            from: end.saturating_sub(1),
                            end: scope_end,
                        });
                    }
                }
                allowed[start..=i].fill(false);
            }
            _ => {}
        }
    }
    Ok(())
}

/// A grouped declaration has one binding boundary per VarSpec, not per `var (...)`.
/// Keep initializer expressions executable; names and types are declaration data.
pub(super) fn grouped_variables<'a>(
    text: &str,
    ts: &[Token<'a>],
    paired: &[Option<usize>],
    open: usize,
    scope_end: usize,
    allowed: &mut [bool],
    bindings: &mut Vec<Binding<'a>>,
) {
    let close = paired[open].unwrap_or(ts.len());
    allowed[open] = false;
    if close < ts.len() {
        allowed[close] = false;
    }
    let mut start = open + 1;
    while start < close {
        if ts[start].text == ";" {
            start += 1;
            continue;
        }
        // The newline before this name terminates the *previous* spec. Start after
        // the name so it cannot be mistaken for this spec's own end boundary.
        let end = statement_end(text, ts, paired, start + 1).min(close);
        let initializer = super::contexts::seek(ts, paired, start, &["="]).min(end);
        allowed[start..initializer].fill(false);
        if initializer < end {
            allowed[initializer] = false;
        }
        let mut names_end = (start + 1).min(initializer);
        while names_end + 1 < initializer && ts[names_end].text == "," {
            names_end += 2;
        }
        for t in ts[start..names_end].iter().step_by(2) {
            if name(t) && t.text != "_" {
                bindings.push(Binding {
                    name: t.text,
                    from: end.saturating_sub(1),
                    end: scope_end,
                });
            }
        }
        // An inserted semicolon has no token: `end` is already the next spec's name.
        start = end;
    }
}

fn name(t: &Token<'_>) -> bool {
    !t.literal
        && t.text.chars().next().is_some_and(ident)
        && !matches!(
            t.text,
            "break"
                | "case"
                | "chan"
                | "const"
                | "continue"
                | "default"
                | "defer"
                | "else"
                | "fallthrough"
                | "for"
                | "func"
                | "go"
                | "goto"
                | "if"
                | "import"
                | "interface"
                | "map"
                | "package"
                | "range"
                | "return"
                | "select"
                | "struct"
                | "switch"
                | "type"
                | "var"
        )
}

/// Go inserts a semicolon only after these token classes, including in comment gaps.
/// In particular, `if\n x := ...` remains a control header, not a block declaration.
fn inserted_semicolon(text: &str, ts: &[Token<'_>], at: usize) -> bool {
    at > 0
        && text[ts[at - 1].end..ts[at].start].contains('\n')
        && (name(&ts[at - 1])
            || ts[at - 1].literal
            || matches!(
                ts[at - 1].text,
                "break" | "continue" | "fallthrough" | "return" | ")" | "]" | "}" | "++" | "--"
            ))
}

pub(super) fn statement_end(
    text: &str,
    ts: &[Token<'_>],
    paired: &[Option<usize>],
    mut at: usize,
) -> usize {
    while at < ts.len() {
        if matches!(ts[at].text, ";" | "}") {
            return at;
        }
        if inserted_semicolon(text, ts, at) {
            return at;
        }
        at = if matches!(ts[at].text, "(" | "[" | "{") {
            paired[at].map_or(at + 1, |n| n + 1)
        } else {
            at + 1
        };
    }
    at
}

fn parameters(text: &str, close: usize) -> Result<Vec<&str>> {
    let head = tokens(&text[..=close], Syntax::Go)?;
    let paired = pairs(&head)?;
    let end = head
        .len()
        .checked_sub(1)
        .context("missing Go parameter list")?;
    let open = paired[end].context("missing Go parameter-list opening")?;
    let mut names = parameter_names(&head, &paired, open, end);
    // A method's receiver list precedes its name and shares the body's block scope.
    if open >= 2
        && head[open - 2].text == ")"
        && let Some(receiver) = paired[open - 2]
        && receiver > 0
        && head[receiver - 1].text == "func"
    {
        names.extend(parameter_names(&head, &paired, receiver, open - 2));
    }
    Ok(names)
}

fn parameter_names<'a>(
    head: &[Token<'a>],
    paired: &[Option<usize>],
    open: usize,
    end: usize,
) -> Vec<&'a str> {
    let mut names = Vec::new();
    let mut pending = Vec::new();
    let mut start = open + 1;
    while start < end {
        let stop = super::contexts::seek(head, paired, start, &[","]).min(end);
        let first = &head[start];
        if stop == start + 1 && name(first) {
            pending.push(first.text);
        } else if name(first) && start + 1 < stop {
            let next = head[start + 1].text;
            // Parameter types may start with keywords as well as type names.
            // Commit pending grouped names only when this segment supplies a type.
            let named = name(&head[start + 1])
                || matches!(
                    next,
                    "(" | "*" | ".." | "<-" | "map" | "chan" | "func" | "struct" | "interface"
                )
                || (next == "[" && paired[start + 1].is_some_and(|close| close + 1 < stop));
            if named {
                names.append(&mut pending);
                names.push(first.text);
            } else {
                pending.clear();
            }
        } else {
            pending.clear();
        }
        start = stop + 1;
    }
    names
}
