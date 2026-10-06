/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::params::extract_param_names_go;
use crate::extract_delegate::common::{is_ident, is_ident_str, split_top};

pub fn restructure_go(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<(String, String)> {
    let (struct_open, struct_close, owner) = {
        let mut found = None;
        for (i, _) in text.match_indices("type ") {
            let after = &text[i + 5..];
            let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if name.is_empty() {
                continue;
            }
            if let Some(sym) = symbol_opt
                && sym != name
            {
                continue;
            }
            let rest = after[name.len()..].trim_start();
            if !rest.starts_with("struct") {
                continue;
            }
            let Some(open_rel) = after.find('{') else {
                continue;
            };
            let open = i + 5 + open_rel;
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            if let Some(l) = line_opt
                && l > 0
            {
                let (sl, _) = crate::signature::position_at(text, i)?;
                let (el, _) = crate::signature::position_at(text, close)?;
                if l < sl || l > el {
                    continue;
                }
            }
            found = Some((open, close, name));
            break;
        }
        found.with_context(|| {
            if let Some(sym) = symbol_opt {
                format!("type `{sym}` struct not found")
            } else {
                "no struct found at specified location".to_string()
            }
        })?
    };

    let struct_body = &text[struct_open + 1..struct_close];
    let mut field_decls: BTreeMap<String, String> = BTreeMap::new();
    let mut field_types: BTreeMap<String, String> = BTreeMap::new();

    for line in struct_body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let fname = parts[0];
            let ftype = parts[1];
            if is_ident_str(fname) {
                field_decls.insert(fname.to_string(), line.to_string());
                field_types.insert(fname.to_string(), ftype.to_string());
            }
        }
    }

    for f in fields {
        anyhow::ensure!(field_decls.contains_key(f), "`{owner}` has no field `{f}`");
    }
    anyhow::ensure!(
        !field_decls.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    struct GoMethod {
        recv_var: String,
        recv_is_ptr: bool,
        params: String,
        ret_type: String,
        body: String,
        start: usize,
        end: usize,
    }

    let mut moved_methods: BTreeMap<String, GoMethod> = BTreeMap::new();
    let mut offset = 0;
    while offset < text.len() {
        let slice = &text[offset..];
        let Some(pos) = slice.find("func ") else {
            break;
        };
        let func_pos = offset + pos;
        let after_func = &text[func_pos + 5..];
        if after_func.starts_with('(')
            && let Some(close_recv) = after_func.find(')')
        {
            let recv_slice = after_func[1..close_recv].trim();
            let recv_parts: Vec<&str> = recv_slice.split_whitespace().collect();
            if recv_parts.len() >= 2 {
                let rvar = recv_parts[0];
                let rty = recv_parts[1];
                let is_ptr = rty.starts_with('*');
                let rname = rty.trim_start_matches('*');
                if rname == owner {
                    let after_recv = after_func[close_recv + 1..].trim_start();
                    let mname: String = after_recv.chars().take_while(|c| is_ident(*c)).collect();
                    if !mname.is_empty()
                        && methods.contains(&mname)
                        && let Some(p_open) = after_recv.find('(')
                        && let Some(p_close) = after_recv[p_open..].find(')')
                    {
                        let params = after_recv[p_open + 1..p_open + p_close].trim().to_string();
                        let after_params = after_recv[p_open + p_close + 1..].trim_start();
                        if let Some(b_open_rel) = after_params.find('{') {
                            let b_open = func_pos
                                + 5
                                + close_recv
                                + 1
                                + (after_func[close_recv + 1..].len() - after_params.len())
                                + b_open_rel;
                            let ret_type = after_params[..b_open_rel].trim().to_string();
                            if let Some(b_close_rel) =
                                crate::parameter_object::matching_bracket(text, b_open)
                            {
                                let body = text[b_open + 1..b_close_rel].to_string();
                                moved_methods.insert(
                                    mname,
                                    GoMethod {
                                        recv_var: rvar.to_string(),
                                        recv_is_ptr: is_ptr,
                                        params,
                                        ret_type,
                                        body,
                                        start: func_pos,
                                        end: b_close_rel + 1,
                                    },
                                );
                                offset = b_close_rel + 1;
                                continue;
                            }
                        }
                    }
                }
            }
        }
        offset = func_pos + 5;
    }

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        let needle = format!("{}.", minfo.recv_var);
        for (pos, _) in minfo.body.match_indices(&needle) {
            let after = &minfo.body[pos + needle.len()..];
            let ident: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if !ident.is_empty() && !fields.contains(&ident) && !methods.contains(&ident) {
                anyhow::bail!("`{mname}` uses `{ident}`, which does not move to `{helper}`");
            }
        }
    }

    let mut helper_lines = Vec::new();
    helper_lines.push(format!("type {helper} struct {{"));
    for f in fields {
        let ty = field_types.get(f).map(|s| s.as_str()).unwrap_or("string");
        helper_lines.push(format!("    {f} {ty}"));
    }
    helper_lines.push("}".to_string());

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        let r_ptr = if minfo.recv_is_ptr { "*" } else { "" };
        let ret_sp = if minfo.ret_type.is_empty() {
            String::new()
        } else {
            format!(" {}", minfo.ret_type)
        };
        let helper_method = format!(
            "func ({} {r_ptr}{helper}) {m}({}){ret_sp} {{\n{}\n}}",
            minfo.recv_var,
            minfo.params,
            minfo.body.trim()
        );
        helper_lines.push(helper_method);
    }
    let helper_text = helper_lines.join("\n");

    let mut out = text.to_string();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_go(&minfo.params);
        let r_ptr = if minfo.recv_is_ptr { "*" } else { "" };
        let ret_sp = if minfo.ret_type.is_empty() {
            String::new()
        } else {
            format!(" {}", minfo.ret_type)
        };
        let ret_kw = if minfo.ret_type.is_empty() {
            ""
        } else {
            "return "
        };
        let forwarding = format!(
            "func ({} {r_ptr}{owner}) {m}({}){ret_sp} {{\n    {ret_kw}{}.{field}.{m}({})\n}}",
            minfo.recv_var,
            minfo.params,
            minfo.recv_var,
            arg_names.join(", ")
        );
        edits.push((minfo.start, minfo.end, forwarding));
    }

    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (s, e, repl) in edits {
        out.replace_range(s..e, &repl);
    }

    let new_struct_open = out
        .find(&format!("type {owner} struct"))
        .context("cannot re-find owner struct")?;
    let b_open = out[new_struct_open..]
        .find('{')
        .map(|x| new_struct_open + x)
        .unwrap();
    let b_close = crate::parameter_object::matching_bracket(&out, b_open).unwrap();
    let cur_struct_body = &out[b_open + 1..b_close];

    let mut new_struct_body_lines = Vec::new();
    let mut field_placed = false;

    for line in cur_struct_body.lines() {
        let trimmed = line.trim();
        let is_moved = fields.iter().any(|f| {
            if let Some(decl) = field_decls.get(f) {
                decl.trim() == trimmed
            } else {
                false
            }
        });
        if is_moved {
            if !field_placed {
                new_struct_body_lines.push(format!("    {field} {helper}"));
                field_placed = true;
            }
            continue;
        }
        new_struct_body_lines.push(line.to_string());
    }

    if !field_placed {
        new_struct_body_lines.insert(0, format!("    {field} {helper}"));
    }

    out.replace_range(
        b_open + 1..b_close,
        &format!("\n{}\n", new_struct_body_lines.join("\n")),
    );

    let final_pos = out.find(&format!("type {owner} struct")).unwrap();
    out.insert_str(final_pos, &format!("{helper_text}\n\n"));
    Ok((out, owner))
}

