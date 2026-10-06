/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use crate::parameter_object::Language;

use super::go_imports::get_go_package;
use super::imports::{remove_from_braced_ts_import, remove_from_py_from_import};
use super::specifiers::{
    display, display_relative_or_name, is_ident, is_symbol_used, python_module_specifier,
    relative_import_specifier,
};

pub fn rewrite_caller_imports(
    caller_text: &str,
    decl_name: &str,
    caller_file: &Path,
    source_file: &Path,
    target_file: &Path,
    root: &Path,
    lang: Language,
) -> Option<(String, String)> {
    match lang {
        Language::TypeScript | Language::JavaScript => {
            let rel_to_source = relative_import_specifier(caller_file, source_file);
            let rel_to_target = relative_import_specifier(caller_file, target_file);
            let moved_default_export =
                std::fs::read_to_string(source_file)
                    .ok()
                    .is_some_and(|source| {
                        source.lines().any(|line| {
                            let line = line.trim();
                            let Some(decl) = line.strip_prefix("export default ") else {
                                return false;
                            };
                            let decl = decl.strip_prefix("async ").unwrap_or(decl);
                            let Some(name) = decl
                                .strip_prefix("function ")
                                .or_else(|| decl.strip_prefix("class "))
                            else {
                                return false;
                            };
                            let name = name
                                .chars()
                                .take_while(|c| is_ident(*c))
                                .collect::<String>();
                            name == decl_name
                        })
                    });

            let mut found = false;
            let mut lines = Vec::new();
            let mut notes = Vec::new();

            for line in caller_text.lines() {
                let trimmed = line.trim();
                let matches_source = trimmed.starts_with("import ")
                    && (trimmed.contains(&rel_to_source)
                        || trimmed.contains(&rel_to_source.replace("./", "")));
                if matches_source && is_symbol_used(line, decl_name) {
                    found = true;
                    let default_binding = trimmed
                        .strip_prefix("import ")
                        .and_then(|clause| clause.split_once(" from "))
                        .map(|(bindings, _)| bindings.trim());
                    if moved_default_export && default_binding == Some(decl_name) {
                        let rewritten_import = line
                            .replace(&rel_to_source, &rel_to_target)
                            .replace(&rel_to_source.replace("./", ""), &rel_to_target);
                        lines.push(rewritten_import);
                        notes.push(format!("rewrote default import to `{rel_to_target}`"));
                        continue;
                    }
                    let (shrunk, _) = remove_from_braced_ts_import(line, decl_name);
                    if shrunk.is_empty() {
                        let rewritten_import = line
                            .replace(&rel_to_source, &rel_to_target)
                            .replace(&rel_to_source.replace("./", ""), &rel_to_target);
                        lines.push(rewritten_import);
                        notes.push(format!("rewrote import to `{rel_to_target}`"));
                    } else {
                        lines.push(shrunk);
                        lines.push(format!(
                            "import {{ {decl_name} }} from \"{rel_to_target}\";"
                        ));
                        notes.push(format!("imported `{decl_name}` from `{rel_to_target}`"));
                    }
                    continue;
                }
                lines.push(line.to_string());
            }

            if found {
                let mut out = lines.join("\n");
                if caller_text.ends_with('\n') {
                    out.push('\n');
                }
                return Some((out, notes.join("; ")));
            }

            if is_symbol_used(caller_text, decl_name) && !caller_text.contains(&rel_to_target) {
                let import_line = format!("import {{ {decl_name} }} from \"{rel_to_target}\";\n");
                let mut out = import_line;
                out.push_str(caller_text);
                return Some((
                    out,
                    format!("added import `{decl_name}` from `{rel_to_target}`"),
                ));
            }

            None
        }
        Language::Python => {
            let src_mod = python_module_specifier(caller_file, source_file, root);
            let tgt_mod = python_module_specifier(caller_file, target_file, root);

            let mut found = false;
            let mut lines = Vec::new();
            let mut notes = Vec::new();

            for line in caller_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("from ")
                    && trimmed.contains(&src_mod)
                    && is_symbol_used(line, decl_name)
                {
                    found = true;
                    let (shrunk, _) = remove_from_py_from_import(line, decl_name);
                    if shrunk.is_empty() {
                        let replaced = line.replace(&src_mod, &tgt_mod);
                        lines.push(replaced);
                        notes.push(format!("from {tgt_mod} import {decl_name}"));
                    } else {
                        lines.push(shrunk);
                        lines.push(format!("from {tgt_mod} import {decl_name}"));
                        notes.push(format!("from {tgt_mod} import {decl_name}"));
                    }
                    continue;
                }
                lines.push(line.to_string());
            }

            if found {
                let mut out = lines.join("\n");
                if caller_text.ends_with('\n') {
                    out.push('\n');
                }
                return Some((out, notes.join("; ")));
            }

            None
        }
        Language::Go => {
            let src_pkg = get_go_package(source_file);
            let tgt_pkg = get_go_package(target_file);
            if src_pkg == tgt_pkg {
                return None;
            }
            let old_call = format!("{src_pkg}.{decl_name}");
            let new_call = format!("{tgt_pkg}.{decl_name}");
            if caller_text.contains(&old_call) {
                let mut replaced = caller_text.replace(&old_call, &new_call);
                let src_pkg_path = display(root, source_file.parent().unwrap_or(root));
                let tgt_pkg_path = display(root, target_file.parent().unwrap_or(root));
                if replaced.contains(&src_pkg_path) {
                    replaced = replaced.replace(&src_pkg_path, &tgt_pkg_path);
                }
                return Some((replaced, format!("rewrote `{old_call}` to `{new_call}`")));
            }
            None
        }
        Language::Cpp | Language::C => {
            let src_hdr = display_relative_or_name(caller_file, source_file);
            let tgt_hdr = display_relative_or_name(caller_file, target_file);
            let old_inc = format!("#include \"{src_hdr}\"");
            let new_inc = format!("#include \"{tgt_hdr}\"");
            if caller_text.contains(&old_inc) && !caller_text.contains(&new_inc) {
                let replaced = caller_text.replace(&old_inc, &format!("{old_inc}\n{new_inc}"));
                return Some((replaced, format!("included \"{tgt_hdr}\"")));
            }
            None
        }
        _ => None,
    }
}
