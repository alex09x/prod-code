/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, BTreeSet};

use super::tokens::{
    ActiveBinding, active_let_bindings, is_field_path_of, is_ident, mentions, parse_unique_structs,
};
use crate::signature::call_sites::normalize_type_spelling;

/// Proves that `b.rhs` is a side-effect-free borrow projection of `parent` (`&mut ParentStruct`)
/// producing `param_ty` (`&TargetTy`) without invoking user-defined methods or `Deref` coercions.
fn prove_pure_borrow_rhs(
    b: &ActiveBinding,
    parent: &str,
    parent_ty: &str,
    param_ty: &str,
    structs: &BTreeMap<String, BTreeMap<String, String>>,
    shadowed: &BTreeSet<String>,
) -> bool {
    if b.is_mut || b.is_ref_pat {
        return false;
    }
    let norm_param = normalize_type_spelling(param_ty);
    let Some(target_ty) = norm_param
        .strip_prefix('&')
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.starts_with("mut "))
    else {
        return false;
    };
    if b.explicit_ty
        .as_ref()
        .is_some_and(|t| normalize_type_spelling(t) != norm_param)
    {
        return false;
    }
    let norm_parent = normalize_type_spelling(parent_ty);
    let Some(parent_struct) = norm_parent
        .strip_prefix("&mut")
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.chars().all(is_ident) && !shadowed.contains(*s))
    else {
        return false;
    };
    let rhs = b.rhs.trim();
    let (place, is_opt_unwrap) = if let Some(rest) = rhs.strip_prefix('&') {
        if rest.trim_start().starts_with("mut ") {
            return false;
        }
        (rest.trim(), false)
    } else if let Some(rest) = rhs.strip_suffix(".as_ref().unwrap()") {
        if shadowed.contains("Option") {
            return false;
        }
        (rest.trim(), true)
    } else {
        return false;
    };
    let Some(field_chain) = place.strip_prefix(parent).and_then(|r| r.strip_prefix('.')) else {
        return false;
    };
    let segs: Vec<&str> = field_chain.split('.').map(str::trim).collect();
    if segs.is_empty()
        || !segs
            .iter()
            .all(|s| !s.is_empty() && s.chars().all(is_ident))
    {
        return false;
    }
    let mut cur = parent_struct;
    for seg in &segs[..segs.len() - 1] {
        let Some(fty) = structs.get(cur).and_then(|f| f.get(*seg)) else {
            return false;
        };
        cur = fty
            .strip_prefix("&mut")
            .or_else(|| fty.strip_prefix('&'))
            .unwrap_or(fty)
            .trim();
        if cur.is_empty() || !cur.chars().all(is_ident) || shadowed.contains(cur) {
            return false;
        }
    }
    let Some(last_ty) = structs.get(cur).and_then(|f| f.get(*segs.last().unwrap())) else {
        return false;
    };
    if is_opt_unwrap {
        last_ty == &format!("Option<{target_ty}>")
            || last_ty == &format!("std::option::Option<{target_ty}>")
            || last_ty == &format!("core::option::Option<{target_ty}>")
    } else {
        last_ty == target_ty
    }
}

fn parent_untouched_since(
    scope: &str,
    from: usize,
    parent: &str,
    mut allowed: Vec<(usize, usize)>,
) -> bool {
    allowed.sort_unstable();
    let mut cursor = from;
    for (s, e) in allowed {
        if s > cursor && mentions(&scope[cursor..s], parent) {
            return false;
        }
        cursor = cursor.max(e);
    }
    cursor >= scope.len() || !mentions(&scope[cursor..], parent)
}

fn is_likely_borrow_of_parent(b: &ActiveBinding, parent: &str) -> bool {
    let rhs = b.rhs.trim();
    (rhs.starts_with('&')
        && is_field_path_of(
            rhs.trim_start_matches("&mut ").trim_start_matches('&'),
            parent,
        ))
        || (b.is_ref_pat && is_field_path_of(rhs, parent))
        || (b
            .explicit_ty
            .as_deref()
            .is_some_and(|ty| ty.trim_start().starts_with('&'))
            && mentions(rhs, parent))
        || (rhs.starts_with(&format!("{parent}."))
            && [
                ".as_ref()",
                ".as_deref()",
                ".as_str()",
                ".as_slice()",
                ".borrow()",
            ]
            .iter()
            .any(|m| rhs.contains(m)))
}

