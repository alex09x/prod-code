/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod fields;
pub mod methods;

#[cfg(test)]
mod tests;

use anyhow::Result;

use super::params::extract_param_names_ts;
use crate::extract_delegate::common::reindent;
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
    let lang = if is_js {
        crate::parameter_object::Language::JavaScript
    } else {
        crate::parameter_object::Language::TypeScript
    };
    let mut source = super::semantic::Source::new(text, lang, field);
    restructure_ts_semantic(
        text,
        symbol_opt,
        line_opt,
        fields,
        methods,
        helper,
        field,
        is_js,
        &mut source,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn restructure_ts_semantic(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    is_js: bool,
    source: &mut super::semantic::Source<'_>,
) -> Result<(String, String)> {
    let (open_brace, close_brace, owner, is_export, insertion_start) =
        super::js_code::selected_class(text, symbol_opt, line_opt, source.lang)?;

    let body = &text[open_brace + 1..close_brace];

    let field_decls = fields::declarations(body, source.lang);
    let ctor_assigned_fields = fields::constructor_assignments(body, source.lang);

    for f in fields {
        anyhow::ensure!(
            !field_decls
                .get(f)
                .is_some_and(|(decl, _)| decl.split_whitespace().any(|word| word == "static")),
            "static field `{f}` has unproved delegate initialization semantics"
        );
        anyhow::ensure!(
            field_decls.contains_key(f) || ctor_assigned_fields.contains_key(f),
            "`{owner}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !field_decls.contains_key(field) && !ctor_assigned_fields.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    let moved_methods = parse_ts_methods(body, methods, &owner, fields, helper)?;
    // Evaluate declaration initializers in source order, regardless of request ordering.
    let mut ordered_fields = fields.to_vec();
    ordered_fields.sort_by_key(|f| {
        field_decls
            .get(f)
            .and_then(|(decl, _)| body.find(decl))
            .unwrap_or(usize::MAX)
    });
    let fields = &ordered_fields;
    let readonly_deferred =
        fields::readonly_deferred(body, fields, &field_decls, &ctor_assigned_fields)?;
    let deferred = readonly_deferred
        || (fields.len() == 1
            && !field_decls.contains_key(&fields[0])
            && ctor_assigned_fields.contains_key(&fields[0])
            && fields::can_defer(body, &fields[0]));

    // Build Helper class
    let export_kw = if is_export { "export " } else { "" };
    let mut helper_lines = Vec::new();
    helper_lines.push(format!("{export_kw}class {helper} {{"));
    for f in fields {
        if let Some((decl, _)) = field_decls.get(f) {
            let head = decl
                .split_once('=')
                .map_or(decl.trim_end_matches(';'), |(head, _)| head)
                .trim();
            let head = head
                .replacen("private ", "public ", 1)
                .replacen("protected ", "public ", 1);
            let vis = if is_js
                || ["public ", "readonly ", "static "]
                    .iter()
                    .any(|v| head.starts_with(v))
            {
                ""
            } else {
                "public "
            };
            helper_lines.push(format!("    {vis}{head};"));
        } else if is_js {
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
    let ctor_range = fields::constructor_range(body, source.lang);

    let helper_init_args = fields::initializer_args(body, fields, &field_decls, is_js)?;
    let delegate_initializer = if deferred {
        String::new()
    } else {
        format!(" = new {helper}({helper_init_args})")
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
            let initializer = line.find('=');
            if let Some(eq) = initializer {
                anyhow::ensure!(
                    source.positions(&line[eq + 1..]).is_empty(),
                    "field initializer refers to a moved field; evaluation semantics are unproved"
                );
            }
            source.consumed(line);
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

        let line_start = line.as_ptr() as usize - body.as_ptr() as usize;
        if ctor_range
            .as_ref()
            .is_some_and(|range| range.contains(&line_start))
        {
            let is_moved_field_assign = fields.iter().any(|f| {
                trimmed.starts_with(&format!("this.{f} ="))
                    || trimmed.starts_with(&format!("this.{f}="))
            });
            if is_moved_field_assign {
                // Keep each assignment at its original evaluation point. Declaration
                // initializers still run before the constructor, even when overwritten.
                let name = fields
                    .iter()
                    .find(|f| trimmed.starts_with(&format!("this.{f}")))
                    .unwrap();
                let lhs = line.find(&format!("this.{name}")).unwrap() + 5;
                let deferred_rhs = if deferred {
                    let eq = line.find('=').unwrap();
                    anyhow::ensure!(
                        source.positions(&line[eq + 1..]).is_empty(),
                        "deferred initializer reads a moved field before its helper exists"
                    );
                    Some(fields::deferred_rhs(line, source.lang)?)
                } else {
                    None
                };
                source.consumed(&line[lhs..lhs + name.len()]);
                if let Some(rhs) = deferred_rhs {
                    new_body_lines.push(format!("        this.{field} = new {helper}({rhs});"));
                    continue;
                }
                let rewritten = source.rewrite(line)?;
                let insertion = rewritten.find(&format!("this.{name}")).unwrap() + 5;
                let mut rewritten = rewritten;
                rewritten.insert_str(insertion, &format!("{field}."));
                new_body_lines.push(rewritten);
                continue;
            }
        }

        let is_moved_method = moved_methods
            .values()
            .any(|minfo| minfo.start_line <= line_index && line_index <= minfo.end_line);
        if is_moved_method {
            source.moved_method(line, "this")?;
            continue;
        }

        new_body_lines.push(source.rewrite(line)?);
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
    let final_text = format!(
        "{}{helper_text}\n\n{}\n{new_body}\n{}",
        source.rewrite(&text[..insertion_start])?,
        source.rewrite(&text[insertion_start..open_brace + 1])?,
        source.rewrite(&text[close_brace..])?
    );
    Ok((final_text, owner))
}
