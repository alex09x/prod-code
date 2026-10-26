/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

use super::codegen::builder::capitalize;
use super::parse::split_balanced_commas;
use super::sites::helpers::{go_literal_values, go_zero_value, is_pure_reorder_value};
use super::types::{InstantiationSite, ReplaceMode, StructDecl};

pub fn rewrite_instantiation(
    site: &InstantiationSite,
    decl: &StructDecl,
    mode: ReplaceMode,
    target_name: &str,
) -> Result<String> {
    if decl.language == "go"
        && !site
            .field_order
            .iter()
            .any(|field| field.starts_with("__positional_"))
    {
        let expected = decl
            .fields
            .iter()
            .filter(|field| site.field_values.contains_key(&field.name))
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        let actual = site
            .field_order
            .iter()
            .filter(|field| site.field_values.contains_key(field.as_str()))
            .map(String::as_str)
            .collect::<Vec<_>>();
        if expected != actual
            && actual.iter().any(|field| {
                site.field_values
                    .get(*field)
                    .is_some_and(|value| !is_pure_reorder_value(value))
            })
        {
            anyhow::bail!(
                "literal field expressions are ordered differently from the declaration and may have side effects; nothing was rewritten"
            );
        }
    }
    match decl.language.as_str() {
        "rust" => {
            let type_ref = if site.rust_self {
                "Self".into()
            } else {
                format!(
                    "{}{}{}",
                    site.prefix,
                    decl.name,
                    site.rust_type_args.as_deref().unwrap_or("")
                )
            };
            match mode {
                ReplaceMode::Factory => {
                    let mut args = Vec::new();
                    for f in &decl.fields {
                        let val = site.field_values.get(&f.name).with_context(|| {
                            format!("missing field `{}` in struct literal", f.name)
                        })?;
                        args.push(val.clone());
                    }
                    let expected = decl.fields.iter().map(|f| &f.name).collect::<Vec<_>>();
                    let actual = site.field_order.iter().collect::<Vec<_>>();
                    if expected != actual
                        && site.field_order.iter().any(|field| {
                            site.field_values
                                .get(field)
                                .is_some_and(|value| !is_pure_reorder_value(value))
                        })
                    {
                        anyhow::ensure!(
                            actual.len() == expected.len()
                                && expected.iter().all(|f| actual.contains(f)),
                            "literal fields do not match the constructor"
                        );
                        let values = actual
                            .iter()
                            .map(|f| site.field_values[*f].as_str())
                            .collect::<Vec<_>>();
                        let bindings = (0..actual.len())
                            .map(|i| format!("__prod_constructor_{i}"))
                            .collect::<Vec<_>>();
                        let arguments = expected
                            .iter()
                            .map(|field| {
                                bindings[actual.iter().position(|f| f == field).unwrap()].clone()
                            })
                            .collect::<Vec<_>>();
                        // Pattern bindings have no scope over the scrutinee. Even a source
                        // variable with one of these names cannot be captured or shadowed.
                        return Ok(format!(
                            "match ({},) {{ ({},) => {type_ref}::{target_name}({}) }}",
                            values.join(", "),
                            bindings.join(", "),
                            arguments.join(", ")
                        ));
                    }
                    Ok(format!("{type_ref}::{target_name}({})", args.join(", ")))
                }
                ReplaceMode::Builder => {
                    let mut chain = String::new();
                    for field in &site.field_order {
                        if let Some(val) = site.field_values.get(field) {
                            chain.push_str(&format!(".{field}({val})"));
                        }
                    }
                    Ok(format!("{type_ref}::builder(){chain}.build()"))
                }
            }
        }
        "go" => match mode {
            ReplaceMode::Factory => {
                let args = go_literal_values(site, decl)?
                    .into_iter()
                    .zip(&decl.fields)
                    .map(|(value, field)| value.unwrap_or_else(|| go_zero_value(&field.ty)))
                    .collect::<Vec<_>>();
                Ok(format!("{target_name}({})", args.join(", ")))
            }
            ReplaceMode::Builder => {
                let mut chain = String::new();
                for (field, value) in decl.fields.iter().zip(go_literal_values(site, decl)?) {
                    if let Some(value) = value {
                        chain.push_str(&format!(".{}({})", field.name, value));
                    }
                }
                Ok(format!("New{target_name}(){chain}.Build()"))
            }
        },
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => {
            let raw_args = site
                .field_values
                .get("__raw_args__")
                .map(String::as_str)
                .unwrap_or("");
            match mode {
                ReplaceMode::Factory => Ok(format!("{}.{target_name}({raw_args})", decl.name)),
                ReplaceMode::Builder => {
                    if raw_args.contains("...")
                        || (decl.language.starts_with("typescript") && raw_args.contains('<'))
                        || decl.fields.iter().any(|f| !f.vis.is_empty())
                    {
                        let helper = super::codegen::members::argument_helper(decl);
                        return Ok(format!("new {target_name}().{helper}({raw_args}).build()"));
                    }
                    let args = if raw_args.trim().is_empty() {
                        Vec::new()
                    } else {
                        crate::parameter_object::entries(
                            raw_args,
                            crate::parameter_object::Language::JavaScript,
                        )
                        .into_iter()
                        .map(|(_, value)| value.to_string())
                        .collect()
                    };
                    anyhow::ensure!(
                        args.len() <= decl.fields.len(),
                        "constructor call has more arguments than fields the builder can set"
                    );
                    let setters = decl
                        .fields
                        .iter()
                        .zip(args)
                        .map(|(field, value)| format!(".{}({})", field.name, value.trim()))
                        .collect::<String>();
                    Ok(format!("new {target_name}(){setters}.build()"))
                }
            }
        }
        "python" => {
            let raw_args = site
                .field_values
                .get("__raw_args__")
                .map(String::as_str)
                .unwrap_or("");
            match mode {
                ReplaceMode::Factory => Ok(format!("{}.{target_name}({raw_args})", decl.name)),
                ReplaceMode::Builder => {
                    let args = if raw_args.trim().is_empty() {
                        Vec::new()
                    } else {
                        crate::parameter_object::entries(
                            raw_args,
                            crate::parameter_object::Language::Python,
                        )
                        .into_iter()
                        .map(|(_, value)| value.to_string())
                        .collect()
                    };
                    let mut setters = Vec::new();
                    for (index, arg) in args.iter().enumerate() {
                        anyhow::ensure!(
                            !arg.trim_start().starts_with('*'),
                            "unsupported expanded Python constructor argument; nothing was written"
                        );
                        let (field_name, value) = if let Some((name, value)) =
                            arg.split_once('=').filter(|(name, value)| {
                                super::parse::is_ident_str(name.trim()) && !value.starts_with('=')
                            }) {
                            (name.trim(), value.trim())
                        } else {
                            let field = decl.fields.get(index).with_context(|| {
                                "constructor call has more arguments than fields the builder can set"
                            })?;
                            (field.name.as_str(), arg.trim())
                        };
                        anyhow::ensure!(
                            decl.fields.iter().any(|field| field.name == field_name),
                            "constructor argument `{field_name}` does not match a field the builder can set"
                        );
                        setters.push(format!(".{field_name}({value})"));
                    }
                    Ok(format!("{target_name}(){}.build()", setters.join("")))
                }
            }
        }
        "cpp" | "c" => {
            let raw_args = site
                .field_values
                .get("__raw_args__")
                .map(String::as_str)
                .unwrap_or("");
            let args = split_balanced_commas(raw_args);
            anyhow::ensure!(
                args.len() <= decl.fields.len(),
                "constructor call has more arguments than fields the factory or builder can set"
            );
            let expression = match mode {
                ReplaceMode::Factory => format!("{}::{target_name}({raw_args})", decl.name),
                ReplaceMode::Builder => {
                    let build = if args.len() < decl.fields.len() {
                        super::codegen::cpp::partial_build_method(decl, args.len())
                    } else {
                        "build".into()
                    };
                    let setters = decl
                        .fields
                        .iter()
                        .zip(args)
                        .map(|(field, value)| format!(".{}({})", field.name, value.trim()))
                        .collect::<String>();
                    format!("{target_name}{{}}{setters}.{build}()")
                }
            };
            if let Some(variable) = site.field_values.get("__variable__") {
                Ok(format!("{} {variable} = {expression}", decl.name))
            } else {
                Ok(expression)
            }
        }
        "swift" => {
            let raw_args = site
                .field_values
                .get("__raw_args__")
                .map(String::as_str)
                .unwrap_or("");
            match mode {
                ReplaceMode::Factory => Ok(format!("{}.{target_name}({raw_args})", decl.name)),
                ReplaceMode::Builder => {
                    let args = if raw_args.trim().is_empty() {
                        Vec::new()
                    } else {
                        split_balanced_commas(raw_args)
                    };
                    anyhow::ensure!(
                        args.len() <= decl.fields.len(),
                        "constructor call has more arguments than fields the builder can set"
                    );
                    let mut setters = Vec::new();
                    for (index, arg) in args.iter().enumerate() {
                        let (field_name, value) = if let Some((label, value)) = arg.split_once(':')
                        {
                            (label.trim(), value.trim())
                        } else {
                            let field = decl.fields.get(index).with_context(|| {
                                "constructor call has more arguments than fields the builder can set"
                            })?;
                            (field.name.as_str(), arg.trim())
                        };
                        anyhow::ensure!(
                            decl.fields.iter().any(|field| field.name == field_name),
                            "constructor argument label `{field_name}` does not match a field the builder can set"
                        );
                        setters.push(format!(".set{}({value})", capitalize(field_name)));
                    }
                    Ok(format!("{target_name}(){}.build()", setters.join("")))
                }
            }
        }
        _ => anyhow::bail!("unsupported language: {}", decl.language),
    }
}
