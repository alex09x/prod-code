/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::fixture::builder::lexer::{Kind, Source};
use anyhow::{Context, Result};

#[derive(Clone)]
pub(super) struct Binding {
    pub prefix: Vec<String>,
    pub conditional: bool,
    pub explicit: bool,
    pub depth: usize,
    // Imports bind their dependencies where declared, independently of later
    // shadowing at a constructor or pattern that uses the imported name.
    pub scope: Option<usize>,
}

pub(super) fn prefixes(text: &str, occurrence: usize, owner: &str) -> Result<Vec<Binding>> {
    let source = Source::new(text)?;
    let mut stack = scope_at(&source, occurrence);
    let mut binding_occurrence = occurrence;
    let mut bound = owner;
    let mut qualified = None;
    if let Some(open) = stack.last().copied() {
        let mut at = open.saturating_sub(1);
        if source.is(at, "Self") {
            // Self is bound by the impl's type, never by imports in the method
            // or by a glob that happens to expose the same physical source file.
            at = impl_type(&source, &stack)
                .context("cannot resolve the enclosing impl type for Self; nothing was planned")?;
            binding_occurrence = source.tokens[at].start;
            stack.retain(|open| source.tokens[*open].start < binding_occurrence);
        }
        if source.tokens[at].kind == Kind::Ident {
            bound = source.t(at);
            let end = at + 1;
            while at >= 2 && source.is(at - 1, "::") && path_segment(&source, at - 2) {
                at -= 2;
            }
            if at + 1 < end {
                let mut prefix: Vec<_> = (at..end - 1)
                    .step_by(2)
                    .map(|i| source.t(i).to_owned())
                    .collect();
                if at > 0 && source.is(at - 1, "::") {
                    prefix.insert(0, String::new());
                }
                qualified = Some(prefix);
            }
        }
    }
    let imports = if let Some(prefix) = qualified {
        vec![Binding {
            prefix,
            conditional: false,
            explicit: true,
            depth: stack.len(),
            scope: None,
        }]
    } else {
        super::binding_imports::visible(&source, text, binding_occurrence, &stack, bound, false)?
    };
    let mut result = Vec::new();
    for import in imports {
        expand(
            &source,
            text,
            binding_occurrence,
            &stack,
            import,
            &mut Vec::new(),
            &mut result,
        )?;
    }
    Ok(result)
}

fn scope_at(source: &Source<'_>, occurrence: usize) -> Vec<usize> {
    let mut stack = Vec::new();
    for (i, _) in source
        .tokens
        .iter()
        .enumerate()
        .take_while(|(_, t)| t.start < occurrence)
    {
        if source.is(i, "{") {
            stack.push(i);
        }
        if source.is(i, "}") {
            stack.pop();
        }
    }
    stack
}

fn path_segment(source: &Source<'_>, at: usize) -> bool {
    source.tokens[at].kind == Kind::Ident
        && ![
            "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
            "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in",
            "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub",
            "ref", "return", "static", "struct", "trait", "true", "try", "type", "typeof",
            "unsafe", "unsized", "use", "virtual", "where", "while", "yield", "_",
        ]
        .contains(&source.t(at))
}

// Follow every import in its own declaration scope. The initial qualified
// constructor uses its occurrence scope (the impl header for Self).
fn expand(
    source: &Source<'_>,
    text: &str,
    occurrence: usize,
    stack: &[usize],
    binding: Binding,
    seen: &mut Vec<(String, usize)>,
    out: &mut Vec<Binding>,
) -> Result<()> {
    let Some(root) = binding.prefix.first() else {
        out.push(binding);
        return Ok(());
    };
    if ["", "crate", "self", "super"].contains(&root.as_str()) {
        out.push(binding);
        return Ok(());
    }
    let declaration_stack;
    let (occurrence, stack) = if let Some(declaration) = binding.scope {
        declaration_stack = scope_at(source, declaration);
        (declaration, declaration_stack.as_slice())
    } else {
        (occurrence, stack)
    };
    let identity = (root.clone(), occurrence);
    anyhow::ensure!(
        !seen.contains(&identity) && seen.len() < 32,
        "cyclic module import; nothing was planned"
    );
    let aliases = super::binding_imports::visible(source, text, occurrence, stack, root, true)?;
    let aliases: Vec<_> = aliases.into_iter().filter(|a| a.explicit).collect();
    if aliases.is_empty() {
        out.push(binding);
        return Ok(());
    }
    if aliases.iter().all(|alias| alias.conditional) {
        // An inactive conditional import leaves the extern prelude available.
        // Keep its absolute spelling so emitting the helper cannot accidentally
        // resolve back through the conditional local alias.
        let mut external = binding.clone();
        external.prefix.insert(0, String::new());
        out.push(external);
    }
    seen.push(identity);
    for mut alias in aliases {
        alias.prefix.extend_from_slice(&binding.prefix[1..]);
        alias.conditional |= binding.conditional;
        expand(source, text, occurrence, stack, alias, seen, out)?;
    }
    seen.pop();
    Ok(())
}

