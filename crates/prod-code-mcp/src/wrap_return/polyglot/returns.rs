/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;
use crate::wrap_return::types::Wrapper;
use crate::wrap_return::utils::{format_constructor_call, is_ident};

pub(crate) fn python_owned_return_offsets(body: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut nested_def_indent = None;
    let mut byte_offset = 0usize;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if let Some(def_indent) = nested_def_indent {
            if trimmed.is_empty() || trimmed.starts_with('#') || indent > def_indent {
                byte_offset += line.len();
                continue;
            }
            nested_def_indent = None;
        }
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            nested_def_indent = Some(indent);
            byte_offset += line.len();
            continue;
        }
        let mut cursor = 0usize;
        while let Some(relative) = line[cursor..].find("return") {
            let at = cursor + relative;
            let before_ok = at == 0 || !is_ident(line[..at].chars().next_back().unwrap());
            let after = at + "return".len();
            let after_ok = after == line.len() || !is_ident(line[after..].chars().next().unwrap());
            let absolute = byte_offset + at;
            if before_ok
                && after_ok
                && !crate::inline_parameter::is_in_string(body, absolute, Language::Python)
            {
                offsets.push(absolute);
            }
            cursor = after;
        }
        byte_offset += line.len();
    }
    offsets
}

/// Rewrites return statements in function body.
pub(crate) fn rewrite_body_returns(
    body: &str,
    lang: Language,
    wrapper: &Wrapper,
    constructor: Option<&str>,
    was: &str,
) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let bytes = body.as_bytes();
    let owned_returns = if lang == Language::Python {
        python_owned_return_offsets(body)
    } else {
        crate::invert_boolean::own_returns(body)
    };
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'r' if body[i..].starts_with("return") => {
                let before = if i > 0 {
                    body.as_bytes()[i - 1] as char
                } else {
                    ' '
                };
                let after = if i + 6 < bytes.len() {
                    body.as_bytes()[i + 6] as char
                } else {
                    ' '
                };
                if !is_ident(before)
                    && !is_ident(after)
                    && owned_returns.contains(&i)
                    && !crate::inline_parameter::is_in_string(body, i, lang)
                {
                    let end_stmt = body[i..].find([';', '\n']).map_or(body.len(), |e| i + e);
                    let ret_stmt = &body[i..end_stmt];
                    let expr = ret_stmt.strip_prefix("return").unwrap().trim();
                    let semi = if ret_stmt.ends_with(';') { ";" } else { "" };
                    let expr_clean = expr.strip_suffix(';').unwrap_or(expr).trim();
                    match wrapper {
                        Wrapper::Custom(custom_name) => {
                            let base = custom_name
                                .split(['<', '['])
                                .next()
                                .unwrap_or(custom_name)
                                .trim();
                            let base = base.rsplit("::").next().unwrap_or(base);
                            let base = base.rsplit('.').next().unwrap_or(base).trim();
                            let wrapped =
                                format_constructor_call(constructor, base, expr_clean, lang, was);
                            edits.push((i, end_stmt - i, format!("return {wrapped}{semi}")));
                        }
                        Wrapper::Result => match lang {
                            Language::Go => {
                                if expr_clean.is_empty() {
                                    edits.push((i, end_stmt - i, format!("return nil{semi}")));
                                } else {
                                    edits.push((
                                        i,
                                        end_stmt - i,
                                        format!("return {expr_clean}, nil{semi}"),
                                    ));
                                }
                            }
                            Language::Swift => {
                                if expr_clean.is_empty() {
                                    edits.push((
                                        i,
                                        end_stmt - i,
                                        format!("return .success(()){semi}"),
                                    ));
                                } else {
                                    edits.push((
                                        i,
                                        end_stmt - i,
                                        format!("return .success({expr_clean}){semi}"),
                                    ));
                                }
                            }
                            Language::Python => {
                                if expr_clean.is_empty() {
                                    edits.push((i, end_stmt - i, format!("return Ok(None){semi}")));
                                } else {
                                    edits.push((
                                        i,
                                        end_stmt - i,
                                        format!("return Ok({expr_clean}){semi}"),
                                    ));
                                }
                            }
                            Language::TypeScript | Language::JavaScript => {
                                if expr_clean.is_empty() {
                                    edits.push((
                                        i,
                                        end_stmt - i,
                                        format!("return {{ ok: true, value: undefined }}{semi}"),
                                    ));
                                } else {
                                    edits.push((
                                        i,
                                        end_stmt - i,
                                        format!("return {{ ok: true, value: {expr_clean} }}{semi}"),
                                    ));
                                }
                            }
                            _ => {}
                        },
                        Wrapper::Pointer | Wrapper::Option
                            if lang == Language::Go
                                && !expr_clean.starts_with('&')
                                && !expr_clean.is_empty()
                                && expr_clean != "nil" =>
                        {
                            edits.push((i, end_stmt - i, format!("return &{expr_clean}{semi}")));
                        }
                        _ => {}
                    }
                    i = end_stmt;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let mut out = body.to_string();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (start, len, repl) in edits {
        out.replace_range(start..start + len, &repl);
    }
    out
}
