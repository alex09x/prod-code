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

use super::tokens::{is_ident, mentions};
use crate::signature::call_sites::opaque_at;

#[derive(Debug, Clone)]
struct ActiveBinding {
    is_mut: bool,
    is_ref_pat: bool,
    explicit_ty: Option<String>,
    rhs: String,
    stmt_start: usize,
    stmt_end: usize,
}

fn is_field_path_of(expr: &str, parent: &str) -> bool {
    let expr = expr.trim().trim_matches(['(', ')']).trim();
    expr == parent
        || expr
            .strip_prefix(parent)
            .and_then(|r| r.strip_prefix('.'))
            .is_some_and(|r| {
                !r.is_empty()
                    && r.split('.')
                        .all(|s| !s.is_empty() && s.chars().all(is_ident))
            })
}

/// Returns true when `rhs` is a side-effect-free borrow projection of `parent` (`&parent.field`,
/// `parent.field.as_ref().unwrap()`, etc.).
pub(crate) fn is_pure_borrow_rhs(rhs: &str, parent: &str) -> bool {
    let rhs = rhs.trim();
    if let Some(rest) = rhs.strip_prefix("&mut").map(str::trim_start)
        && rest.len() < rhs.len() - 4
    {
        return is_field_path_of(rest, parent);
    }
    if let Some(rest) = rhs.strip_prefix('&') {
        return is_field_path_of(rest.trim_start(), parent);
    }
    let Some(rest) = rhs.strip_prefix(parent).and_then(|r| r.strip_prefix('.')) else {
        return false;
    };
    let segments: Vec<&str> = rest.split('.').map(str::trim).collect();
    let Some(call_idx) = segments.iter().position(|s| s.contains('(')) else {
        return false;
    };
    if !segments[..call_idx]
        .iter()
        .all(|seg| !seg.is_empty() && seg.chars().all(is_ident))
    {
        return false;
    }
    let mut saw_borrow_proj = false;
    for seg in &segments[call_idx..] {
        match *seg {
            "as_ref()" | "as_deref()" | "as_str()" | "as_slice()" | "borrow()" => {
                saw_borrow_proj = true;
            }
            "unwrap()" if saw_borrow_proj => {}
            _ => return false,
        }
    }
    saw_borrow_proj
}

fn parse_let_stmt(rest: &str) -> Option<(Option<usize>, usize)> {
    let (mut depth, mut eq_at, mut j) = (0i32, None, 0usize);
    while j < rest.len() {
        if let Ok(Some((end, _))) = opaque_at(rest, j, 0) {
            j = end;
            continue;
        }
        match rest.as_bytes()[j] {
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' if depth == 0 => return None,
            b')' | b'}' | b']' => depth -= 1,
            b'=' if depth == 0
                && eq_at.is_none()
                && !rest[..j].ends_with(['=', '!', '<', '>'])
                && !rest[j + 1..].starts_with('=') =>
            {
                eq_at = Some(j);
            }
            b';' if depth == 0 => return Some((eq_at, j)),
            _ => {}
        }
        j += 1;
    }
    None
}