fn impl_type(source: &Source<'_>, stack: &[usize]) -> Option<usize> {
    for &open in stack.iter().rev().skip(1) {
        let Some(start) = impl_header_start(source, open) else {
            continue;
        };
        // Only an impl item can bind Self. An impl Trait argument or return
        // type belongs to a function header, even when its body contains Self.
        let mut implementation = start;
        while source.is(implementation, "#") && source.is(implementation + 1, "[") {
            implementation = source.close_of(implementation + 1)? + 1;
        }
        while ["unsafe", "default"]
            .iter()
            .any(|s| source.is(implementation, s))
        {
            implementation += 1;
        }
        if !source.is(implementation, "impl") {
            continue;
        }
        let mut at = implementation + 1;
        let mut angles = 0usize;
        let mut target = None;
        while at < open {
            if angles > 0 && ["(", "[", "{"].iter().any(|s| source.is(at, s)) {
                // Const expressions may contain comparisons, nested blocks and
                // semicolons. None of their tokens belong to the type path.
                at = source.close_of(at)?;
            } else if source.is(at, "<") {
                angles += 1;
            } else if source.is(at, ">") {
                angles = angles.checked_sub(1)?;
            } else if angles == 0 {
                if source.is(at, "where") {
                    break;
                }
                if source.is(at, "for") {
                    target = None;
                } else if source.is(at, "(") {
                    let close = source.close_of(at)?;
                    if target.is_some() || !self_type_end(source, close + 1, open) {
                        return None;
                    }
                } else if source.is(at, ")") {
                    if target.is_none() || !self_type_end(source, at + 1, open) {
                        return None;
                    }
                } else if source.tokens[at].kind == Kind::Ident {
                    target = Some(at);
                } else if !source.is(at, "::") {
                    return None;
                }
            }
            at += 1;
        }
        return (angles == 0).then_some(target).flatten();
    }
    None
}

fn self_type_end(source: &Source<'_>, next: usize, open: usize) -> bool {
    next == open || source.is(next, "where") || source.is(next, ")")
}

fn impl_header_start(source: &Source<'_>, mut at: usize) -> Option<usize> {
    let mut start = 0;
    while at > 0 {
        at -= 1;
        if source.is(at, ">") {
            if let Some(open) = generic_open(source, at) {
                at = open;
            }
        } else if source.is(at, ")") || source.is(at, "]") {
            // Skip complete header groups before considering item boundaries.
            at = source.open_of(at)?;
        } else if ["{", "}", ";"].iter().any(|s| source.is(at, s)) {
            start = at + 1;
            break;
        }
    }
    if start == 0 || source.is(start - 1, "{") {
        while source.is(start, "#") && source.is(start + 1, "!") && source.is(start + 2, "[") {
            start = source.close_of(start + 2)? + 1;
        }
    }
    Some(start)
}

fn generic_open(source: &Source<'_>, close: usize) -> Option<usize> {
    if source.is(close + 1, "=") {
        return None;
    }
    let mut depth = 1usize;
    let mut at = close;
    while at > 0 {
        at -= 1;
        if source.is(at, ")") || source.is(at, "]") {
            at = source.open_of(at)?;
        } else if source.is(at, "}") {
            let open = source.open_of(at)?;
            if !["<", ",", "="]
                .iter()
                .any(|s| open > 0 && source.is(open - 1, s))
                || ![">", ","].iter().any(|s| source.is(at + 1, s))
            {
                return None;
            }
            at = open;
        } else if source.is(at, "{") || source.is(at, ";") {
            return None;
        } else if source.is(at, ">") && !source.is(at + 1, "=") {
            depth += 1;
        } else if source.is(at, "<") && !source.is(at + 1, "=") {
            depth -= 1;
            if depth == 0 {
                return (at > 0 && source.tokens[at - 1].kind != Kind::Literal).then_some(at);
            }
        }
    }
    None
}
