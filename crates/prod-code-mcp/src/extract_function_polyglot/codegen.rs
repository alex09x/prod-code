/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lang::Language;
use super::outputs::reindent_body;
use super::types::{ExtractedParam, OutputKind};

pub fn generate_call_replacement(
    name: &str,
    args: &[String],
    param_names: &[String],
    output: &OutputKind,
    lang: Language,
    is_method: bool,
    indent: &str,
) -> String {
    let args_str = match lang {
        Language::Swift => param_names
            .iter()
            .zip(args)
            .map(|(p, a)| format!("{p}: {a}"))
            .collect::<Vec<_>>()
            .join(", "),
        _ => args.join(", "),
    };

    let call = if is_method {
        match lang {
            Language::Python | Language::Swift => format!("self.{name}({args_str})"),
            Language::TypeScript | Language::JavaScript | Language::Java | Language::Csharp => {
                format!("this.{name}({args_str})")
            }
            Language::Kotlin => format!("{name}({args_str})"),
            Language::Cpp | Language::C => format!("this->{name}({args_str})"),
            Language::Go => format!("r.{name}({args_str})"),
            Language::Rust | Language::Zig => format!("self.{name}({args_str})"),
        }
    } else {
        format!("{name}({args_str})")
    };

    let semi = match lang {
        Language::Python | Language::Swift | Language::Go | Language::Kotlin => "",
        _ => ";",
    };

    match output {
        OutputKind::Expression(_) => format!("{indent}{call}"),
        OutputKind::EndsWithReturn => format!("{indent}return {call}{semi}"),
        OutputKind::SingleVar { name: v, is_new } => {
            if *is_new {
                match lang {
                    Language::Python => format!("{indent}{v} = {call}"),
                    Language::TypeScript | Language::JavaScript => {
                        format!("{indent}const {v} = {call};")
                    }
                    Language::Go => format!("{indent}{v} := {call}"),
                    Language::Cpp | Language::C => format!("{indent}auto {v} = {call};"),
                    Language::Swift => format!("{indent}let {v} = {call}"),
                    Language::Rust => format!("{indent}let {v} = {call};"),
                    Language::Java | Language::Csharp => format!("{indent}var {v} = {call};"),
                    Language::Kotlin => format!("{indent}val {v} = {call}"),
                    Language::Zig => format!("{indent}const {v} = {call};"),
                }
            } else {
                format!("{indent}{v} = {call}{semi}")
            }
        }
        OutputKind::MultipleVars(vars) => {
            let joined = vars.join(", ");
            match lang {
                Language::Python => format!("{indent}{joined} = {call}"),
                Language::TypeScript | Language::JavaScript => {
                    format!("{indent}const [{joined}] = {call};")
                }
                Language::Go => format!("{indent}{joined} := {call}"),
                Language::Cpp | Language::C => format!("{indent}auto [{joined}] = {call};"),
                Language::Swift => format!("{indent}let ({joined}) = {call}"),
                Language::Rust => format!("{indent}let ({joined}) = {call};"),
                Language::Java => format!("{indent}var res = {call};"),
                Language::Csharp => format!("{indent}var ({joined}) = {call};"),
                Language::Kotlin => format!("{indent}val ({joined}) = {call}"),
                Language::Zig => format!("{indent}const {joined} = {call};"),
            }
        }
        OutputKind::Void => format!("{indent}{call}{semi}"),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn generate_function_code(
    name: &str,
    params: &[ExtractedParam],
    output: &OutputKind,
    selection_body: &str,
    lang: Language,
    is_method: bool,
    is_exported: bool,
    method_indent: &str,
    enclosing_ret: Option<&str>,
) -> String {
    let body = match output {
        OutputKind::Expression(expr) => {
            let semi = match lang {
                Language::Python | Language::Swift | Language::Go | Language::Kotlin => "",
                _ => ";",
            };
            format!("return {expr}{semi}")
        }
        OutputKind::EndsWithReturn => selection_body.trim().to_string(),
        OutputKind::SingleVar { name: v, .. } => {
            let semi = match lang {
                Language::Python | Language::Swift | Language::Go | Language::Kotlin => "",
                _ => ";",
            };
            let mut s = selection_body.trim_end().to_string();
            if !s.ends_with(&format!("return {v}")) && !s.ends_with(&format!("return {v};")) {
                s.push('\n');
                s.push_str(&format!("return {v}{semi}"));
            }
            s
        }
        OutputKind::MultipleVars(vars) => {
            let semi = match lang {
                Language::Python | Language::Swift | Language::Go | Language::Kotlin => "",
                _ => ";",
            };
            let mut s = selection_body.trim_end().to_string();
            let ret_val = match lang {
                Language::TypeScript | Language::JavaScript => format!("[{}]", vars.join(", ")),
                _ => vars.join(", "),
            };
            s.push('\n');
            s.push_str(&format!("return {ret_val}{semi}"));
            s
        }
        OutputKind::Void => selection_body.trim_end().to_string(),
    };

    let base_body_indent = if is_method {
        method_indent.len() + 4
    } else {
        4
    };
    let reindented = reindent_body(&body, base_body_indent, lang);

    match lang {
        Language::Python => {
            let mut p_list = Vec::new();
            if is_method {
                p_list.push("self".to_string());
            }
            p_list.extend(params.iter().map(|p| p.name.clone()));
            let p_str = p_list.join(", ");
            if is_method {
                format!("{method_indent}def {name}({p_str}):\n{reindented}\n")
            } else {
                format!("def {name}({p_str}):\n{reindented}\n")
            }
        }
        Language::TypeScript => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("any")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("number");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => format!(": {ret_ty}"),
                OutputKind::Void => ": void".to_string(),
                _ => String::new(),
            };
            let export_prefix = if is_exported && !is_method {
                "export "
            } else {
                ""
            };
            if is_method {
                format!(
                    "{method_indent}private {name}({p_str}){ret_ann} {{\n{reindented}\n{method_indent}}}\n"
                )
            } else {
                format!("{export_prefix}function {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
            }
        }
        Language::JavaScript => {
            let p_str = params
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            let export_prefix = if is_exported && !is_method {
                "export "
            } else {
                ""
            };
            if is_method {
                format!(
                    "{method_indent}private {name}({p_str}) {{\n{reindented}\n{method_indent}}}\n"
                )
            } else {
                format!("{export_prefix}function {name}({p_str}) {{\n{reindented}\n}}\n")
            }
        }
        Language::Go => {
            let p_str = params
                .iter()
                .map(|p| format!("{} {}", p.name, p.ty.as_deref().unwrap_or("int")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("int");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => format!(" {ret_ty}"),
                _ => String::new(),
            };
            format!("func {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
        }
        Language::Cpp | Language::C => {
            let p_str = params
                .iter()
                .map(|p| format!("{} {}", p.ty.as_deref().unwrap_or("int"), p.name))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("int");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => ret_ty,
                _ => "void",
            };
            if is_method {
                format!(
                    "{method_indent}{ret_ann} {name}({p_str}) {{\n{reindented}\n{method_indent}}}\n"
                )
            } else {
                format!("static {ret_ann} {name}({p_str}) {{\n{reindented}\n}}\n")
            }
        }
        Language::Swift => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("Int")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("Int");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => format!(" -> {ret_ty}"),
                _ => String::new(),
            };
            if is_method {
                format!(
                    "{method_indent}private func {name}({p_str}){ret_ann} {{\n{reindented}\n{method_indent}}}\n"
                )
            } else {
                format!("func {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
            }
        }
        Language::Rust => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("usize")))
                .collect::<Vec<_>>()
                .join(", ");
            format!("fn {name}({p_str}) {{\n{reindented}\n}}\n")
        }
        Language::Java => {
            let p_str = params
                .iter()
                .map(|p| format!("{} {}", p.ty.as_deref().unwrap_or("Object"), p.name))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("void");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => ret_ty,
                _ => "void",
            };
            let vis = if is_exported { "public " } else { "private " };
            let has_this = selection_body.contains("this.") || selection_body.contains("this ");
            if is_method || has_this {
                format!(
                    "{method_indent}{vis}{ret_ann} {name}({p_str}) {{\n{reindented}\n{method_indent}}}\n"
                )
            } else {
                format!(
                    "{method_indent}{vis}static {ret_ann} {name}({p_str}) {{\n{reindented}\n{method_indent}}}\n"
                )
            }
        }
        Language::Csharp => {
            let p_str = params
                .iter()
                .map(|p| format!("{} {}", p.ty.as_deref().unwrap_or("object"), p.name))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ty = enclosing_ret.unwrap_or("void");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => ret_ty,
                _ => "void",
            };
            let vis = if is_exported { "public " } else { "private " };
            let has_this = selection_body.contains("this.") || selection_body.contains("this ");
            if is_method || has_this {
                format!(
                    "{method_indent}{vis}{ret_ann} {name}({p_str})\n{method_indent}{{\n{reindented}\n{method_indent}}}\n"
                )
            } else {
                format!(
                    "{method_indent}{vis}static {ret_ann} {name}({p_str})\n{method_indent}{{\n{reindented}\n{method_indent}}}\n"
                )
            }
        }
        Language::Kotlin => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("Any")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => {
                    if let Some(ret) = enclosing_ret {
                        format!(": {ret}")
                    } else {
                        String::new()
                    }
                }
                OutputKind::Void => String::new(),
                _ => String::new(),
            };
            let vis = if is_exported { "" } else { "private " };
            if is_method {
                format!(
                    "{method_indent}{vis}fun {name}({p_str}){ret_ann} {{\n{reindented}\n{method_indent}}}\n"
                )
            } else {
                format!("{vis}fun {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
            }
        }
        Language::Zig => {
            let p_str = params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.ty.as_deref().unwrap_or("anytype")))
                .collect::<Vec<_>>()
                .join(", ");
            let ret_ann = match output {
                OutputKind::Expression(_) | OutputKind::SingleVar { .. } => {
                    if let Some(ret) = enclosing_ret {
                        format!(" {ret}")
                    } else {
                        " anytype".to_string()
                    }
                }
                OutputKind::Void => " void".to_string(),
                _ => String::new(),
            };
            let vis = if is_exported { "pub " } else { "" };
            if is_method {
                format!(
                    "{method_indent}{vis}fn {name}({p_str}){ret_ann} {{\n{reindented}\n{method_indent}}}\n"
                )
            } else {
                format!("{vis}fn {name}({p_str}){ret_ann} {{\n{reindented}\n}}\n")
            }
        }
    }
}
