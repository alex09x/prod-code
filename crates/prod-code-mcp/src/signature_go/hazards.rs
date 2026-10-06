/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::Param;
use crate::signature_go::parse::body_open;
use crate::signature_go::syntax::{canonical, is_ident, list_text};
use crate::signature_go::text::{closing, skip_opaque};
use crate::signature_go::types::{ArgKind, Call, GOPLS_VERSION, GoParam, refusal};
use anyhow::{Context, Result};

/// The new parameters as indices into `declared`: each declared parameter at most once, in the
/// new order; one left out is to be removed.
pub(crate) fn permutation(declared: &[GoParam], request: &[Param]) -> Result<Vec<usize>> {
    let mut order = Vec::with_capacity(request.len());
    for want in request {
        let name = match want {
            Param::Keep(name) => name,
            Param::Add { name, .. } => {
                return Err(refusal(format!(
                    "adding the parameter `{name}` is not supported: gopls {GOPLS_VERSION} \
                     refuses new parameters, and passing a new argument at every call site is not \
                     done by hand here"
                )));
            }
        };
        let at = declared
            .iter()
            .position(|d| &d.name == name)
            .with_context(|| {
                format!(
                    "no parameter named `{name}`; the declaration takes {}",
                    list_text(declared)
                )
            })?;
        anyhow::ensure!(!order.contains(&at), "`{name}` is listed twice");
        order.push(at);
    }
    anyhow::ensure!(
        order.len() < declared.len() || !is_subsequence(&order),
        "the requested order is the declared one; there is nothing to change"
    );
    Ok(order)
}

/// Whether the kept parameters keep their declared order.
pub(crate) fn is_subsequence(order: &[usize]) -> bool {
    order.windows(2).all(|w| w[0] < w[1])
}

