/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::returns::rewrite_body_returns;
use crate::parameter_object::Language;
use crate::wrap_return::types::{PolyglotFuncDecl, Wrapper};
use anyhow::{Context, Result};

/// Restructures the declaring file: signature and return expressions in the body.
pub fn restructure_declaring_file(
    text: &str,
    lang: Language,
    decl: &PolyglotFuncDecl,
    wrapper: &Wrapper,
    constructor: Option<&str>,
    error: Option<&str>,
) -> Result<(String, String)> {
    let was = &decl.was;
    let mut out = text.to_string();

    let now = match lang {
        Language::TypeScript | Language::JavaScript => {
            let is_ts = lang == Language::TypeScript;
            match wrapper {
                Wrapper::Promise => {
                    let now = if was.is_empty() || was == "void" {
                        if is_ts && !decl.has_return_type {
                            "Promise<inferred>".to_string()
                        } else if is_ts {
                            "Promise<void>".to_string()
                        } else {
                            "Promise".to_string()
                        }
                    } else {
                        format!("Promise<{was}>")
                    };
                    if is_ts {
                        if decl.has_return_type
                            && let Some((s, e)) = decl.ret_span
                        {
                            out.replace_range(s..e, &format!("Promise<{was}>"));
                        }
                    }
                    if !decl.is_async {
                        let header_part = &out[decl.decl_start..decl.name_start];
                        if let Some(pos) = header_part.rfind("function ") {
                            out.insert_str(decl.decl_start + pos, "async ");
                        } else if decl.is_arrow {
                            let open_p = out[decl.decl_start..].find('(').unwrap();
                            out.insert_str(decl.decl_start + open_p, "async ");
                        } else {
                            // Method
                            out.insert_str(decl.name_start, "async ");
                        }
                    }
                    now
                }
                Wrapper::Option => {
                    let now = if is_ts {
                        format!("{was} | null")
                    } else {
                        "Option".to_string()
                    };
                    if is_ts {
                        if decl.has_return_type
                            && let Some((s, e)) = decl.ret_span
                        {
                            out.replace_range(s..e, &format!("{was} | null"));
                        } else {
                            out.insert_str(decl.close_paren + 1, ": void | null");
                        }
                    }
                    now
                }
                Wrapper::Result => {
                    let err_ty = error.unwrap_or("Error");
                    let now = if is_ts {
                        format!("Result<{was}, {err_ty}>")
                    } else {
                        format!("Result<{err_ty}>")
                    };
                    if is_ts {
                        if decl.has_return_type
                            && let Some((s, e)) = decl.ret_span
                        {
                            out.replace_range(s..e, &now);
                        } else {
                            out.insert_str(
                                decl.close_paren + 1,
                                &format!(": Result<void, {err_ty}>"),
                            );
                        }
                    }
                    now
                }
                Wrapper::Custom(custom_name) => {
                    let base = custom_name.split('<').next().unwrap_or(custom_name).trim();
                    let now = if custom_name.contains('<') {
                        custom_name
                            .replace("<T>", &format!("<{was}>"))
                            .replace("<>", &format!("<{was}>"))
                    } else if was.is_empty() || was == "void" {
                        if is_ts {
                            format!("{base}<void>")
                        } else {
                            base.to_string()
                        }
                    } else {
                        if is_ts {
                            format!("{base}<{was}>")
                        } else {
                            base.to_string()
                        }
                    };
                    if is_ts {
                        if decl.has_return_type
                            && let Some((s, e)) = decl.ret_span
                        {
                            out.replace_range(s..e, &now);
                        } else {
                            out.insert_str(decl.close_paren + 1, &format!(": {now}"));
                        }
                    }
                    now
                }
                Wrapper::Pointer => {
                    anyhow::bail!("Pointer wrapper is not supported for TypeScript/JavaScript")
                }
            }
        }
        Language::Python => match wrapper {
            Wrapper::Option => {
                let now = if was.is_empty() || was == "None" {
                    "Optional[Any]".to_string()
                } else {
                    format!("Optional[{was}]")
                };
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, &format!(" -> {now}"));
                }
                now
            }
            Wrapper::Result => {
                let err_ty = error.unwrap_or("Exception");
                let now = if was.is_empty() || was == "None" {
                    format!("Result[Any, {err_ty}]")
                } else {
                    format!("Result[{was}, {err_ty}]")
                };
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, &format!(" -> {now}"));
                }
                now
            }
            Wrapper::Custom(custom_name) => {
                let base = custom_name.split('[').next().unwrap_or(custom_name).trim();
                let now = if custom_name.contains('[') {
                    custom_name
                        .replace("[T]", &format!("[{was}]"))
                        .replace("[]", &format!("[{was}]"))
                } else if was.is_empty() || was == "None" {
                    format!("{base}[Any]")
                } else {
                    format!("{base}[{was}]")
                };
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, &format!(" -> {now}"));
                }
                now
            }
            Wrapper::Promise | Wrapper::Pointer => {
                anyhow::bail!("{wrapper:?} wrapper is not supported for Python")
            }
        },
        Language::Cpp | Language::C => match wrapper {
            Wrapper::Option => {
                let now = format!("std::optional<{was}>");
                if let Some((s, e)) = decl.ret_span {
                    out.replace_range(s..e, &now);
                }
                now
            }
            Wrapper::Result => {
                let err_ty = error.unwrap_or("std::string");
                let now = format!("std::expected<{was}, {err_ty}>");
                if let Some((s, e)) = decl.ret_span {
                    out.replace_range(s..e, &now);
                }
                now
            }
            Wrapper::Custom(custom_name) => {
                let base = custom_name.split('<').next().unwrap_or(custom_name).trim();
                let now = if custom_name.contains('<') {
                    custom_name
                        .replace("<T>", &format!("<{was}>"))
                        .replace("<>", &format!("<{was}>"))
                } else if was.is_empty() || was == "void" {
                    base.to_string()
                } else {
                    format!("{base}<{was}>")
                };
                if let Some((s, e)) = decl.ret_span {
                    out.replace_range(s..e, &now);
                }
                now
            }
            Wrapper::Promise | Wrapper::Pointer => {
                anyhow::bail!("{wrapper:?} wrapper is not supported for C++")
            }
        },
        Language::Swift => match wrapper {
            Wrapper::Option => {
                let now = format!("{was}?");
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, &format!(" -> {now} "));
                }
                now
            }
            Wrapper::Result => {
                let err_ty = error.unwrap_or("Error");
                let now = format!("Result<{was}, {err_ty}>");
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, &format!(" -> {now} "));
                }
                now
            }
            Wrapper::Custom(custom_name) => {
                let base = custom_name.split('<').next().unwrap_or(custom_name).trim();
                let now = if custom_name.contains('<') {
                    custom_name
                        .replace("<T>", &format!("<{was}>"))
                        .replace("<>", &format!("<{was}>"))
                } else if was.is_empty() || was == "Void" {
                    format!("{base}<Void>")
                } else {
                    format!("{base}<{was}>")
                };
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, &format!(" -> {now} "));
                }
                now
            }
            Wrapper::Promise | Wrapper::Pointer => {
                anyhow::bail!("{wrapper:?} wrapper is not supported for Swift")
            }
        },
        Language::Go => match wrapper {
            Wrapper::Result => {
                let now = if was.is_empty() {
                    "error".to_string()
                } else if was.starts_with('(') && was.ends_with(')') {
                    let inner = was[1..was.len() - 1].trim();
                    format!("({inner}, error)")
                } else {
                    format!("({was}, error)")
                };
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, " error ");
                }
                now
            }
            Wrapper::Pointer | Wrapper::Option => {
                let now = format!("*{was}");
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, &format!(" *{was} "));
                }
                now
            }
            Wrapper::Custom(custom_name) => {
                let base = custom_name.split('[').next().unwrap_or(custom_name).trim();
                let now = if custom_name.contains('[') {
                    custom_name
                        .replace("[T]", &format!("[{was}]"))
                        .replace("[]", &format!("[{was}]"))
                } else if custom_name.starts_with('*') {
                    custom_name.to_string()
                } else if was.is_empty() {
                    base.to_string()
                } else {
                    format!("{base}[{was}]")
                };
                if decl.has_return_type
                    && let Some((s, e)) = decl.ret_span
                {
                    out.replace_range(s..e, &now);
                } else {
                    out.insert_str(decl.body_open, &format!(" {now} "));
                }
                now
            }
            Wrapper::Promise => anyhow::bail!("Promise wrapper is not supported for Go"),
        },
        Language::Rust | Language::Java => unreachable!(),
    };

    // Body rewrite: re-find body open and close in `out`
    let new_body_open = out[decl.name_start..]
        .find(if lang == Language::Python { ':' } else { '{' })
        .map(|i| decl.name_start + i)
        .context("cannot find body open")?;
    let new_body_close = if lang == Language::Python {
        crate::inline_parameter::find_python_body_close(&out, decl.decl_start, new_body_open)
    } else {
        crate::parameter_object::matching_bracket(&out, new_body_open).context("unclosed body")?
    };

    let body_text = &out[new_body_open + 1..new_body_close];
    let rewritten_body = rewrite_body_returns(body_text, lang, wrapper, constructor, was);
    out.replace_range(new_body_open + 1..new_body_close, &rewritten_body);

    Ok((out, now))
}
