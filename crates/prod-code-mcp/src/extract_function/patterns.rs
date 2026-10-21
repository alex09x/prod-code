/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::tokens::{Token, tokens};

pub(super) struct LocalBinding<'a> {
    name: &'a str,
    declaration: usize,
    scope: std::ops::Range<usize>,
}

impl LocalBinding<'_> {
    pub(super) fn covers(&self, name: &str, at: usize) -> bool {
        self.name == name && (self.declaration == at || self.scope.contains(&at))
    }
}

struct Syntax<'a> {
    text: &'a str,
    tokens: Vec<(Token, usize, usize)>,
    pairs: Vec<Option<usize>>,
    blocks: Vec<usize>,
}

impl<'a> Syntax<'a> {
    fn new(text: &'a str) -> Self {
        let tokens = tokens(text);
        let mut pairs = vec![None; tokens.len()];
        let mut blocks = vec![tokens.len(); tokens.len()];
        let mut stack = Vec::new();
        for (i, (_, s, e)) in tokens.iter().enumerate() {
            blocks[i] = stack
                .iter()
                .rev()
                .copied()
                .find(|&j| {
                    let (_, s, e) = tokens[j];
                    &text[s..e] == "{"
                })
                .unwrap_or(tokens.len());
            match &text[*s..*e] {
                "(" | "[" | "{" => stack.push(i),
                ")" | "]" | "}" => {
                    if let Some(open) = stack.pop() {
                        pairs[open] = Some(i);
                        pairs[i] = Some(open);
                    }
                }
                _ => {}
            }
        }
        Self {
            text,
            tokens,
            pairs,
            blocks,
        }
    }

    fn word(&self, i: usize) -> &'a str {
        self.tokens
            .get(i)
            .map_or("", |(_, s, e)| &self.text[*s..*e])
    }

    fn at(&self, i: usize) -> usize {
        self.tokens.get(i).map_or(self.text.len(), |t| t.1)
    }

    fn next(&self, i: usize) -> usize {
        self.pairs[i]
            .filter(|&end| end > i)
            .map_or(i + 1, |end| end + 1)
    }

    fn find(&self, mut i: usize, stop: &[&str]) -> usize {
        while i < self.tokens.len() && !stop.contains(&self.word(i)) {
            i = self.next(i);
        }
        i
    }

    fn block_end(&self, i: usize) -> usize {
        self.pairs
            .get(self.blocks[i])
            .and_then(|end| *end)
            .unwrap_or(self.tokens.len())
    }

    fn expression_end(&self, i: usize) -> usize {
        if self.word(i) == "{" {
            self.next(i)
        } else {
            self.find(i, &[",", ";", ")", "]", "}"])
        }
    }

    fn pattern(
        &self,
        start: usize,
        end: usize,
        scope: std::ops::Range<usize>,
        out: &mut Vec<LocalBinding<'a>>,
    ) {
        // Strip a top-level type annotation, but keep struct field labels inside braces.
        let mut typed = start;
        while typed < end {
            if self.word(typed) == ":"
                && self.word(typed + 1) != ":"
                && (typed == start || self.word(typed - 1) != ":")
            {
                break;
            }
            typed = self.next(typed);
        }
        let end = typed.min(end);
        for i in start..end {
            let name = self.word(i);
            let field_or_path = i + 1 < end && self.word(i + 1) == ":"
                || i >= start + 2 && self.word(i - 1) == ":" && self.word(i - 2) == ":";
            if self.tokens[i].0 == Token::Word
                && !field_or_path
                && !matches!(name, "ref" | "mut" | "_" | "true" | "false" | "self")
                && !name.starts_with(|c: char| c.is_uppercase() || c == '\'')
            {
                out.push(LocalBinding {
                    name,
                    declaration: self.at(i),
                    scope: scope.clone(),
                });
            }
        }
    }

    fn body(&self, start: usize) -> usize {
        let mut open = self.find(start, &["{"]);
        // A block-valued iterable/scrutinee can precede the actual body.
        while open < self.tokens.len() && self.word(self.next(open)) == "{" {
            open = self.next(open);
        }
        open
    }
}

/// Pattern declarations with their lexical extent. A name bound in one arm/loop/closure
/// must not hide a genuinely uncaptured use in its initializer or a different scope.
pub(super) fn local_bindings(text: &str) -> Vec<LocalBinding<'_>> {
    let syntax = Syntax::new(text);
    let s = &syntax;
    let mut out = Vec::new();
    for i in 0..s.tokens.len() {
        match s.word(i) {
            "let" | "for" => {
                let is_for = s.word(i) == "for";
                let end = s.find(i + 1, &[if is_for { "in" } else { "=" }, ";"]);
                if end == s.tokens.len() || is_for && s.word(end) == ";" {
                    continue;
                }
                let conditional = !is_for
                    && (0..i)
                        .rev()
                        .take_while(|&j| !matches!(s.word(j), ";" | "{" | "}"))
                        .any(|j| matches!(s.word(j), "if" | "while"));
                let (from, to) = if is_for || conditional {
                    let open = s.body(end + 1);
                    let Some(close) = s.pairs.get(open).and_then(|p| *p) else {
                        continue;
                    };
                    let mut from = end + 1;
                    while from < open {
                        if s.word(from) == "&" && s.word(from + 1) == "&" {
                            break;
                        }
                        from = s.next(from);
                    }
                    (if conditional { from } else { open + 1 }, close)
                } else {
                    let semicolon = if s.word(end) == ";" {
                        end
                    } else {
                        s.find(end + 1, &[";"])
                    };
                    (semicolon + 1, s.block_end(i))
                };
                s.pattern(i + 1, end, s.at(from)..s.at(to), &mut out);
            }
            "|" => {
                // Only an expression boundary can introduce closure parameters. In particular,
                // an or-pattern or bitwise-or expression does not declare a closure binding.
                let prev = i.checked_sub(1).map_or("", |j| s.word(j));
                if !matches!(
                    prev,
                    "" | "="
                        | "("
                        | "["
                        | ","
                        | "{"
                        | ";"
                        | ":"
                        | "move"
                        | "async"
                        | "return"
                        | "break"
                        | ">"
                ) {
                    continue;
                }
                let close = s.find(i + 1, &["|"]);
                if close == s.tokens.len() {
                    continue;
                }
                let end = s.expression_end(close + 1);
                let scope = s.at(close + 1)..s.at(end);
                let mut param = i + 1;
                while param < close {
                    let end = s.find(param, &[",", "|"]).min(close);
                    s.pattern(param, end, scope.clone(), &mut out);
                    param = end + 1;
                }
            }
            "match" => {
                let open = s.body(i + 1);
                let Some(close) = s.pairs.get(open).and_then(|p| *p) else {
                    continue;
                };
                let mut arm = open + 1;
                while arm < close {
                    let arrow = s.find(arm, &["="]).min(close);
                    if s.word(arrow + 1) != ">" {
                        break;
                    }
                    let guard = s.find(arm, &["if"]).min(arrow);
                    let end = s.expression_end(arrow + 2).min(close);
                    s.pattern(arm, guard, s.at(guard)..s.at(end), &mut out);
                    arm = end + usize::from(s.word(end) == ",");
                }
            }
            _ => {}
        }
    }
    out
}