/// Scope-aware scan of `let` bindings in `code` that remain in scope at the end of `code`.
fn active_let_bindings(code: &str) -> BTreeMap<String, ActiveBinding> {
    let bytes = code.as_bytes();
    let mut scopes: Vec<BTreeMap<String, Option<ActiveBinding>>> = vec![BTreeMap::new()];
    let mut i = 0;
    while i < bytes.len() {
        if let Ok(Some((end, _))) = opaque_at(code, i, 0) {
            i = end;
            continue;
        }
        if bytes[i] == b'{' {
            scopes.push(BTreeMap::new());
            i += 1;
            continue;
        }
        if bytes[i] == b'}' {
            if scopes.len() > 1 {
                scopes.pop();
            }
            i += 1;
            continue;
        }
        if code[i..].starts_with("let")
            && !code[..i].chars().next_back().is_some_and(is_ident)
            && code[i + 3..].starts_with(char::is_whitespace)
        {
            let before = code[..i].trim_end();
            let is_cond = ["if", "while"].iter().any(|kw| {
                before.ends_with(kw)
                    && !before[..before.len() - kw.len()]
                        .chars()
                        .next_back()
                        .is_some_and(is_ident)
            });
            let rest = &code[i + 3..];
            if !is_cond && let Some((eq_at, semi)) = parse_let_stmt(rest) {
                let header = rest[..eq_at.unwrap_or(semi)].trim();
                let split = crate::signature::parse::split_at_top_level(header, ':');
                let (pat, explicit_ty) = split.map_or((header, None), |(p, ty)| {
                    (p.trim(), Some(ty.trim().to_string()))
                });
                let (is_ref_pat, after_ref) = pat
                    .strip_prefix("ref")
                    .filter(|r| r.starts_with(char::is_whitespace))
                    .map_or((false, pat), |r| (true, r.trim_start()));
                let (is_mut, name) = after_ref
                    .strip_prefix("mut")
                    .filter(|r| r.starts_with(char::is_whitespace))
                    .map_or((false, after_ref.trim()), |r| (true, r.trim()));
                let frame = scopes.last_mut().unwrap();
                if !name.is_empty() && name.chars().all(is_ident) {
                    let entry = eq_at
                        .map(|eq| rest[eq + 1..semi].trim())
                        .filter(|rhs| !rhs.is_empty())
                        .map(|rhs| ActiveBinding {
                            is_mut,
                            is_ref_pat,
                            explicit_ty,
                            rhs: rhs.to_string(),
                            stmt_start: i,
                            stmt_end: i + 3 + semi + 1,
                        });
                    frame.insert(name.to_string(), entry);
                } else {
                    for tok in pat.split(|c: char| !is_ident(c)).filter(|s| !s.is_empty()) {
                        frame.insert(tok.to_string(), None);
                    }
                }
                i = i + 3 + semi + 1;
                continue;
            }
        }
        i += 1;
    }
    let mut out = BTreeMap::new();
    for (k, v) in scopes.into_iter().flatten() {
        if let Some(b) = v {
            out.insert(k, b);
        } else {
            out.remove(&k);
        }
    }
    out
}

fn parent_untouched_since(
    scope: &str,
    from: usize,
    parent: &str,
    bindings: &BTreeMap<String, ActiveBinding>,
) -> bool {
    let mut allowed: Vec<(usize, usize)> = bindings
        .values()
        .filter(|b| b.stmt_start >= from && is_pure_borrow_rhs(&b.rhs, parent))
        .map(|b| (b.stmt_start, b.stmt_end))
        .collect();
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

fn is_proven_borrow_of_parent(b: &ActiveBinding, parent: &str) -> bool {
    is_pure_borrow_rhs(&b.rhs, parent)
        || (b.is_ref_pat && is_field_path_of(&b.rhs, parent))
        || (b
            .explicit_ty
            .as_deref()
            .is_some_and(|ty| ty.trim_start().starts_with('&'))
            && mentions(&b.rhs, parent))
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
    mut_parents: Vec<String>,
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
    let mut mut_parents: Vec<String> = declared
        .iter()
        .filter(|d| d.raw.contains("&mut"))
        .map(|d| d.name.clone())
        .collect();
    if receiver.as_deref().is_some_and(|rv| rv.contains("&mut")) {
        mut_parents.insert(0, "self".to_string());
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
    let (mut reacquired, mut kept_params) = (Vec::new(), Vec::new());
    if let Some(rv) = ctx.receiver.as_deref() {
        kept_params.push(rv.trim().to_string());
    }
    for d in &ctx.declared {
        let param_ty = crate::signature::parse::split_at_top_level(&d.raw, ':')
            .map_or("", |(_, ty)| ty.trim());
        let is_imm_borrow = param_ty.contains('&') && !param_ty.trim_start().starts_with("&mut");
        if !ctx.mut_parents.contains(&d.name)
            && is_imm_borrow
            && let Some(b) = ctx.bindings.get(&d.name)
            && !b.is_mut
            && ctx.mut_parents.iter().any(|p| {
                is_pure_borrow_rhs(&b.rhs, p)
                    && parent_untouched_since(ctx.scope, b.stmt_end, p, &ctx.bindings)
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

/// Reports `E0502` borrow conflicts when an extracted function takes `&mut parent` while a proven
/// borrow derived from `parent` before `start` is either also passed as a reference argument or
/// remains live after `end`.
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
    for parent in &ctx.mut_parents {
        for d in &ctx.declared {
            let param_ty = crate::signature::parse::split_at_top_level(&d.raw, ':')
                .map_or("", |(_, ty)| ty.trim());
            if &d.name != parent
                && param_ty.contains('&')
                && let Some(b) = ctx.bindings.get(&d.name)
                && (is_proven_borrow_of_parent(b, parent) || mentions(&b.rhs, parent))
            {
                conflicts.push(format!(
                    "cannot borrow `*{parent}` as mutable because it is also borrowed as immutable via `{}` [E0502]",
                    d.name
                ));
            }
        }
        for (var, b) in &ctx.bindings {
            if var != parent && is_proven_borrow_of_parent(b, parent) && mentions(ctx.after, var) {
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
