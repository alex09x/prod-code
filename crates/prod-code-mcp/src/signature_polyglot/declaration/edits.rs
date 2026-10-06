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
use crate::signature::Modifiers;

use crate::signature_polyglot::types::PolyglotDecl;

pub fn build_decl_edits(
    text: &str,
    decl: &PolyglotDecl,
    new_signature: &str,
    modifiers: &Modifiers,
    lang: Language,
) -> Vec<(usize, usize, String)> {
    let mut decl_edits: Vec<(usize, usize, String)> = Vec::new();

    // 1. Parameter list replacement
    decl_edits.push((
        decl.open_paren + 1,
        decl.close_paren,
        if new_signature.is_empty() {
            String::new()
        } else {
            new_signature.to_string()
        },
    ));

    // 2. Return type modifier
    if let Some(new_ret) = &modifiers.returns {
        match lang {
            Language::TypeScript | Language::JavaScript => {
                if let Some((start, end)) = decl.ret_span {
                    decl_edits.push((start, end, format!(": {new_ret} ")));
                } else {
                    decl_edits.push((
                        decl.close_paren + 1,
                        decl.close_paren + 1,
                        format!(": {new_ret} "),
                    ));
                }
            }
            Language::Python => {
                if let Some((start, end)) = decl.ret_span {
                    decl_edits.push((start, end, format!(" -> {new_ret}")));
                } else {
                    decl_edits.push((
                        decl.close_paren + 1,
                        decl.close_paren + 1,
                        format!(" -> {new_ret}"),
                    ));
                }
            }
            Language::Swift => {
                if let Some((start, end)) = decl.ret_span {
                    decl_edits.push((start, end, format!(" -> {new_ret} ")));
                } else {
                    decl_edits.push((
                        decl.close_paren + 1,
                        decl.close_paren + 1,
                        format!(" -> {new_ret} "),
                    ));
                }
            }
            Language::Go => {
                if let Some((start, end)) = decl.ret_span {
                    decl_edits.push((start, end, format!(" {new_ret} ")));
                } else {
                    decl_edits.push((
                        decl.close_paren + 1,
                        decl.close_paren + 1,
                        format!(" {new_ret} "),
                    ));
                }
            }
            Language::Cpp | Language::C | Language::Java => {
                if let Some((start, end)) = decl.ret_span {
                    let original = &text[start..end];
                    let indent = original
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect::<String>();
                    decl_edits.push((start, end, format!("{indent}{new_ret} ")));
                }
            }
            _ => {}
        }
    }

    // 3. Async modifier
    if let Some(want_async) = modifiers.asyncness {
        if want_async && !decl.is_async {
            match lang {
                Language::TypeScript | Language::JavaScript => {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    if let Some(pos) = text[line_start..decl.open_paren].find("function") {
                        decl_edits.push((line_start + pos, line_start + pos, "async ".to_string()));
                    } else {
                        let indent_len = text[line_start..decl.open_paren]
                            .chars()
                            .take_while(|c| c.is_whitespace())
                            .count();
                        decl_edits.push((
                            line_start + indent_len,
                            line_start + indent_len,
                            "async ".to_string(),
                        ));
                    }
                }
                Language::Python => {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    if let Some(pos) = text[line_start..decl.open_paren].find("def ") {
                        decl_edits.push((line_start + pos, line_start + pos, "async ".to_string()));
                    }
                }
                Language::Swift => {
                    decl_edits.push((
                        decl.close_paren + 1,
                        decl.close_paren + 1,
                        " async".to_string(),
                    ));
                }
                _ => {}
            }
        } else if !want_async && decl.is_async {
            match lang {
                Language::TypeScript | Language::JavaScript => {
                    if let Some((start, end)) = decl.async_keyword_span {
                        decl_edits.push((start, end, String::new()));
                    }
                }
                Language::Python => {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    if let Some(pos) = text[line_start..decl.open_paren].find("async def ") {
                        decl_edits.push((line_start + pos, line_start + pos + 6, String::new()));
                    }
                }
                Language::Swift => {
                    if let Some(idx) = text[decl.close_paren..decl.body_open].find("async") {
                        let start = decl.close_paren + idx;
                        decl_edits.push((start, start + 5, String::new()));
                    }
                }
                _ => {}
            }
        }
    }

    // 4. Visibility modifier
    if let Some(vis) = &modifiers.visibility {
        match lang {
            Language::TypeScript | Language::JavaScript => {
                if let Some((start, end)) = decl.visibility_span {
                    decl_edits.push((start, end, format!("{vis} ")));
                } else {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    let indent_len = text[line_start..decl.open_paren]
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .count();
                    decl_edits.push((
                        line_start + indent_len,
                        line_start + indent_len,
                        format!("{vis} "),
                    ));
                }
            }
            Language::Swift => {
                if let Some((start, end)) = decl.visibility_span {
                    decl_edits.push((start, end, format!("{vis} ")));
                } else {
                    let line_start = text[..decl.open_paren].rfind('\n').map_or(0, |p| p + 1);
                    if let Some(pos) = text[line_start..decl.open_paren].find("func ") {
                        let indent_len = text[line_start..line_start + pos]
                            .chars()
                            .take_while(|c| c.is_whitespace())
                            .count();
                        decl_edits.push((
                            line_start + indent_len,
                            line_start + pos,
                            format!("{vis} "),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    decl_edits
}
