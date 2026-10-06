/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod methods;

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::params::extract_param_names_ts;
use crate::extract_delegate::common::{is_ident, is_ident_str, reindent};
pub use methods::{TsMethod, parse_ts_methods};

#[allow(clippy::too_many_arguments)]
pub fn restructure_ts(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    is_js: bool,
) -> Result<(String, String)> {
    let (open_brace, close_brace, owner, is_export) = {
        let mut found = None;
        for (i, _) in text.match_indices("class ") {
            if i > 0 && text[..i].chars().next_back().is_some_and(is_ident) {
                continue;
            }
            let after = &text[i + 6..];
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
            let open = i + 6 + open_rel;
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            if let Some(line) = line_opt
                && line > 0
            {
                let (sl, _) = crate::signature::position_at(text, i)?;
                let (el, _) = crate::signature::position_at(text, close)?;
                if line < sl || line > el {
                    continue;
                }
            }
            let before_class = text[..i].trim_end();
            let is_export =
                before_class.ends_with("export") || before_class.ends_with("export default");
            found = Some((open, close, name, is_export));
            break;
        }
        found.with_context(|| {
            if let Some(sym) = symbol_opt {
                format!("class `{sym}` not found")
            } else {
                "no class found at the specified location".to_string()
            }
        })?
    };

    let body = &text[open_brace + 1..close_brace];

    let mut field_decls: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }
        if let Some(semi) = trimmed.strip_suffix(';') {
            let decl = semi.trim();
            let without_init = decl.split('=').next().unwrap_or(decl).trim();
            if let Some((lhs, ty)) = without_init.split_once(':') {
                let fname = lhs.split_whitespace().last().unwrap_or("").trim();
                if is_ident_str(fname) && !fname.starts_with("return") {
                    field_decls.insert(
                        fname.to_string(),
                        (line.to_string(), Some(ty.trim().to_string())),
                    );
                }
            } else {
                let fname = without_init.split_whitespace().last().unwrap_or("").trim();
                if is_ident_str(fname)
                    && !fname.starts_with("constructor")
                    && !fname.starts_with("return")
                {
                    field_decls.insert(fname.to_string(), (line.to_string(), None));
                }
            }
        }
    }

    let mut ctor_assigned_fields: BTreeMap<String, String> = BTreeMap::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("this.") && trimmed.contains('=') && trimmed.ends_with(';') {
            let without_semi = &trimmed[..trimmed.len() - 1];
            if let Some((lhs, rhs)) = without_semi.split_once('=') {
                let fname = lhs.trim().trim_start_matches("this.").trim();
                if is_ident_str(fname) {
                    ctor_assigned_fields.insert(fname.to_string(), rhs.trim().to_string());
                }
            }
        }
    }

    for f in fields {
        anyhow::ensure!(
            field_decls.contains_key(f)
                || ctor_assigned_fields.contains_key(f)
                || body.contains(&format!("this.{f}")),
            "`{owner}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !field_decls.contains_key(field) && !ctor_assigned_fields.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    let moved_methods = parse_ts_methods(body, methods, &owner, fields, helper)?;

    // Build Helper class
    let export_kw = if is_export { "export " } else { "" };
    let mut helper_lines = Vec::new();
    helper_lines.push(format!("{export_kw}class {helper} {{"));
    for f in fields {
        if is_js {
            helper_lines.push(format!("    {f};"));
        } else if let Some((_, Some(ty))) = field_decls.get(f) {
            helper_lines.push(format!("    public {f}: {ty};"));
        } else {
            helper_lines.push(format!("    public {f}: any;"));
        }
    }
    helper_lines.push(String::new());
    let ctor_params = if is_js {
        fields.join(", ")
    } else {
        fields
            .iter()
            .map(|f| {
                if let Some((_, Some(ty))) = field_decls.get(f) {
                    format!("{f}: {ty}")
                } else {
                    format!("{f}: any")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    helper_lines.push(format!("    constructor({ctor_params}) {{"));
    for f in fields {
        helper_lines.push(format!("        this.{f} = {f};"));
    }
    helper_lines.push("    }".to_string());

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        let reindented = reindent(&minfo.full_text, "    ");
        let pub_reindented = if reindented.trim_start().starts_with("private ") {
            reindented.replacen("private ", "public ", 1)
        } else if reindented.trim_start().starts_with("protected ") {
            reindented.replacen("protected ", "public ", 1)
        } else if !reindented.trim_start().starts_with("public ") && !is_js {
            reindented.replacen("    ", "    public ", 1)
        } else {
            reindented
        };
        helper_lines.push(pub_reindented);
    }
    helper_lines.push("}".to_string());
    let helper_text = helper_lines.join("\n");

    // Rewrite Owner class
    let mut new_body_lines = Vec::new();
    let mut delegate_field_placed = false;
    let mut inside_ctor = false;
    let mut ctor_replaced = false;

    let helper_init_args = fields
        .iter()
        .map(|f| {
            if let Some(rhs) = ctor_assigned_fields.get(f) {
                rhs.clone()
            } else if let Some((decl, _)) = field_decls.get(f) {
                decl.split_once('=')
                    .map(|(_, rhs)| rhs.trim().trim_end_matches(';').trim().to_string())
                    .unwrap_or_else(|| {
                        if is_js {
                            "undefined".to_string()
                        } else {
                            "undefined as any".to_string()
                        }
                    })
            } else {
                if is_js {
                    "undefined".to_string()
                } else {
                    "undefined as any".to_string()
                }
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let delegate_initializer = if ctor_assigned_fields.is_empty() {
        format!(" = new {helper}({helper_init_args})")
    } else {
        String::new()
    };

    for (line_index, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        let is_moved_field_decl = fields.iter().any(|f| {
            if let Some((decl_line, _)) = field_decls.get(f) {
                decl_line.trim() == trimmed
            } else {
                false
            }
        });

        if is_moved_field_decl {
            if !delegate_field_placed {
                let vis = if is_js { "" } else { "public " };
                let type_ann = if is_js { "" } else { ": " };
                let type_name = if is_js { "" } else { helper };
                new_body_lines.push(format!(
                    "    {vis}{field}{type_ann}{type_name}{delegate_initializer};"
                ));
                delegate_field_placed = true;
            }
            continue;
        }

        if trimmed.starts_with("constructor") && trimmed.contains('{') {
            inside_ctor = true;
        }

        if inside_ctor {
            let is_moved_field_assign = fields.iter().any(|f| {
                trimmed.starts_with(&format!("this.{f} ="))
                    || trimmed.starts_with(&format!("this.{f}="))
            });
            if is_moved_field_assign {
                if !ctor_replaced {
                    new_body_lines.push(format!(
                        "        this.{field} = new {helper}({helper_init_args});"
                    ));
                    ctor_replaced = true;
                }
                continue;
            }
            if trimmed.contains('}') {
                inside_ctor = false;
            }
        }

        let is_moved_method = moved_methods
            .values()
            .any(|minfo| minfo.start_line <= line_index && line_index <= minfo.end_line);
        if is_moved_method {
            continue;
        }

        new_body_lines.push(line.to_string());
    }

    if !delegate_field_placed {
        let vis = if is_js { "" } else { "public " };
        let type_ann = if is_js { "" } else { ": " };
        let type_name = if is_js { "" } else { helper };
        new_body_lines.insert(
            0,
            format!("    {vis}{field}{type_ann}{type_name}{delegate_initializer};"),
        );
    }

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_ts(&minfo.params);
        let ret_ann = if let Some(ret) = &minfo.ret_type {
            format!(": {ret}")
        } else {
            String::new()
        };
        let async_kw = if minfo.is_async { "async " } else { "" };
        let vis = if minfo.vis.is_empty() {
            if is_js {
                String::new()
            } else {
                "public ".to_string()
            }
        } else {
            format!("{} ", minfo.vis)
        };
        let forwarding = format!(
            "    {vis}{async_kw}{m}({}){ret_ann} {{\n        return this.{field}.{m}({});\n    }}",
            minfo.params,
            arg_names.join(", ")
        );
        new_body_lines.push(String::new());
        new_body_lines.push(forwarding);
    }

    let new_body = new_body_lines.join("\n");
    let mut out = text.to_string();
    out.replace_range(open_brace + 1..close_brace, &format!("\n{new_body}\n"));
    let owner_marker = format!("class {owner}");
    let struct_marker = format!("struct {owner}");
    let owner_start = text[..open_brace]
        .rfind(&owner_marker)
        .or_else(|| text[..open_brace].rfind(&struct_marker))
        .context("cannot locate the selected class declaration for helper insertion")?;
    out.insert_str(owner_start, &format!("{helper_text}\n\n"));
    let final_text = out;
    Ok((final_text, owner))
}
