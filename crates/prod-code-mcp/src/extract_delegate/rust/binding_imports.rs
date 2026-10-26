/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::binding_path::Binding;
use crate::fixture::builder::lexer::{Kind, Source, bare};
use anyhow::{Context, Result};

pub(super) fn visible(
    source: &Source<'_>,
    text: &str,
    occurrence: usize,
    stack: &[usize],
    bound: &str,
    module: bool,
) -> Result<Vec<Binding>> {
    let modules = crate::parameter_object::rust_modules(text, occurrence);
    let mut imports = Vec::new();
    let mut enabled = Some(true);
    let mut depth = 0usize;
    let mut at = 0;
    while at < source.tokens.len() {
        if source.is(at, "#") && source.is(at + 1, "[") {
            let close = source
                .close_of(at + 1)
                .context("unclosed import attribute")?;
            enabled = conjunction(enabled, attribute_enabled(source, at + 2, close));
            at = close + 1;
        } else if source.is(at, "{") && !stack.contains(&at) {
            at = source.close_of(at).context("unclosed scope")? + 1;
            enabled = Some(true);
        } else if module
            && source.is(at, "mod")
            && bare(source.t(at + 1)) == bare(bound)
            && enabled != Some(false)
            && crate::parameter_object::rust_modules(text, source.tokens[at].start) == modules
        {
            // A lexical module root wins over an identically named extern
            // prelude entry. Anchor it to self before resolving target identity.
            imports.push(Binding {
                prefix: vec!["self".into(), source.t(at + 1).into()],
                conditional: enabled.is_none(),
                explicit: true,
                depth,
                scope: Some(source.tokens[at].start),
            });
            at += 2;
        } else if module && source.is(at, "extern") && source.is(at + 1, "crate") {
            let name = source.t(at + 2);
            let alias = if source.is(at + 3, "as") {
                source.t(at + 4)
            } else {
                name
            };
            if bare(alias) == bare(bound)
                && enabled != Some(false)
                && crate::parameter_object::rust_modules(text, source.tokens[at].start) == modules
            {
                let prefix = if name == "self" {
                    vec!["crate".into()]
                } else {
                    vec![String::new(), name.into()]
                };
                imports.push(Binding {
                    prefix,
                    conditional: enabled.is_none(),
                    explicit: true,
                    depth,
                    scope: Some(source.tokens[at].start),
                });
            }
            at += 3;
        } else if source.is(at, "use") {
            let end = (at + 1..source.tokens.len())
                .find(|i| source.is(*i, ";"))
                .context("unclosed import")?;
            if enabled != Some(false)
                && crate::parameter_object::rust_modules(text, source.tokens[at].start) == modules
            {
                let mut found = Vec::new();
                use_tree(source, at + 1, end, &[], bound, module, &mut found);
                for mut import in found {
                    import.depth = depth;
                    import.conditional = enabled.is_none();
                    import.scope = Some(source.tokens[at].start);
                    imports.push(import);
                }
            }
            at = end + 1;
            enabled = Some(true);
        } else {
            if source.is(at, "{") {
                depth += 1;
            } else if source.is(at, "}") {
                depth = depth.saturating_sub(1);
            }
            if source.is(at, ";") || source.is(at, "{") || source.is(at, "}") {
                enabled = Some(true);
            }
            at += 1;
        }
    }
    // Definite explicit imports shadow outer imports and globs; conditional
    // imports retain alternatives for the occurrence's semantic owner hover.
    if let Some(depth) = imports
        .iter()
        .filter(|i| i.explicit && !i.conditional)
        .map(|i| i.depth)
        .max()
    {
        imports.retain(|i| i.depth > depth || (i.depth == depth && i.explicit && !i.conditional));
    }
    imports.sort_by_key(|i| std::cmp::Reverse((i.depth, i.explicit)));
    Ok(imports)
}

fn conjunction(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

fn attribute_enabled(source: &Source<'_>, start: usize, end: usize) -> Option<bool> {
    if source.is(start, "cfg") && source.is(start + 1, "(") {
        return super::helper_path::constant_cfg(source, start + 2, end - 1);
    }
    if !source.is(start, "cfg_attr") {
        return Some(true);
    }
    let close = source.close_of(start + 1)?;
    if close + 1 != end {
        return None;
    }
    let mut at = start + 2;
    let mut item = at;
    let mut predicate = None;
    let mut enabled = Some(true);
    while at <= close {
        if at == close || source.is(at, ",") {
            if predicate.is_none() {
                predicate = Some(super::helper_path::constant_cfg(source, item, at));
            } else if item < at {
                enabled = conjunction(enabled, attribute_enabled(source, item, at));
            }
            item = at + 1;
        } else if ["(", "[", "{"].iter().any(|s| source.is(at, s)) {
            at = source.close_of(at)?;
        }
        at += 1;
    }
    match predicate? {
        Some(false) => Some(true),
        Some(true) => enabled,
        None if enabled == Some(true) => Some(true),
        None => None,
    }
}

fn use_tree(
    source: &Source<'_>,
    mut at: usize,
    end: usize,
    parent: &[String],
    bound: &str,
    module: bool,
    out: &mut Vec<Binding>,
) {
    let mut prefix = parent.to_vec();
    while at < end {
        if source.is(at, "{") {
            let Some(close) = source.close_of(at) else {
                return;
            };
            use_tree(source, at + 1, close, &prefix, bound, module, out);
            at = close;
        } else if source.is(at, ",") {
            prefix = parent.to_vec();
        } else if source.is(at, "::") && prefix.is_empty() {
            // The empty first segment is the absolute extern-prelude marker.
            prefix.push(String::new());
        } else if source.is(at, "*") {
            out.push(Binding {
                prefix: prefix.clone(),
                conditional: false,
                explicit: false,
                depth: 0,
                scope: None,
            });
        } else if source.tokens[at].kind == Kind::Ident {
            let name = source.t(at);
            let alias = if source.is(at + 1, "as") {
                source.t(at + 2)
            } else {
                name
            };
            if !source.is(at + 1, "::") && bare(alias) == bare(bound) {
                let mut path = prefix.clone();
                if module && name != "self" {
                    path.push(name.to_owned());
                }
                out.push(Binding {
                    prefix: path,
                    conditional: false,
                    explicit: true,
                    depth: 0,
                    scope: None,
                });
            }
            prefix.push(name.to_owned());
            if source.is(at + 1, "as") {
                at += 2;
            }
        }
        at += 1;
    }
}
