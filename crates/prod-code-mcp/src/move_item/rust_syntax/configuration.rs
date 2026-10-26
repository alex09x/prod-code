/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use ra_ap_syntax::{AstNode, AstToken, SyntaxToken, ast};

/// A structural sufficient proof, deliberately not a solver for arbitrary cfg expressions.
/// Unknown predicates remain opaque: equal predicates and conjunctions still prove useful
/// conditional moves without assuming the workstation's feature or target configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Condition {
    #[default]
    True,
    False,
    Atom(String),
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
}

impl Condition {
    fn all(parts: Vec<Self>) -> Self {
        if parts.contains(&Self::False) {
            return Self::False;
        }
        let parts: Vec<_> = parts.into_iter().filter(|p| *p != Self::True).collect();
        match parts.len() {
            0 => Self::True,
            1 => parts.into_iter().next().unwrap(),
            _ => Self::All(parts),
        }
    }

    fn any(parts: Vec<Self>) -> Self {
        if parts.contains(&Self::True) {
            return Self::True;
        }
        let parts: Vec<_> = parts.into_iter().filter(|p| *p != Self::False).collect();
        match parts.len() {
            0 => Self::False,
            1 => parts.into_iter().next().unwrap(),
            _ => Self::Any(parts),
        }
    }

    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Not(inner) => *inner,
            other => Self::Not(Box::new(other)),
        }
    }

    pub fn and(&self, other: &Self) -> Self {
        Self::all(vec![self.clone(), other.clone()])
    }

    pub fn implies(&self, destination: &Self) -> bool {
        self == destination
            || *self == Self::False
            || *destination == Self::True
            || match (self, destination) {
                (_, Self::All(parts)) => parts.iter().all(|p| self.implies(p)),
                (Self::Any(parts), _) => parts.iter().all(|p| p.implies(destination)),
                (Self::All(parts), _) => parts.iter().any(|p| p.implies(destination)),
                (_, Self::Any(parts)) => parts.iter().any(|p| self.implies(p)),
                _ => false,
            }
    }
}

fn tokens(attr: &ast::Attr) -> Vec<SyntaxToken> {
    let tokens: Vec<_> = attr
        .syntax()
        .descendants_with_tokens()
        .filter_map(|n| n.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    // Use the complete attribute's lexer tokens, including the outer cfg_attr arguments.
    // The pinned syntax represents meta arguments structurally; its first TokenTree may
    // instead be the nested predicate's `all()` or a macro in the attribute value.
    let start = tokens
        .iter()
        .position(|t| t.text() == "[")
        .map_or(0, |n| n + 1);
    let end = tokens
        .iter()
        .rposition(|t| t.text() == "]")
        .unwrap_or(tokens.len());
    tokens[start..end].to_vec()
}

fn arguments(tokens: &[SyntaxToken]) -> Option<&[SyntaxToken]> {
    (tokens.get(1)?.text() == "(" && tokens.last()?.text() == ")")
        .then(|| &tokens[2..tokens.len() - 1])
}

fn segments(tokens: &[SyntaxToken]) -> Vec<&[SyntaxToken]> {
    let mut depth = 0;
    let mut start = 0;
    let mut out = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.text() {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth -= 1,
            "," if depth == 0 => {
                out.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < tokens.len() {
        out.push(&tokens[start..]);
    }
    out
}

fn predicate(tokens: &[SyntaxToken]) -> Condition {
    if let Some(args) = arguments(tokens) {
        let parts = segments(args)
            .into_iter()
            .map(predicate)
            .collect::<Vec<_>>();
        match tokens[0].text() {
            "all" => return Condition::all(parts),
            "any" => return Condition::any(parts),
            "not" if parts.len() == 1 => return parts.into_iter().next().unwrap().not(),
            _ => {}
        }
    }
    Condition::Atom(tokens.iter().map(|t| t.text()).collect())
}

#[derive(Default)]
pub(super) struct Metadata {
    pub path: Option<String>,
    pub alternatives: Vec<String>,
    pub unresolved: bool,
    pub condition: Condition,
    pub literals: Vec<std::ops::Range<usize>>,
}

impl Metadata {
    fn scan(&mut self, meta: &[SyntaxToken], when: Condition) {
        if when == Condition::False || meta.is_empty() {
            return;
        }
        match meta[0].text() {
            "path" => {
                let literal = (meta.len() == 3 && meta[1].text() == "=")
                    .then(|| ast::String::cast(meta[2].clone()))
                    .flatten()
                    .and_then(|s| s.value().ok().map(|s| s.into_owned()));
                if let Some(path) = literal {
                    if when == Condition::True {
                        let span = meta[2].text_range();
                        self.literals
                            .push(usize::from(span.start())..usize::from(span.end()));
                    }
                    if when == Condition::True && self.path.is_none() {
                        self.path = Some(path);
                    } else if self.path.as_ref() != Some(&path) {
                        self.alternatives.push(path);
                    }
                } else {
                    self.unresolved = true;
                }
            }
            "cfg" => {
                if let Some(args) = arguments(meta) {
                    self.condition = self
                        .condition
                        .and(&Condition::any(vec![when.not(), predicate(args)]));
                } else {
                    self.unresolved = true;
                }
            }
            "cfg_attr" => {
                if let Some(args) = arguments(meta) {
                    let parts = segments(args);
                    if let Some((cfg, attrs)) = parts.split_first() {
                        let when = when.and(&predicate(cfg));
                        for attr in attrs {
                            self.scan(attr, when.clone());
                        }
                    }
                } else {
                    self.unresolved = true;
                }
            }
            _ => {}
        }
    }
}

pub(super) fn metadata(attrs: impl Iterator<Item = ast::Attr>) -> Metadata {
    let mut result = Metadata::default();
    for attr in attrs {
        result.scan(&tokens(&attr), Condition::True);
    }
    result
}

pub(super) fn module(module: &ast::Module) -> Metadata {
    metadata(
        module
            .syntax()
            .children()
            .filter_map(ast::Attr::cast)
            .chain(
                module
                    .item_list()
                    .into_iter()
                    .flat_map(|list| list.syntax().children().filter_map(ast::Attr::cast)),
            ),
    )
}

pub(crate) fn active_path_literals(attr: ast::Attr) -> Vec<std::ops::Range<usize>> {
    metadata(std::iter::once(attr)).literals
}

pub(crate) fn scope_condition(
    text: &str,
    scope: std::ops::Range<usize>,
    edition: ra_ap_syntax::Edition,
) -> Condition {
    let tree = super::parse(text, edition);
    tree.syntax()
        .descendants()
        .find(|n| {
            super::range(n) == scope
                && (ast::SourceFile::can_cast(n.kind()) || ast::ItemList::can_cast(n.kind()))
        })
        .map(|node| metadata(node.children().filter_map(ast::Attr::cast)).condition)
        .unwrap_or_default()
}
