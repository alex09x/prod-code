/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::Path;

use super::syntax::is_ident;

pub(crate) enum CallRewriteOutcome {
    Edit {
        call_start: usize,
        close_paren: usize,
        new_call: String,
        blocked: Option<String>,
    },
    Blocked(String),
    Unmatched(String),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn rewrite_call_site(
    body: &str,
    at: usize,
    site: &str,
    c: u32,
    name: &str,
    path: &Path,
    file: &Path,
    span_start: usize,
    span_end: usize,
    index: usize,
    receiver: &str,
    target_name: &str,
    target_module: &crate::move_item::ModulePath,
    force: bool,
) -> Result<CallRewriteOutcome> {
    if !body[at..].starts_with(name) || body[at + name.len()..].starts_with(is_ident) {
        return Ok(CallRewriteOutcome::Unmatched(format!(
            "{site}:{c}: the analyzer places `{name}` here, but the file says otherwise"
        )));
    }
    if path == file && span_start <= at && at < span_end {
        return Ok(CallRewriteOutcome::Unmatched(format!(
            "{site}: `{name}` calls itself; not handled"
        )));
    }
    let Some((args_start, args_end)) =
        crate::parameter_object::call_args_span(body, at + name.len())
    else {
        return Ok(CallRewriteOutcome::Unmatched(format!(
            "{site}: `{name}` is used as a value, not called; its callers pass the receiver"
        )));
    };
    let args = crate::parameter_object::split_args(&body[args_start..args_end]);
    let before = body[..at].trim_end();
    let close_paren = args_end + body[args_end..].find(')').unwrap_or(0) + 1;

    if let Some(dot) = before.strip_suffix('.').map(|b| b.len()) {
        // `o.price_with(t, 1)` → `t.price_with(&o, 1)`.
        let recv_start = crate::encapsulate_field::chain_start(body, dot);
        let recv = body[recv_start..dot].trim().to_string();
        let Some(arg) = args.get(index) else {
            return Ok(CallRewriteOutcome::Unmatched(format!(
                "{site}: the call has fewer arguments than `{name}`"
            )));
        };
        let mut blocked = None;
        if crate::make_static::receiver_has_effects(&recv)
            || crate::make_static::receiver_has_effects(arg)
        {
            let reason = format!(
                "{site}: `{recv}` and `{}` would be evaluated in the other order",
                arg.trim()
            );
            if !force {
                return Ok(CallRewriteOutcome::Blocked(reason));
            }
            blocked = Some(reason);
        }
        let recv_expr = crate::to_method::receiver_of(&recv);
        let passed = match receiver.trim() {
            r if r.starts_with("&mut") || r.contains("mut self") && r.starts_with('&') => {
                format!("&mut {recv_expr}")
            }
            r if r.starts_with('&') => format!("&{recv_expr}"),
            _ => recv_expr,
        };
        let mut new_args: Vec<String> = args.iter().map(|a| a.trim().to_string()).collect();
        new_args[index] = passed;
        Ok(CallRewriteOutcome::Edit {
            call_start: recv_start,
            close_paren,
            new_call: format!(
                "{}.{name}({})",
                crate::to_method::receiver_of(arg),
                new_args.join(", ")
            ),
            blocked,
        })
    } else if let Some(qualifier) = before.strip_suffix("::") {
        // `Order::price_with(o, t, 1)` → `Tax::price_with(t, o, 1)`.
        let path_start = qualifier
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c) || *c == ':')
            .last()
            .map_or(qualifier.len(), |(i, _)| i);
        if args.len() < index + 2 {
            return Ok(CallRewriteOutcome::Unmatched(format!(
                "{site}: the call has fewer arguments than `{name}`"
            )));
        }
        let mut blocked = None;
        if crate::make_static::receiver_has_effects(&args[0])
            || crate::make_static::receiver_has_effects(&args[index + 1])
        {
            let reason = format!(
                "{site}: `{}` and `{}` would be evaluated in the other order",
                args[0].trim(),
                args[index + 1].trim()
            );
            if !force {
                return Ok(CallRewriteOutcome::Blocked(reason));
            }
            blocked = Some(reason);
        }
        let mut new_args: Vec<String> = args.iter().map(|a| a.trim().to_string()).collect();
        new_args.swap(0, index + 1);
        let (_, caller_module) = crate::move_item::module_of(path)?;
        let target_path = if caller_module.segments == target_module.segments {
            target_name.to_string()
        } else {
            format!(
                "{}::{target_name}",
                target_module.spelled_from(&caller_module.krate)
            )
        };
        Ok(CallRewriteOutcome::Edit {
            call_start: path_start,
            close_paren,
            new_call: format!("{target_path}::{name}({})", new_args.join(", ")),
            blocked,
        })
    } else {
        Ok(CallRewriteOutcome::Unmatched(format!(
            "{site}: neither a method call nor a path call"
        )))
    }
}