/// The removed parameters' names, for a message: "`a`, `b`".
pub(crate) fn removed_names(declared: &[GoParam], removed: &[usize]) -> String {
    removed
        .iter()
        .map(|&i| format!("`{}`", declared[i].name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A call's arguments after the change: the kept ones in the new order. `arity` is the number of
/// parameters declared before the change, not the number kept. For a variadic function the
/// arguments from its last parameter on are one tail: kept last where the parameter is kept,
/// spread or not, and gone with it where it is removed.
pub(crate) fn permuted(
    args: &[String],
    order: &[usize],
    arity: usize,
    variadic: bool,
) -> Vec<String> {
    let tail = if variadic { arity - 1 } else { usize::MAX };
    let mut out = Vec::with_capacity(args.len());
    for &i in order {
        if i == tail {
            out.extend(args.iter().skip(tail).cloned());
        } else if let Some(a) = args.get(i) {
            out.push(a.clone());
        }
    }
    out
}

/// What the change would do to the program at run time, one line per place: a call whose
/// arguments cannot be matched to the declared parameters, two arguments evaluated the other way
/// round when either can have an effect the other sees, and a dropped argument whose evaluation
/// could do anything at all.
pub(crate) fn effect_hazards(
    name: &str,
    declared: &[GoParam],
    order: &[usize],
    variadic: bool,
    calls: &[Call],
) -> Vec<String> {
    let mut swapped = Vec::new();
    for (p, &later) in order.iter().enumerate() {
        for &earlier in &order[p + 1..] {
            if earlier < later {
                swapped.push((earlier, later));
            }
        }
    }
    let arity = declared.len();
    let mut out = Vec::new();
    for call in calls {
        let spread = call
            .args
            .last()
            .is_some_and(|a| a.trim_end().ends_with("..."));
        let fits = if variadic {
            call.args.len() + 1 >= arity && (!spread || call.args.len() == arity)
        } else {
            call.args.len() == arity && !spread
        };
        if !fits {
            out.push(format!(
                "{}: the call passes {} argument(s) and `{name}` declares {arity}, so what the \
                 reorder does to it cannot be checked",
                call.at,
                call.args.len()
            ));
            continue;
        }
        let kinds: Vec<ArgKind> = call.args.iter().map(|a| classify(a)).collect();
        for &(i, j) in &swapped {
            let independent = kinds[i] == ArgKind::Literal
                || kinds[j] == ArgKind::Literal
                || (kinds[i] == ArgKind::Place && kinds[j] == ArgKind::Place);
            if !independent {
                out.push(format!(
                    "{}: `{}` and `{}` would be evaluated in the opposite order",
                    call.at,
                    call.args[i].trim(),
                    call.args[j].trim()
                ));
            }
        }
        for (i, param) in declared.iter().enumerate() {
            if order.contains(&i) {
                continue;
            }
            // The removed variadic parameter takes the whole tail with it, a spread slice too.
            let dropped: Vec<&String> = if variadic && i == arity - 1 {
                call.args.iter().skip(i).collect()
            } else {
                call.args.get(i).into_iter().collect()
            };
            for arg in dropped {
                let value = arg.trim();
                if !droppable(value.strip_suffix("...").unwrap_or(value)) {
                    out.push(format!(
                        "{}: `{value}` is passed for the removed `{}` and would no longer be \
                         evaluated; only a literal or a plain variable can be dropped",
                        call.at, param.name
                    ));
                }
            }
        }
    }
    out
}

/// Whether evaluating the argument does nothing the program could notice: a number, string, rune
/// or function literal, or a plain variable or its address. Not a selector, which can dereference
/// a nil pointer, nor an index, a call, a receive, a conversion or an operator.
pub(crate) fn droppable(arg: &str) -> bool {
    let text = canonical(arg);
    let mut e = text.as_str();
    while e.starts_with('(') && closing(e, 0) == Some(e.len() - 1) {
        e = &e[1..e.len() - 1];
    }
    is_literal(e) || is_ident(e.strip_prefix('&').unwrap_or(e))
}

pub(crate) fn classify(arg: &str) -> ArgKind {
    let text = canonical(arg);
    let mut e = text.as_str();
    while e.starts_with('(') && closing(e, 0) == Some(e.len() - 1) {
        e = &e[1..e.len() - 1];
    }
    if is_literal(e) {
        ArgKind::Literal
    } else if is_place(e.strip_prefix('&').unwrap_or(e)) {
        ArgKind::Place
    } else {
        ArgKind::Effectful
    }
}

/// A literal by its syntax alone. Not `true`, `false` or `nil`, which a scope can redeclare.
pub(crate) fn is_literal(e: &str) -> bool {
    let s = e.as_bytes();
    if matches!(s.first(), Some(b'"' | b'`' | b'\'')) {
        return skip_opaque(s, 0) == Some(s.len());
    }
    let n = e
        .strip_prefix('-')
        .or_else(|| e.strip_prefix('+'))
        .unwrap_or(e);
    let nb = n.as_bytes();
    if nb.first().is_some_and(|b| b.is_ascii_digit())
        || (nb.first() == Some(&b'.') && nb.get(1).is_some_and(|b| b.is_ascii_digit()))
    {
        return nb.iter().enumerate().all(|(i, &b)| {
            b.is_ascii_alphanumeric()
                || b == b'_'
                || b == b'.'
                || (matches!(b, b'+' | b'-')
                    && i > 0
                    && if n.starts_with("0x") || n.starts_with("0X") {
                        matches!(nb[i - 1], b'p' | b'P')
                    } else {
                        matches!(nb[i - 1], b'e' | b'E')
                    })
        });
    }
    is_func_literal(e)
}

/// `func(…) … { … }` and nothing after it: evaluating it makes a closure and runs nothing.
pub(crate) fn is_func_literal(e: &str) -> bool {
    let Some(rest) = e.strip_prefix("func") else {
        return false;
    };
    let open = e.len() - rest.trim_start().len();
    if e.as_bytes().get(open) != Some(&b'(') {
        return false;
    }
    let Some(close) = closing(e, open) else {
        return false;
    };
    let body = body_open(e, close + 1);
    body.and_then(|b| closing(e, b)) == Some(e.len() - 1)
}

pub(crate) fn is_place(e: &str) -> bool {
    !e.is_empty() && e.split('.').all(is_ident)
}
