/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use super::enclosing::enclosing_polyglot_info;
use super::find::find_polyglot_decl;
use super::restructure::restructure_declaring_file;
use crate::parameter_object::Language;
use crate::wrap_return::types::{PolyglotFuncDecl, Wrapper};
use crate::wrap_return::utils::{
    display, is_ident, is_import_export_call_context, one_based_lsp_position,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn collect_and_rewrite_callers(
    root: &Path,
    file: &Path,
    text: &str,
    lang: Language,
    decl: &PolyglotFuncDecl,
    wrapper: &Wrapper,
    now: &str,
    was: &str,
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

    // Traverse workspace files for callers
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !crate::inline_parameter::language_matches(lang, path) {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !other_content.contains(name.as_str()) {
            continue;
        }

        let is_decl_file =
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) == canonical_file;
        let rel_path = display(root, path);

        let mut file_edits: Vec<(usize, usize, String)> = Vec::new();

        for (at, _) in other_content.match_indices(name.as_str()) {
            if at > 0 {
                let prev = other_content[..at].chars().next_back().unwrap();
                if is_ident(prev) {
                    continue;
                }
            }
            let after = &other_content[at + name.len()..];
            if after.starts_with(is_ident) {
                continue;
            }
            if crate::inline_parameter::is_in_comment(&other_content, at, lang)
                || crate::inline_parameter::is_in_string(&other_content, at, lang)
            {
                continue;
            }

            let source_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            let (line_num, col_num) = one_based_lsp_position(&other_content, at);
            let reference_key = (source_path, line_num, col_num);
            if is_import_export_call_context(&other_content, at, lang) {
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

            if !has_semantic_references {
                unmatched.push(format!(
                    "{site}: analyzer returned no references for `{name}`; refusing to rewrite this name-only match"
                ));
                continue;
            }
            if !semantic_references.remove(&reference_key) {
                continue;
            }

            // C++ prototype in header
            let proto_close_paren = other_content[at + name.len()..].find('(').and_then(|open| {
                crate::parameter_object::matching_bracket(&other_content, at + name.len() + open)
            });
            if matches!(lang, Language::Cpp | Language::C)
                && let Some(cp) = proto_close_paren
                && crate::inline_parameter::is_c_cpp_prototype(&other_content, at, cp)
            {
                if let Some(ret_start) = other_content[..at].rfind(was) {
                    file_edits.push((ret_start, was.len(), now.to_string()));
                }
                continue;
            }

            let Some((_args_start, args_end)) =
                crate::parameter_object::call_args_span(&other_content, at + name.len())
            else {
                unmatched.push(format!("{site} `{name}` used as a value"));
                continue;
            };

            // Call site found!
            let caller_info = enclosing_polyglot_info(&other_content, at, lang);
            let (caller_ret, caller_is_async) =
                caller_info.unwrap_or_else(|| (String::new(), false));

            match wrapper {
                Wrapper::Promise => {
                    if caller_is_async {
                        let before_call = other_content[..at].trim_end();
                        if before_call.ends_with("await") {
                            *propagated += 1;
                        } else {
                            let after_call = other_content[args_end + 1..].trim_start();
                            let has_postfix = after_call.starts_with('.')
                                || after_call.starts_with("?.")
                                || after_call.starts_with('[')
                                || after_call.starts_with('(')
                                || after_call.starts_with('!');
                            if has_postfix {
                                file_edits.push((at, 0, "(await ".to_string()));
                                file_edits.push((args_end + 1, 0, ")".to_string()));
                            } else {
                                file_edits.push((at, 0, "await ".to_string()));
                            }
                            *propagated += 1;
                        }
                    } else {
                        let line_text = other_content
                            [other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        blocked.push(format!("{site} the caller is not async and cannot await `{name}`: `{line_text}`"));
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
                        let line_text = other_content
                            [other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let desc = if caller_ret.is_empty() {
                            "none"
                        } else {
                            &caller_ret
                        };
                        blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
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
                        let line_text = other_content
                            [other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let desc = if caller_ret.is_empty() {
                            "none"
                        } else {
                            &caller_ret
                        };
                        blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
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
                        let line_text = other_content
                            [other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let desc = if caller_ret.is_empty() {
                            "none"
                        } else {
                            &caller_ret
                        };
                        blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
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
                        let line_text = other_content
                            [other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let desc = if caller_ret.is_empty() {
                            "none"
                        } else {
                            &caller_ret
                        };
                        blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
                    }
                }
            }
        }

        if !file_edits.is_empty() {
            let mut body = other_content;
            file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
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
