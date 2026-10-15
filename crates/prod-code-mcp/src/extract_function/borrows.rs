/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::tokens::{is_ident, mentions};

/// Scans `let <ident> [: <ty>] = <rhs>;` statements in `code` and maps `<ident>` to `<rhs>`.
fn simple_let_bindings(code: &str) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for (i, _) in code.match_indices("let") {
        let whole = !code[..i].chars().next_back().is_some_and(is_ident)
            && code[i + 3..].starts_with(char::is_whitespace);
        if !whole {
            continue;
        }
        let rest = &code[i + 3..];
        let mut depth = 0i32;
        let mut eq_at = None;
        let mut semi_at = None;
        for (j, c) in rest.char_indices() {
            match c {
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' => depth = (depth - 1).max(0),
                '=' if depth == 0
                    && eq_at.is_none()
                    && !rest[..j].ends_with(['=', '!', '<', '>'])
                    && !rest[j + 1..].starts_with('=') =>
                {
                    eq_at = Some(j);
                }
                ';' if depth == 0 && eq_at.is_some() => {
                    semi_at = Some(j);
                    break;
                }
                _ => {}
            }
        }
        let (Some(eq), Some(semi)) = (eq_at, semi_at) else {
            continue;
        };
        let pat = rest[..eq].split(':').next().unwrap_or("").trim();
        let name = pat.trim_start_matches("mut ").trim();
        if !name.is_empty() && name.chars().all(is_ident) {
            let rhs = rest[eq + 1..semi].trim();
            if !rhs.is_empty() {
                map.insert(name.to_string(), rhs.to_string());
            }
        }
    }
    map
}

