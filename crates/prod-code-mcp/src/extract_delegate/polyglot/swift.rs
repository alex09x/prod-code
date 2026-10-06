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

use super::params::extract_param_names_swift;
use crate::extract_delegate::common::{is_ident, is_ident_str, reindent};

pub fn restructure_swift(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<(String, String)> {
    let (open_brace, close_brace, owner) = {
        let mut found = None;
        for keyword in ["struct ", "class "] {
            for (i, _) in text.match_indices(keyword) {
                if i > 0 && text[..i].chars().next_back().is_some_and(is_ident) {
                    continue;
                }
                let after = &text[i + keyword.len()..];
                let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
                if name.is_empty() {
                    continue;
                }
                if let Some(sym) = symbol_opt
                    && sym != name
                {
                    continue;
                }
                let Some(open_rel) = after.find('{') else {
                    continue;
                };
                let open = i + keyword.len() + open_rel;
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
            if found.is_some() {
                break;
            }
        }
        found.with_context(|| {
            if let Some(sym) = symbol_opt {
                format!("struct or class `{sym}` not found")
            } else {
                "no struct/class found at specified location".to_string()
            }
        })?
    };

    let body = &text[open_brace + 1..close_brace];
    let mut prop_decls: BTreeMap<String, String> = BTreeMap::new();
    let mut prop_types: BTreeMap<String, String> = BTreeMap::new();

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }
        if trimmed.contains("var ") || trimmed.contains("let ") {
            let decl = trimmed.strip_prefix("public ").unwrap_or(trimmed);
            let after_kw = if let Some(s) = decl.strip_prefix("var ") {
                s
            } else if let Some(s) = decl.strip_prefix("let ") {
                s
            } else {
                continue;
            };
            if let Some((lhs, ty_part)) = after_kw.split_once(':') {
                let pname = lhs.trim();
                let ty = ty_part.split('=').next().unwrap_or(ty_part).trim();
                if is_ident_str(pname) {
                    prop_decls.insert(pname.to_string(), line.to_string());
                    prop_types.insert(pname.to_string(), ty.to_string());
                }
            }
        }
    }

    for f in fields {
        anyhow::ensure!(
            prop_decls.contains_key(f) || body.contains(&format!("self.{f}")),
            "`{owner}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !prop_decls.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    struct SwiftMethod {
        sig: String,
        params: String,
        full_text: String,
        body: String,
        start_line: usize,
        end_line: usize,
    }

    let mut moved_methods: BTreeMap<String, SwiftMethod> = BTreeMap::new();
    let mut offset = 0;
    while offset < body.len() {
        let slice = &body[offset..];
        let Some(paren_rel) = slice.find('(') else {
            break;
        };
        let paren_pos = offset + paren_rel;
        let before_paren = body[offset..paren_pos].trim();
        let last_word = before_paren.split_whitespace().last().unwrap_or("");
        if is_ident_str(last_word)
            && last_word != "init"
            && last_word != "if"
            && last_word != "while"
            && last_word != "for"
            && let Some(close_paren_rel) = body[paren_pos..].find(')')
        {
            let close_paren = paren_pos + close_paren_rel;
            let params = body[paren_pos + 1..close_paren].trim().to_string();
            if let Some(open_b_rel) = body[close_paren + 1..].find('{') {
                let open_b = close_paren + 1 + open_b_rel;
                if let Some(close_b_rel) =
                    crate::parameter_object::matching_bracket(&body[open_b..], 0)
                {
                    let close_b = open_b + close_b_rel;
                    let line_start = body
                        [..offset + slice[..paren_rel].rfind('\n').map_or(0, |x| x + 1)]
                        .rfind('\n')
                        .map_or(0, |x| x + 1);
                    let sig = body[line_start..open_b].trim().to_string();
                    let m_body = body[open_b + 1..close_b].to_string();
                    let full_text = body[line_start..=close_b].trim().to_string();
                    let start_line = body[..line_start].matches('\n').count();
                    let end_line = body[..close_b].matches('\n').count();
                    let mname = last_word.to_string();
                    if methods.contains(&mname) {
                        moved_methods.insert(
                            mname,
                            SwiftMethod {
                                sig,
                                params,
                                full_text,
                                body: m_body,
                                start_line,
                                end_line,
                            },
                        );
                    }
                    offset = close_b + 1;
                    continue;
                }
            }
        }
        offset = paren_pos + 1;
    }

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        for (idx, _) in minfo.body.match_indices("self.") {
            let after = &minfo.body[idx + 5..];
            let ident: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if !ident.is_empty() && !fields.contains(&ident) && !methods.contains(&ident) {
                anyhow::bail!("`{mname}` uses `{ident}`, which does not move to `{helper}`");
            }
        }
    }

    let mut helper_lines = Vec::new();
    helper_lines.push(format!("public struct {helper} {{"));
    for f in fields {
        let ty = prop_types.get(f).map(|s| s.as_str()).unwrap_or("String");
        helper_lines.push(format!("    public var {f}: {ty}"));
    }
    helper_lines.push(String::new());
    let init_params = fields
        .iter()
        .map(|f| {
            let ty = prop_types.get(f).map(|s| s.as_str()).unwrap_or("String");
            format!("{f}: {ty}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    helper_lines.push(format!("    public init({init_params}) {{"));
    for f in fields {
        helper_lines.push(format!("        self.{f} = {f}"));
    }
    helper_lines.push("    }".to_string());
    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        helper_lines.push(reindent(&minfo.full_text, "    "));
    }
    helper_lines.push("}".to_string());
    let helper_text = helper_lines.join("\n");

    let mut new_body_lines = Vec::new();
    let mut prop_placed = false;
    let mut inside_init = false;
    let mut init_replaced = false;

    let init_call_args = fields
        .iter()
        .map(|f| format!("{f}: {f}"))
        .collect::<Vec<_>>()
        .join(", ");

    for (line_index, line) in body.lines().enumerate() {
        if moved_methods
            .values()
            .any(|minfo| minfo.start_line <= line_index && line_index <= minfo.end_line)
        {
            continue;
        }
        let trimmed = line.trim();
        let is_moved_prop = fields.iter().any(|f| {
            if let Some(decl) = prop_decls.get(f) {
                decl.trim() == trimmed
            } else {
                false
            }
        });
        if is_moved_prop {
            if !prop_placed {
                new_body_lines.push(format!("    public var {field}: {helper}"));
                prop_placed = true;
            }
            continue;
        }

        if trimmed.contains("init(") && trimmed.contains('{') {
            inside_init = true;
        }

        if inside_init {
            let is_moved_assign = fields.iter().any(|f| {
                trimmed.starts_with(&format!("self.{f} ="))
                    || trimmed.starts_with(&format!("self.{f}="))
            });
            if is_moved_assign {
                if !init_replaced {
                    new_body_lines
                        .push(format!("        self.{field} = {helper}({init_call_args})"));
                    init_replaced = true;
                }
                continue;
            }
            if trimmed.contains('}') {
                inside_init = false;
            }
        }

        new_body_lines.push(line.to_string());
    }

    if !prop_placed {
        new_body_lines.insert(0, format!("    public var {field}: {helper}"));
    }

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_swift(&minfo.params);
        new_body_lines.push(String::new());
        new_body_lines.push(format!(
            "    {} {{\n        return {field}.{m}({})\n    }}",
            minfo.sig,
            arg_names.join(", ")
        ));
    }

    let new_body = new_body_lines.join("\n");
    let mut out = text.to_string();
    out.replace_range(open_brace + 1..close_brace, &format!("\n{new_body}\n"));
    let final_text = format!("{helper_text}\n\n{out}");
    Ok((final_text, owner))
}
