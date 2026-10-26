/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::contexts::{Binding, angle_end, declaration_end, pattern_bindings, seek};
use super::{Syntax, tokens::Token};

/// Classify declarations, patterns and types before parsing value expressions. A nested
/// declaration cannot capture the new parameter, and a closure has its own binding context.
pub(super) fn executable<'a>(
    text: &str,
    ts: &[Token<'a>],
    pairs: &[Option<usize>],
    syntax: Syntax,
) -> (Vec<bool>, Vec<Binding<'a>>) {
    let mut allowed = vec![true; ts.len()];
    let mut bindings = Vec::new();
    let mut scopes = Vec::new();
    let mut i = 0;
    while i < ts.len() {
        // Grouped Go declarations preclassify their heads, including nested type braces.
        if syntax == Syntax::Go && !allowed[i] {
            i += 1;
            continue;
        }
        let word = ts[i].text;
        if word == "}" {
            scopes.pop();
        }
        if word == "{" {
            scopes.push(pairs[i].unwrap_or(ts.len()));
        }
        let scope_end = scopes.last().copied().unwrap_or(ts.len());
        if syntax.is_c_family()
            && let Some(end) = super::c_constants::constant_end(ts, pairs, i, syntax)
        {
            allowed[i..end].fill(false);
            i = end;
            continue;
        }
        if syntax == Syntax::Go
            && let Some(end) = super::go_types::composite_type_end(ts, pairs, i)
        {
            allowed[i..end].fill(false);
            i = end;
            continue;
        }
        if syntax == Syntax::TypeScript
            && let Some(end) = super::javascript::type_alias_end(text, ts, pairs, i)
        {
            allowed[i..end].fill(false);
            i = end;
            continue;
        }
        if syntax == Syntax::TypeScript
            && let Some(end) = super::javascript::satisfies_type_end(ts, pairs, i)
        {
            allowed[i..end].fill(false);
            i = end;
            continue;
        }
        // Function literals have independent parameters/results, just like nested
        // declarations. A function *type* has no body and must not hide later code.
        if syntax == Syntax::Go
            && word == "func"
            && let Some(end) = super::go_bindings::function_literal_end(text, ts, pairs, i)
        {
            allowed[i..end].fill(false);
            i = end;
            continue;
        }
        // Object methods are declarations, including numeric/quoted keys. Computed keys
        // are evaluated in this scope, but parameters and method bodies are independent.
        if matches!(syntax, Syntax::JavaScript | Syntax::TypeScript)
            && word == "("
            && let Some((start, key, computed, end)) = object_method(ts, pairs, i)
        {
            allowed[start..key].fill(false);
            if !computed {
                allowed[key..i].fill(false);
            }
            allowed[i..=end].fill(false);
            i = end + 1;
            continue;
        }
        let nested = matches!(
            word,
            "fn" | "function"
                | "def"
                | "class"
                | "struct"
                | "enum"
                | "impl"
                | "trait"
                | "interface"
                | "namespace"
        ) || (syntax == Syntax::Rust
            && matches!(word, "const" | "static" | "type" | "mod" | "use"));
        if nested {
            let end = declaration_end(text, ts, pairs, i, syntax);
            allowed[i..end].fill(false);
            i = end;
            continue;
        }
        // Macro tokens are a different grammar (including arbitrary patterns and types).
        if syntax == Syntax::Rust
            && word == "!"
            && i > 0
            && ts[i - 1]
                .text
                .chars()
                .next()
                .is_some_and(super::tokens::ident)
            && let Some(end) = pairs.get(i + 1).copied().flatten()
        {
            allowed[i - 1..=end].fill(false);
            i = end + 1;
            continue;
        }
        if matches!(word, "let" | "var" | "const") {
            if syntax == Syntax::Go && word == "var" && ts.get(i + 1).is_some_and(|t| t.text == "(")
            {
                super::go_bindings::grouped_variables(
                    text,
                    ts,
                    pairs,
                    i + 1,
                    scope_end,
                    &mut allowed,
                    &mut bindings,
                );
                allowed[i] = false;
                i += 2;
                continue;
            }
            // Go declaration scopes begin after their VarSpec, including when the
            // terminating semicolon is inserted at a newline or inside a comment.
            let go_end = (syntax == Syntax::Go)
                .then(|| super::go_bindings::statement_end(text, ts, pairs, i + 1));
            let end =
                seek(ts, pairs, i + 1, &["=", ":=", ";", "}"]).min(go_end.unwrap_or(ts.len()));
            allowed[i..end].fill(false);
            let initialized = end < ts.len() && matches!(ts[end].text, "=" | ":=");
            if initialized || go_end.is_some() {
                if initialized {
                    allowed[end] = false;
                }
                // A binding begins after its initializer, so `let x = x + 1` reads the old x.
                let conditional = i > 0 && matches!(ts[i - 1].text, "if" | "while");
                let stmt_end = go_end.unwrap_or_else(|| {
                    seek(
                        ts,
                        pairs,
                        end + 1,
                        if conditional { &["{"] } else { &[";", "}"] },
                    )
                });
                let binding_end = if conditional {
                    pairs.get(stmt_end).copied().flatten().unwrap_or(scope_end)
                } else {
                    scope_end
                };
                let names_end = if syntax == Syntax::Go {
                    // A VarSpec begins with a comma-separated name list. Its type
                    // identifiers are not declarations in the surrounding block.
                    let mut at = (i + 2).min(end);
                    while at + 1 < end && ts[at].text == "," {
                        at += 2;
                    }
                    at
                } else {
                    (i + 1..end).find(|n| ts[*n].text == ":").unwrap_or(end)
                };
                for t in &ts[i + 1..names_end] {
                    if !t.literal
                        && t.text.chars().next().is_some_and(super::tokens::ident)
                        && !matches!(t.text, "mut" | "ref")
                    {
                        bindings.push(Binding {
                            name: t.text,
                            from: if syntax == Syntax::Go {
                                stmt_end.saturating_sub(1)
                            } else {
                                stmt_end
                            },
                            end: binding_end,
                        });
                    }
                }
            }
            i = end.max(i + 1);
            continue;
        }
        if word == "for" {
            let end = seek(ts, pairs, i + 1, &["in", ";", "{"]);
            if end < ts.len() && ts[end].text == "in" {
                allowed[i..=end].fill(false);
                let open = seek(ts, pairs, end + 1, &["{"]);
                if let Some(close) = pairs.get(open).copied().flatten() {
                    pattern_bindings(ts, i + 1..end, open, close, &mut bindings);
                }
            }
        }
        if word == "match" {
            let open = seek(ts, pairs, i + 1, &["{"]);
            if let Some(end) = pairs.get(open).copied().flatten() {
                let mut arm = open + 1;
                while arm < end {
                    let arrow = seek(ts, pairs, arm, &["=>"]);
                    if arrow >= end {
                        break;
                    }
                    let guard = (arm..arrow).find(|n| ts[*n].text == "if").unwrap_or(arrow);
                    allowed[arm..guard].fill(false);
                    allowed[arrow] = false;
                    let next = if ts.get(arrow + 1).is_some_and(|t| t.text == "{") {
                        pairs[arrow + 1].map_or(end, |n| n + 1)
                    } else {
                        seek(ts, pairs, arrow + 1, &[","]).min(end)
                    };
                    pattern_bindings(ts, arm..guard, guard.saturating_sub(1), next, &mut bindings);
                    arm = next + usize::from(ts.get(next).is_some_and(|t| t.text == ","));
                }
            }
        }
        // A closure's arguments and body form an independent binding boundary.
        let prefix = i == 0 || matches!(ts[i - 1].text, "=" | "(" | "," | "{" | "return" | "move");
        if (syntax == Syntax::Rust && prefix && matches!(word, "|" | "||")) || word == "=>" {
            let body = if word == "|" {
                (i + 1..ts.len())
                    .find(|n| ts[*n].text == "|")
                    .map(|n| n + 1)
            } else {
                Some(i + 1)
            };
            if let Some(body) = body {
                let end = if ts.get(body).is_some_and(|t| t.text == "{") {
                    pairs[body].map(|n| n + 1).unwrap_or(body)
                } else {
                    seek(ts, pairs, body, &[",", ";", "}", ")"])
                };
                // A match arm arrow was already classified above and is not a closure.
                if allowed[i] {
                    let start = if word == "=>" && i > 0 {
                        pairs[i - 1].unwrap_or(i - 1)
                    } else {
                        i
                    };
                    allowed[start..end].fill(false);
                    i = end;
                    continue;
                }
            }
        }
        // Member names and record labels are not independent value expressions.
        if matches!(word, "." | "?." | "::" | "->") && i + 1 < ts.len() {
            allowed[i + 1] = false;
        }
        if ts.get(i + 1).is_some_and(|t| t.text == ":")
            && !(matches!(
                syntax,
                Syntax::JavaScript | Syntax::TypeScript | Syntax::C | Syntax::Cpp
            ) && conditional_colon(ts, pairs, i + 1))
        {
            allowed[i] = false;
        }
        // Array repeat lengths are const expressions, not values that can read a parameter.
        if syntax == Syntax::Rust
            && word == "["
            && let Some(end) = pairs[i]
        {
            let semi = seek(ts, pairs, i + 1, &[";", "]"]);
            if semi < end {
                allowed[semi..end].fill(false);
            }
        }
        if syntax == Syntax::Rust
            && word == "#"
            && ts.get(i + 1).is_some_and(|t| t.text == "[")
            && let Some(end) = pairs[i + 1]
        {
            allowed[i..=end].fill(false);
            i = end + 1;
            continue;
        }
        // Type arguments and `as` annotations are not runtime values.
        if syntax == Syntax::Rust
            && word == "<"
            && (i == 0 || matches!(ts[i - 1].text, "=" | "(" | "," | "return" | "{"))
            && let Some(end) = angle_end(ts, i)
        {
            allowed[i..end].fill(false);
        }
        if (word == "<" && i > 0 && ts[i - 1].text == "::") || word == "as" {
            let end = if word == "<" {
                angle_end(ts, i).unwrap_or(ts.len())
            } else {
                seek(ts, pairs, i + 1, &[";", ",", ")", "+", "-", "*", "/", "}"])
            };
            allowed[i..end].fill(false);
        }
        i += 1;
    }
    (allowed, bindings)
}