struct ExtractContext<'a> {
    scope: &'a str,
    after: &'a str,
    bindings: BTreeMap<String, ActiveBinding>,
    fn_text: &'a str,
    fn_kw: usize,
    list_open: usize,
    list_close: usize,
    receiver: Option<String>,
    declared: Vec<crate::signature::Declared>,
    mut_parents: Vec<(String, String)>,
}

fn parse_extract_context<'a>(
    text: &'a str,
    start: usize,
    end: usize,
    extracted: &'a str,
    (fn_start, fn_end): (usize, usize),
) -> Option<ExtractContext<'a>> {
    let (body_open, body_close) = crate::introduce_variable::enclosing_body(text, start)?;
    if start <= body_open || fn_start >= extracted.len() || fn_end > extracted.len() {
        return None;
    }
    let scope = &text[body_open..start];
    let bindings = active_let_bindings(scope);
    let fn_text = &extracted[fn_start..fn_end];
    let fn_kw = fn_text.find("fn ")?;
    let (_, list_open, list_close) = crate::signature::param_span(fn_text, fn_kw + 3)?;
    let (receiver, declared) = crate::signature::parse_declared(&fn_text[list_open..list_close]);
    let mut mut_parents: Vec<(String, String)> = declared
        .iter()
        .filter_map(|d| {
            let (_, ty) = crate::signature::parse::split_at_top_level(&d.raw, ':')?;
            ty.contains("&mut")
                .then(|| (d.name.clone(), ty.trim().to_string()))
        })
        .collect();
    if receiver.as_deref().is_some_and(|rv| rv.contains("&mut")) {
        mut_parents.insert(0, ("self".to_string(), "&mut Self".to_string()));
    }
    (!bindings.is_empty() && !mut_parents.is_empty()).then_some(ExtractContext {
        scope,
        after: &text[end.min(body_close)..body_close],
        bindings,
        fn_text,
        fn_kw,
        list_open,
        list_close,
        receiver,
        declared,
        mut_parents,
    })
}