/// When rust-analyzer extracts a helper that takes both `parent: &mut T` and `child: &U` where
/// `child` was bound before the selection from `parent` (e.g. `let pool = state.pool.as_ref().unwrap();`)
/// and is not read after the selection, passing both `(state, ..., pool)` at the call site triggers
/// `E0502`. Re-acquire `let child = <rhs>;` at the start of the extracted helper and drop `child`
/// from the helper's parameter list and call arguments (#1090).
pub fn reacquire_conflicting_borrows(
    text: &str,
    start: usize,
    end: usize,
    extracted: &str,
) -> String {
    let Some(r) = super::rewrite::rewrite_of(text, extracted, start, end) else {
        return extracted.to_string();
    };
    let Some((body_open, body_close)) = crate::introduce_variable::enclosing_body(text, start)
    else {
        return extracted.to_string();
    };
    if start <= body_open {
        return extracted.to_string();
    }
    let bindings = simple_let_bindings(&text[body_open..start]);
    if bindings.is_empty() {
        return extracted.to_string();
    }
    let after = &text[end.min(body_close)..body_close];
    let fn_start = start + r.call.len() + (r.inserted_at - end);
    let fn_end = fn_start + r.function_len;
    if fn_end > extracted.len() {
        return extracted.to_string();
    }
    let fn_text = &extracted[fn_start..fn_end];
    let Some(fn_kw) = fn_text.find(&format!("fn {}", super::types::PLACEHOLDER)) else {
        return extracted.to_string();
    };
    let Some((_, list_open, list_close)) = crate::signature::param_span(fn_text, fn_kw + 3) else {
        return extracted.to_string();
    };
    let (receiver, declared) = crate::signature::parse_declared(&fn_text[list_open..list_close]);
    let mut mut_parents: Vec<String> = Vec::new();
    if receiver.as_deref().is_some_and(|rv| rv.contains("&mut")) {
        mut_parents.push("self".to_string());
    }
    for d in &declared {
        if d.raw.contains("&mut") {
            mut_parents.push(d.name.clone());
        }
    }
    if mut_parents.is_empty() {
        return extracted.to_string();
    }
    let mut reacquired: Vec<(String, String)> = Vec::new();
    let mut kept_params: Vec<String> = Vec::new();
    if let Some(rv) = receiver.as_deref() {
        kept_params.push(rv.trim().to_string());
    }
    for d in &declared {
        if !mut_parents.contains(&d.name)
            && let Some(rhs) = bindings.get(&d.name)
            && mut_parents.iter().any(|p| mentions(rhs, p))
            && !mentions(after, &d.name)
        {
            reacquired.push((d.name.clone(), rhs.clone()));
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
    let Some(call_close) = crate::signature::argument_list_end(&r.call, call_open) else {
        return extracted.to_string();
    };
    let Some(call_args) = crate::signature::split_arguments(&r.call, call_open) else {
        return extracted.to_string();
    };
    let dropped_names: std::collections::BTreeSet<&str> =
        reacquired.iter().map(|(n, _)| n.as_str()).collect();
    let new_call_args: Vec<String> = call_args
        .into_iter()
        .filter(|arg| {
            let clean = arg
                .trim()
                .trim_start_matches("&mut ")
                .trim_start_matches('&')
                .trim();
            !dropped_names.contains(clean)
        })
        .collect();
    let mut new_call = r.call.clone();
    new_call.replace_range(call_open + 1..call_close - 1, &new_call_args.join(", "));

    let Some(brace_rel) = fn_text[list_close..].find('{') else {
        return extracted.to_string();
    };
    let brace_at = list_close + brace_rel;
    let fn_indent = super::rewrite::indent_at(fn_text, fn_kw);
    let stmt_indent = format!("{fn_indent}    ");
    let mut injected = String::new();
    for (var, rhs) in &reacquired {
        injected.push_str(&format!("\n{stmt_indent}let {var} = {rhs};"));
    }
    let mut new_fn = String::with_capacity(fn_text.len() + injected.len());
    new_fn.push_str(&fn_text[..list_open]);
    new_fn.push_str(&kept_params.join(", "));
    new_fn.push_str(&fn_text[list_close..=brace_at]);
    new_fn.push_str(&injected);
    new_fn.push_str(&fn_text[brace_at + 1..]);

    let mut out = String::with_capacity(extracted.len() + injected.len());
    out.push_str(&extracted[..start]);
    out.push_str(&new_call);
    out.push_str(&extracted[start + r.call.len()..fn_start]);
    out.push_str(&new_fn);
    out.push_str(&extracted[fn_end..]);
    out
}

/// Reports `E0502` borrow conflicts when an extracted function takes `&mut parent` while a borrow
/// derived from `parent` before `start` is either also passed as an argument or used after `end`.
pub fn conflicting_borrow_captures(
    text: &str,
    start: usize,
    end: usize,
    extracted: &str,
    function_span: Option<(usize, usize)>,
) -> Vec<String> {
    let Some((body_open, body_close)) = crate::introduce_variable::enclosing_body(text, start)
    else {
        return Vec::new();
    };
    if start <= body_open {
        return Vec::new();
    }
    let bindings = simple_let_bindings(&text[body_open..start]);
    if bindings.is_empty() {
        return Vec::new();
    }
    let after = &text[end.min(body_close)..body_close];
    let (fn_start, fn_end) = match function_span {
        Some((s, e)) if s < extracted.len() && e <= extracted.len() => (s, e),
        _ => return Vec::new(),
    };
    let fn_text = &extracted[fn_start..fn_end];
    let Some(fn_kw) = fn_text.find("fn ") else {
        return Vec::new();
    };
    let Some((_, list_open, list_close)) = crate::signature::param_span(fn_text, fn_kw + 3) else {
        return Vec::new();
    };
    let (receiver, declared) = crate::signature::parse_declared(&fn_text[list_open..list_close]);
    let mut mut_parents: Vec<String> = Vec::new();
    if receiver.as_deref().is_some_and(|rv| rv.contains("&mut")) {
        mut_parents.push("self".to_string());
    }
    for d in &declared {
        if d.raw.contains("&mut") {
            mut_parents.push(d.name.clone());
        }
    }
    let mut conflicts = Vec::new();
    for parent in &mut_parents {
        for d in &declared {
            if &d.name != parent
                && let Some(rhs) = bindings.get(&d.name)
                && mentions(rhs, parent)
            {
                conflicts.push(format!(
                    "cannot borrow `*{parent}` as mutable because it is also borrowed as immutable via `{}` [E0502]",
                    d.name
                ));
            }
        }
        for (var, rhs) in &bindings {
            if var != parent && mentions(rhs, parent) && mentions(after, var) {
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