/// Match a conditional question mark at this delimiter level. Object keys,
/// labels and type annotations have no such opener; nested conditional colons
/// consume their own question mark, and grouped values are skipped as a unit.
pub(super) fn conditional_colon(ts: &[Token<'_>], pairs: &[Option<usize>], colon: usize) -> bool {
    let mut at = colon;
    let mut nested = 0;
    while let Some(i) = at.checked_sub(1) {
        at = i;
        match ts[i].text {
            ")" | "]" | "}" => at = pairs[i].unwrap_or(i),
            "(" | "[" | "{" | ";" => return false,
            ":" => nested += 1,
            "?" if nested == 0 => return true,
            "?" => nested -= 1,
            _ => {}
        }
    }
    false
}

/// Recognize a property head followed by a parameter list and method body. Calls such as
/// `return f(x)` have no method body; control statements have no property-head boundary.
fn object_method(
    ts: &[Token<'_>],
    pairs: &[Option<usize>],
    open: usize,
) -> Option<(usize, usize, bool, usize)> {
    let last = open.checked_sub(1)?;
    let computed = ts[last].text == "]";
    let key = if computed {
        pairs[last]?
    } else {
        if !ts[last].literal
            && !ts[last]
                .text
                .chars()
                .next()
                .is_some_and(super::tokens::ident)
        {
            return None;
        }
        last
    };
    let mut start = key;
    while start > 0 && matches!(ts[start - 1].text, "async" | "get" | "set" | "*") {
        start -= 1;
    }
    if start == 0 || !matches!(ts[start - 1].text, "{" | ",") {
        return None;
    }
    // These are statements even when they immediately follow an opening block brace.
    if !computed
        && matches!(
            ts[key].text,
            "if" | "while" | "for" | "switch" | "catch" | "with"
        )
    {
        return None;
    }
    let close = pairs[open]?;
    let body = if ts.get(close + 1)?.text == ":" {
        seek(ts, pairs, close + 2, &["{", ";", ",", "}"])
    } else {
        close + 1
    };
    if ts.get(body)?.text != "{" {
        return None;
    }
    Some((start, key, computed, pairs[body]?))
}