/// Re-acquires proven, side-effect-free field borrows of `&mut parent` inside the extracted
/// helper when they are not used after `end`, avoiding `E0502` at the call site (#1090).
pub fn reacquire_conflicting_borrows(
    text: &str,
    start: usize,
    end: usize,
    extracted: &str,
) -> String {
    let Some(r) = super::rewrite::rewrite_of(text, extracted, start, end) else {
        return extracted.to_string();
    };
    let fn_start = start + r.call.len() + (r.inserted_at - end);
    let fn_end = fn_start + r.function_len;
    let Some(ctx) = parse_extract_context(text, start, end, extracted, (fn_start, fn_end)) else {
        return extracted.to_string();
    };
    let (structs, shadowed) = parse_unique_structs(text);
    let (mut reacquired, mut kept_params) = (Vec::new(), Vec::new());
    if let Some(rv) = ctx.receiver.as_deref() {
        kept_params.push(rv.trim().to_string());
    }
    for d in &ctx.declared {
        let param_ty = crate::signature::parse::split_at_top_level(&d.raw, ':')
            .map_or("", |(_, ty)| ty.trim());
        let is_mut_parent = ctx.mut_parents.iter().any(|(p, _)| p == &d.name);
        if !is_mut_parent
            && let Some(b) = ctx.bindings.get(&d.name)
            && ctx.mut_parents.iter().any(|(p, p_ty)| {
                prove_pure_borrow_rhs(b, p, p_ty, param_ty, &structs, &shadowed) && {
                    let allowed = ctx
                        .bindings
                        .values()
                        .filter(|ob| {
                            ob.stmt_start >= b.stmt_end
                                && ((ob.rhs.trim().starts_with('&')
                                    && is_field_path_of(ob.rhs.trim().trim_start_matches('&'), p))
                                    || prove_pure_borrow_rhs(
                                        ob, p, p_ty, param_ty, &structs, &shadowed,
                                    ))
                        })
                        .map(|ob| (ob.stmt_start, ob.stmt_end))
                        .collect();
                    parent_untouched_since(ctx.scope, b.stmt_end, p, allowed)
                }
            })
            && !mentions(ctx.after, &d.name)
        {
            reacquired.push((d.name.clone(), b.rhs.clone()));
        } else {
            kept_params.push(d.raw.trim().to_string());
        }
    }
    if reacquired.is_empty() {
        return extracted.to_string();
    }
    let Some(call_fn) = r.call.find(&format!("{}(", super::types::PLACEHOLDER)) else {
        return extracted.to_string();
    };
    let call_open = call_fn + super::types::PLACEHOLDER.len();
    let (Some(call_close), Some(call_args), Some(brace_rel)) = (
        crate::signature::argument_list_end(&r.call, call_open),
        crate::signature::split_arguments(&r.call, call_open),
        ctx.fn_text[ctx.list_close..].find('{'),
    ) else {
        return extracted.to_string();
    };
    let dropped: BTreeSet<&str> = reacquired.iter().map(|(n, _)| n.as_str()).collect();
    let new_args: Vec<String> = call_args
        .into_iter()
        .filter(|a| {
            !dropped.contains(
                a.trim()
                    .trim_start_matches("&mut ")
                    .trim_start_matches('&')
                    .trim(),
            )
        })
        .collect();
    let mut new_call = r.call.clone();
    new_call.replace_range(call_open + 1..call_close - 1, &new_args.join(", "));
    let brace_at = ctx.list_close + brace_rel;
    let stmt_indent = format!("{}    ", super::rewrite::indent_at(ctx.fn_text, ctx.fn_kw));
    let injected: String = reacquired
        .iter()
        .map(|(v, rhs)| format!("\n{stmt_indent}let {v} = {rhs};"))
        .collect();
    format!(
        "{}{new_call}{}{}{}{}{injected}{}{}",
        &extracted[..start],
        &extracted[start + r.call.len()..fn_start],
        &ctx.fn_text[..ctx.list_open],
        kept_params.join(", "),
        &ctx.fn_text[ctx.list_close..=brace_at],
        &ctx.fn_text[brace_at + 1..],
        &extracted[fn_end..],
    )
}

/// Reports `E0502` borrow conflicts when an extracted function takes `&mut parent` while a borrow
/// derived from `parent` before `start` is either also passed as a reference argument or remains
/// live after `end`.
pub fn conflicting_borrow_captures(
    text: &str,
    start: usize,
    end: usize,
    extracted: &str,
    function_span: Option<(usize, usize)>,
) -> Vec<String> {
    let Some(ctx) =
        function_span.and_then(|span| parse_extract_context(text, start, end, extracted, span))
    else {
        return Vec::new();
    };
    let mut conflicts = Vec::new();
    for (parent, _) in &ctx.mut_parents {
        for d in &ctx.declared {
            let param_ty = crate::signature::parse::split_at_top_level(&d.raw, ':')
                .map_or("", |(_, ty)| ty.trim());
            if &d.name != parent
                && param_ty.contains('&')
                && let Some(b) = ctx.bindings.get(&d.name)
                && (is_likely_borrow_of_parent(b, parent) || mentions(&b.rhs, parent))
            {
                conflicts.push(format!(
                    "cannot borrow `*{parent}` as mutable because it is also borrowed as immutable via `{}` [E0502]",
                    d.name
                ));
            }
        }
        for (var, b) in &ctx.bindings {
            if var != parent && is_likely_borrow_of_parent(b, parent) && mentions(ctx.after, var) {
                conflicts.push(format!(
                    "cannot borrow `*{parent}` as mutable in extracted call while immutable borrow `{var}` is live across the call [E0502]"
                ));
            }
        }
    }
    conflicts.sort();
    conflicts.dedup();
    conflicts
}
