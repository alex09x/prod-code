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

use super::syntax::is_ident;

pub fn rewrite_param_entry(
    entry_text: &str,
    target_param: &crate::parameter_object::Param,
    type_param: &str,
    lang: Language,
) -> String {
    match lang {
        Language::TypeScript | Language::JavaScript => {
            if let Some(colon) = entry_text.find(':') {
                let before_colon = &entry_text[..colon];
                let after_colon = &entry_text[colon + 1..];
                let (ty_str, default_str) = crate::parameter_object::split_default(
                    after_colon.trim(),
                    Language::TypeScript,
                );
                let trimmed_ty = ty_str.trim();
                let new_ty = if trimmed_ty.ends_with("[]") {
                    format!("{type_param}[]")
                } else if let Some(angle_open) = trimmed_ty.find('<') {
                    if trimmed_ty.ends_with('>') {
                        let container = &trimmed_ty[..angle_open];
                        format!("{container}<{type_param}>")
                    } else {
                        type_param.to_string()
                    }
                } else {
                    type_param.to_string()
                };
                let mut out = format!("{before_colon}: {new_ty}");
                if let Some(def) = default_str {
                    out.push_str(&format!(" = {def}"));
                }
                out
            } else if let Some(eq) = entry_text.find('=') {
                let before_eq = entry_text[..eq].trim_end();
                let after_eq = &entry_text[eq..];
                format!("{before_eq}: {type_param} {after_eq}")
            } else {
                format!("{}: {type_param}", target_param.name)
            }
        }
        Language::Python => {
            if let Some(colon) = entry_text.find(':') {
                let before_colon = &entry_text[..colon];
                let after_colon = &entry_text[colon + 1..];
                let (ty_str, default_str) =
                    crate::parameter_object::split_default(after_colon.trim(), Language::Python);
                let trimmed_ty = ty_str.trim();
                let new_ty = if let Some(bracket_open) = trimmed_ty.find('[') {
                    if trimmed_ty.ends_with(']') {
                        let container = &trimmed_ty[..bracket_open];
                        format!("{container}[{type_param}]")
                    } else {
                        type_param.to_string()
                    }
                } else {
                    type_param.to_string()
                };
                let mut out = format!("{before_colon}: {new_ty}");
                if let Some(def) = default_str {
                    out.push_str(&format!(" = {def}"));
                }
                out
            } else if let Some(eq) = entry_text.find('=') {
                let before_eq = entry_text[..eq].trim_end();
                let after_eq = &entry_text[eq..];
                format!("{before_eq}: {type_param} {after_eq}")
            } else {
                format!("{}: {type_param}", target_param.name)
            }
        }
        Language::Swift => {
            if let Some(colon) = entry_text.find(':') {
                let before_colon = &entry_text[..colon];
                let after_colon = &entry_text[colon + 1..];
                let (ty_str, default_str) =
                    crate::parameter_object::split_default(after_colon.trim(), Language::Swift);
                let trimmed_ty = ty_str.trim();
                let new_ty = if trimmed_ty.ends_with('?') {
                    format!("{type_param}?")
                } else if trimmed_ty.starts_with('[') && trimmed_ty.ends_with(']') {
                    format!("[{type_param}]")
                } else if let Some(rest) = trimmed_ty.strip_prefix("inout ") {
                    let _ = rest;
                    format!("inout {type_param}")
                } else {
                    type_param.to_string()
                };
                let mut out = format!("{before_colon}: {new_ty}");
                if let Some(def) = default_str {
                    out.push_str(&format!(" = {def}"));
                }
                out
            } else {
                format!("{}: {type_param}", target_param.name)
            }
        }
        Language::Go => {
            if let Some(ref old_ty) = target_param.ty {
                let trimmed_ty = old_ty.trim();
                let new_ty = if trimmed_ty.starts_with('*') {
                    format!("*{type_param}")
                } else if trimmed_ty.starts_with("[]") {
                    format!("[]{type_param}")
                } else if trimmed_ty.starts_with("...") {
                    format!("...{type_param}")
                } else {
                    type_param.to_string()
                };
                entry_text.replace(trimmed_ty, &new_ty)
            } else {
                format!("{} {type_param}", target_param.name)
            }
        }
        Language::Cpp | Language::C | Language::Java => {
            let (decl_part, default_part) = if let Some(eq) = entry_text.find('=') {
                (&entry_text[..eq], Some(&entry_text[eq..]))
            } else {
                (entry_text, None)
            };
            let name_match = decl_part
                .match_indices(&target_param.name)
                .filter(|(idx, _)| {
                    let before_ok =
                        *idx == 0 || !is_ident(decl_part[..*idx].chars().last().unwrap());
                    let after_ok = *idx + target_param.name.len() == decl_part.len()
                        || !is_ident(
                            decl_part[*idx + target_param.name.len()..]
                                .chars()
                                .next()
                                .unwrap(),
                        );
                    before_ok && after_ok
                })
                .last();
            if let Some((name_pos, _)) = name_match {
                let ty_part = decl_part[..name_pos].trim_end();
                let words: Vec<&str> = ty_part.split_whitespace().collect();
                let mut rewritten_words = Vec::new();
                let mut replaced = false;
                for w in words {
                    let clean = w.trim_matches(|c: char| !is_ident(c) && c != ':');
                    if !replaced
                        && clean != "const"
                        && clean != "volatile"
                        && clean != "struct"
                        && clean != "class"
                        && !clean.is_empty()
                    {
                        let replaced_w = w.replace(clean, type_param);
                        rewritten_words.push(replaced_w);
                        replaced = true;
                    } else {
                        rewritten_words.push(w.to_string());
                    }
                }
                let mut out = format!(
                    "{} {}",
                    rewritten_words.join(" "),
                    decl_part[name_pos..].trim()
                );
                if let Some(def) = default_part {
                    out.push_str(def);
                }
                out
            } else {
                entry_text.to_string()
            }
        }
        Language::Rust => unreachable!(),
    }
}
