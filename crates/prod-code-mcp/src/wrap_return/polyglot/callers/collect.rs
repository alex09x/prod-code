/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::enclosing::enclosing_polyglot_info;
use super::super::find::find_polyglot_decl;
use super::super::restructure::restructure_declaring_file;
use super::{identity, promise, syntax};
use crate::parameter_object::Language;
use crate::wrap_return::types::{PolyglotFuncDecl, Wrapper};
use crate::wrap_return::utils::{display, is_ident, one_based_lsp_position};
use anyhow::Result;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_and_rewrite_callers(
    root: &Path,
    file: &Path,
    text: &str,
    lang: Language,
    decl: &PolyglotFuncDecl,
    wrapper: &Wrapper,
    now: &str,
    _was: &str,
    constructor: Option<&str>,
    error: Option<&str>,
    semantic_references: &mut HashSet<(PathBuf, u32, u32)>,
    rewritten: &mut BTreeMap<PathBuf, String>,
    propagated: &mut usize,
    blocked: &mut Vec<String>,
    unmatched: &mut Vec<String>,
) -> Result<()> {
    let name = &decl.name;
    let canonical_file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let has_semantic_references = !semantic_references.is_empty();
    let cpp = matches!(lang, Language::C | Language::Cpp)
        .then(|| super::super::cpp_boundary::Boundary::new(root, file, text, decl));

    // Traverse workspace files for callers
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !crate::inline_parameter::language_matches(lang, path) {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        let is_decl_file =
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) == canonical_file;
        let rel_path = display(root, path);
        // Keep byte offsets while ignoring trivia in JS callee/await/postfix syntax.
        let syntax = syntax::SyntaxView::new(&other_content, lang);
        let cpp_code = cpp
            .as_ref()
            .map(|_| std::borrow::Cow::Owned(super::super::cpp_boundary::code(&other_content)));
        let code = cpp_code.as_ref().unwrap_or(&syntax.code);
        if !super::super::swift_scope::admit(
            (root, path, file),
            lang,
            (&other_content, name),
            is_decl_file || has_semantic_references,
            unmatched,
        ) {
            continue;
        }

        let js = matches!(lang, Language::TypeScript | Language::JavaScript);
        let symbols_to_search = super::symbols::discover_symbols_to_search(
            &other_content,
            path,
            file,
            name,
            lang,
            is_decl_file,
            has_semantic_references,
        );
        if symbols_to_search.is_empty() {
            continue;
        }

        let mut file_edits: Vec<(usize, usize, String)> = Vec::new();
        for search_symbol in &symbols_to_search {
            for (matched_at, _) in other_content.match_indices(search_symbol.as_str()) {
                if matched_at > 0 {
                    let prev = other_content[..matched_at].chars().next_back().unwrap();
                    if is_ident(prev) {
                        continue;
                    }
                }
                let after = &other_content[matched_at + search_symbol.len()..];
                if after.starts_with(is_ident) {
                    continue;
                }
                let Some((at, call_symbol)) = identity::call_symbol_site(
                    &other_content,
                    matched_at,
                    search_symbol,
                    name,
                    lang,
                    path,
                    file,
                ) else {
                    continue;
                };
                if (js && !syntax.is_code(at) && !syntax.is_uncertain(at))
                    || (!js
                        && if let Some(code) = &cpp_code {
                            code.as_bytes()[at].is_ascii_whitespace()
                        } else {
                            crate::inline_parameter::is_in_comment(&other_content, at, lang)
                                || crate::inline_parameter::is_in_string(&other_content, at, lang)
                        })
                {
                    continue;
                }

                use identity::Receiver;
                let (call_start, is_member_access, unproven_receiver) = match identity::receiver(
                    &other_content,
                    code,
                    at,
                    lang,
                    (path, file),
                    is_decl_file,
                    has_semantic_references,
                ) {
                    Receiver::Bare => (at, false, false),
                    Receiver::Selected(start) => (start, true, false),
                    Receiver::Ambiguous(start) => (start, true, true),
                    Receiver::Unrelated => continue,
                };

                let source_path =
                    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                let (line_num, col_num) = one_based_lsp_position(&other_content, at);
                let reference_key = (source_path, line_num, col_num);
                if identity::is_import_context_or_binding_at(
                    &other_content,
                    at,
                    search_symbol,
                    name,
                    path,
                    file,
                    lang,
                ) {
                    semantic_references.remove(&reference_key);
                    continue;
                }
                let site = format!("{rel_path}:{line_num}:{col_num}");

                // In declaring file, skip the declaration itself
                if is_decl_file && at >= decl.decl_start && at <= decl.close_paren {
                    semantic_references.remove(&reference_key);
                    continue;
                }

                // Self-call inside function's own body
                if is_decl_file && at > decl.body_open && at < decl.body_close {
                    if has_semantic_references && !semantic_references.remove(&reference_key) {
                        continue;
                    }
                    unmatched.push(format!("{site} (a call inside `{name}` itself)"));
                    continue;
                }

                if let Some(boundary) = &cpp
                    && !boundary.visit(
                        path,
                        &other_content,
                        at,
                        now,
                        has_semantic_references,
                        semantic_references,
                        &reference_key,
                        &site,
                        &mut file_edits,
                        unmatched,
                    )
                {
                    continue;
                }

                if has_semantic_references && !semantic_references.remove(&reference_key) {
                    continue;
                }
                if !has_semantic_references
                    && (!js || !is_member_access)
                    && super::scope::is_shadowed(
                        &other_content,
                        code,
                        at,
                        search_symbol,
                        lang,
                        path,
                        file,
                        name,
                    )
                {
                    continue;
                }
                if unproven_receiver {
                    unmatched.push(format!(
                        "{site} receiver identity for `{call_symbol}` cannot be established"
                    ));
                    continue;
                }
                if syntax.is_uncertain(at) {
                    unmatched.push(format!(
                        "{site} ambiguous regex/division syntax around `{call_symbol}`"
                    ));
                    continue;
                }

                let Some((_args_start, args_end)) =
                    crate::parameter_object::call_args_span(code, at + call_symbol.len())
                else {
                    unmatched.push(format!("{site} `{call_symbol}` used as a value"));
                    continue;
                };

                // Call site found!
                let caller_text = if js && matches!(wrapper, Wrapper::Promise) {
                    code.as_ref()
                } else {
                    &other_content
                };
                let caller_info = enclosing_polyglot_info(caller_text, at, lang);
                let (caller_ret, caller_is_async) =
                    caller_info.unwrap_or_else(|| (String::new(), false));

                match wrapper {
                    Wrapper::Promise => {
                        if caller_is_async {
                            if let Err(reason) =
                                promise::await_call(code, call_start, at, args_end, &mut file_edits)
                            {
                                unmatched.push(format!("{site} {reason}"));
                                continue;
                            }
                            *propagated += 1;
                        } else {
                            let line_text = other_content
                                [other_content[..call_start].rfind('\n').map_or(0, |i| i + 1)..]
                                .lines()
                                .next()
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            blocked.push(format!("{site} the caller is not async and cannot await `{search_symbol}`: `{line_text}`"));
                        }
                    }
                    Wrapper::Option => {
                        let can_propagate = match lang {
                            Language::TypeScript | Language::JavaScript => {
                                caller_ret.contains("| null") || caller_ret.contains("Option<")
                            }
                            Language::Python => {
                                caller_ret.contains("Optional[") || caller_ret.contains("| None")
                            }
                            Language::Cpp | Language::C | Language::Java => {
                                caller_ret.contains("optional") || caller_ret.contains("Optional<")
                            }
                            Language::Swift => {
                                caller_ret.ends_with('?') || caller_ret.contains("Optional<")
                            }
                            Language::Go => caller_ret.starts_with('*'),
                            Language::Rust => unreachable!(),
                        };
                        if can_propagate {
                            *propagated += 1;
                        } else {
                            block_return_incompatible(
                                blocked,
                                &other_content,
                                at,
                                &site,
                                &caller_ret,
                            );
                        }
                    }
                    Wrapper::Result => {
                        let can_propagate = match lang {
                            Language::TypeScript | Language::JavaScript => {
                                caller_ret.contains("Result<")
                            }
                            Language::Python => caller_ret.contains("Result["),
                            Language::Cpp | Language::C | Language::Java => {
                                caller_ret.contains("expected") || caller_ret.contains("Result")
                            }
                            Language::Swift => caller_ret.contains("Result<"),
                            Language::Go => caller_ret.contains("error"),
                            Language::Rust => unreachable!(),
                        };
                        if can_propagate {
                            *propagated += 1;
                        } else {
                            block_return_incompatible(
                                blocked,
                                &other_content,
                                at,
                                &site,
                                &caller_ret,
                            );
                        }
                    }
                    Wrapper::Pointer => {
                        let can_propagate = match lang {
                            Language::Go => caller_ret.starts_with('*'),
                            _ => caller_ret.contains('*'),
                        };
                        if can_propagate {
                            *propagated += 1;
                        } else {
                            block_return_incompatible(
                                blocked,
                                &other_content,
                                at,
                                &site,
                                &caller_ret,
                            );
                        }
                    }
                    Wrapper::Custom(custom_name) => {
                        let base = custom_name
                            .split(['<', '['])
                            .next()
                            .unwrap_or(custom_name)
                            .trim();
                        let base = base.rsplit("::").next().unwrap_or(base);
                        let base = base.rsplit('.').next().unwrap_or(base).trim();
                        let can_propagate = caller_ret.contains(base);
                        if can_propagate {
                            *propagated += 1;
                        } else {
                            block_return_incompatible(
                                blocked,
                                &other_content,
                                at,
                                &site,
                                &caller_ret,
                            );
                        }
                    }
                }
            }
        }
        if !file_edits.is_empty() {
            let mut body = other_content;
            file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
            file_edits.dedup_by_key(|(at, len, _)| (*at, *len));
            for (at, len, replacement) in file_edits.into_iter().rev() {
                body.replace_range(at..at + len, &replacement);
            }
            if is_decl_file {
                let declaration_line = text[..decl.decl_start].lines().count() as u32 + 1;
                let updated_decl =
                    find_polyglot_decl(&body, lang, Some(name.as_str()), Some(declaration_line))?;
                body = restructure_declaring_file(
                    &body,
                    lang,
                    &updated_decl,
                    wrapper,
                    constructor,
                    error,
                )?
                .0;
            }
            rewritten.insert(path.to_path_buf(), body);
        }
    }
    Ok(())
}

fn block_return_incompatible(
    blocked: &mut Vec<String>,
    other_content: &str,
    at: usize,
    site: &str,
    caller_ret: &str,
) {
    let line_text = other_content[other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let desc = if caller_ret.is_empty() {
        "none"
    } else {
        caller_ret
    };
    blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
}
