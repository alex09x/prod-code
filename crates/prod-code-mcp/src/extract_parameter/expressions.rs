/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Syntax, tokens::Token};
use std::ops::Range;

#[path = "precedence.rs"]
mod operators;
#[path = "python_subset.rs"]
mod python_subset;
use operators::{precedence, unary};

/// A precedence parser records complete value nodes, including the left subtree of a
/// left-associative chain. It never treats arbitrary token windows as expressions.
pub(super) fn nodes(
    ts: &[Token<'_>],
    allowed: &[bool],
    syntax: Syntax,
    casts: &[super::c_templates::Cast],
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let python_allowed;
    let mut uncertain = Vec::new();
    let allowed = if syntax == Syntax::Python {
        python_allowed = python_subset::allowed(ts, allowed);
        uncertain = python_allowed.uncertain;
        &python_allowed.allowed[..]
    } else {
        allowed
    };
    if matches!(syntax, Syntax::JavaScript | Syntax::TypeScript) {
        uncertain.extend(ts.iter().enumerate().filter_map(|(i, token)| {
            (allowed[i] && token.literal && super::javascript::interpolated(token.text))
                .then_some(i..i + 1)
        }));
    }
    let mut parser = Parser {
        ts,
        allowed,
        syntax,
        casts,
        nodes: Vec::new(),
    };
    let mut at = 0;
    while at < ts.len() {
        let end = parser.expression(at, 0, 0);
        at = end.unwrap_or(at + 1).max(at + 1);
    }
    (parser.nodes, uncertain)
}

struct Parser<'a, 's> {
    ts: &'a [Token<'s>],
    allowed: &'a [bool],
    syntax: Syntax,
    casts: &'a [super::c_templates::Cast],
    nodes: Vec<Range<usize>>,
}

impl Parser<'_, '_> {
    fn text(&self, i: usize) -> &str {
        self.ts.get(i).map_or("", |t| t.text)
    }
    fn node(&mut self, from: usize, to: usize) {
        self.nodes.push(from..to);
    }

    fn cast(&mut self, start: usize, depth: usize) -> Option<usize> {
        let cast = self
            .casts
            .iter()
            .find(|c| c.range.start == self.ts[start].start)?;
        let from = self.ts.partition_point(|t| t.start < cast.operand.start);
        let to = self.ts.partition_point(|t| t.end <= cast.operand.end);
        let end = self.ts.partition_point(|t| t.end <= cast.range.end);
        let mut operand = Parser {
            ts: &self.ts[from..to],
            allowed: &self.allowed[from..to],
            syntax: self.syntax,
            casts: self.casts,
            nodes: Vec::new(),
        };
        let mut at = 0;
        while at < operand.ts.len() {
            at = operand
                .expression(at, 0, depth + 1)
                .unwrap_or(at + 1)
                .max(at + 1);
        }
        self.nodes.extend(
            operand
                .nodes
                .into_iter()
                .map(|n| from + n.start..from + n.end),
        );
        Some(end)
    }

    fn expression(&mut self, start: usize, min: u8, depth: usize) -> Option<usize> {
        // Bound recursion by structure, returning no node for an unsupported/deep expression.
        if depth > 256 || !self.allowed.get(start).copied().unwrap_or(false) {
            return None;
        }
        let first = self.ts.get(start)?;
        let mut at = if let Some(end) = self.cast(start, depth) {
            end
        } else if let Some(binding) = unary(first.text, self.syntax) {
            self.expression(start + 1, binding, depth + 1)?
        } else if matches!(first.text, "(" | "[") {
            let close = if first.text == "(" { ")" } else { "]" };
            let mut end = start + 1;
            while self.text(end) != close {
                end = self.expression(end, 0, depth + 1)?;
                if matches!(self.text(end), "," | ";") {
                    end += 1;
                } else if self.text(end) != close {
                    return None;
                }
            }
            end + 1
        } else if self.syntax == Syntax::Cpp
            && first.literal
            && super::literals::cpp_string(first.text)
        {
            // Translation-phase string concatenation is one primary expression.
            // Never record either component as an independently selectable node.
            let mut end = start + 1;
            while self.allowed.get(end).copied().unwrap_or(false)
                && self.ts[end].literal
                && super::literals::cpp_string(self.text(end))
            {
                end += 1;
            }
            end
        } else if first.literal
            || (first.text.chars().next().is_some_and(super::tokens::ident) && !keyword(first.text))
        {
            start + 1
        } else {
            return None;
        };
        self.node(start, at);
        loop {
            if !self.allowed.get(at).copied().unwrap_or(false) {
                break;
            }
            if matches!(self.text(at), "(" | "[") {
                let close = if self.text(at) == "(" { ")" } else { "]" };
                let mut end = at + 1;
                while self.text(end) != close {
                    let Some(next) = self.expression(end, 0, depth + 1) else {
                        return Some(at);
                    };
                    end = next;
                    if self.text(end) == "," {
                        end += 1;
                    } else if self.text(end) != close {
                        return Some(at);
                    }
                }
                at = end + 1;
            } else if self.text(at) == "::" && self.text(at + 1) == "<" {
                let Some(end) = super::contexts::angle_end(self.ts, at + 1) else {
                    break;
                };
                at = end;
            } else if matches!(self.text(at), "." | "?." | "::" | "->")
                && self.ts.get(at + 1).is_some()
            {
                at += 2;
            } else if self.text(at) == "?"
                && matches!(
                    self.syntax,
                    Syntax::JavaScript | Syntax::TypeScript | Syntax::C | Syntax::Cpp
                )
            {
                // Conditional values sit below logical OR and above assignment.
                // Record each branch as a value, as well as the whole conditional.
                if min > 1 {
                    break;
                }
                let yes = self.expression(at + 1, 0, depth + 1)?;
                if self.text(yes) != ":" {
                    break;
                }
                at = self.expression(yes + 1, 1, depth + 1)?;
            } else if self.text(at) == "?" {
                at += 1;
            } else if let Some((left, right)) = precedence(self.text(at), self.syntax) {
                if left < min {
                    break;
                }
                if self.syntax == Syntax::Python
                    && self.text(at) == "not"
                    && self.text(at + 1) != "in"
                {
                    break;
                }
                let width = if self.syntax == Syntax::Python
                    && matches!(
                        (self.text(at), self.text(at + 1)),
                        ("is", "not") | ("not", "in")
                    ) {
                    2
                } else {
                    1
                };
                let Some(end) = self.expression(at + width, right, depth + 1) else {
                    break;
                };
                at = end;
                // Comparisons are one Python chain, not nested binary comparisons.
                // Likewise BoolOp holds all consecutive operands of `and` / `or`.
                if self.syntax == Syntax::Python
                    && matches!(left, 2 | 3 | 5)
                    && precedence(self.text(at), self.syntax).is_some_and(|(next, _)| next == left)
                {
                    continue;
                }
            } else {
                break;
            }
            self.node(start, at);
        }
        Some(at)
    }
}

fn keyword(s: &str) -> bool {
    matches!(
        s,
        "return"
            | "yield"
            | "throw"
            | "if"
            | "else"
            | "while"
            | "for"
            | "match"
            | "switch"
            | "case"
            | "break"
            | "continue"
            | "let"
            | "var"
            | "const"
            | "fn"
            | "function"
            | "def"
            | "in"
            | "as"
            | "mut"
            | "ref"
            | "pub"
            | "async"
            | "move"
    )
}