pub fn rewrite_go_literals(
    text: &str,
    owner: &str,
    fields: &[String],
    field: &str,
    helper: &str,
) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (i, _) in text.match_indices(owner) {
        let before = &text[..i];
        if before.chars().next_back().is_some_and(is_ident)
            || text[i + owner.len()..].chars().next().is_some_and(is_ident)
            || before.trim_end().ends_with("type")
        {
            continue;
        }
        let rest = &text[i + owner.len()..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('{') {
            continue;
        }
        let open = i + owner.len() + (rest.len() - trimmed.len());
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let body = &text[open + 1..close];
        let entries: Vec<&str> = split_top(body, ',')
            .into_iter()
            .map(|(s, e)| body[s..e].trim())
            .filter(|e| !e.is_empty())
            .collect();
        let key = |e: &str| -> String { e.split(':').next().unwrap_or(e).trim().to_string() };
        let inner: Vec<&str> = entries
            .iter()
            .copied()
            .filter(|e| fields.contains(&key(e)))
            .collect();
        if inner.is_empty() {
            continue;
        }
        let indent: String = body
            .trim_start_matches(['\n'])
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let nested = format!("{field}: {helper}{{ {} }}", inner.join(", "));
        let mut all: Vec<String> = Vec::new();
        let mut placed = false;
        for e in &entries {
            if fields.contains(&key(e)) {
                if !placed {
                    all.push(nested.clone());
                    placed = true;
                }
            } else {
                all.push(e.to_string());
            }
        }
        let new_body = if body.contains('\n') {
            let close_indent: String = text[..close]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            format!(
                "\n{indent}{},\n{close_indent}",
                all.join(&format!(",\n{indent}"))
            )
        } else {
            format!(" {} ", all.join(", "))
        };
        edits.push((open + 1, close, new_body));
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_string();
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    out
}
