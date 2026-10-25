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

use anyhow::Result;

use crate::parameter_object::Language;

use super::go_imports::{
    add_go_import, get_go_package, go_module_import_path, go_package_is_used,
    rewrite_go_cross_pkg_in_source,
};
use super::imports::{py_from_module, remove_from_braced_ts_import, remove_from_py_from_import};
use super::specifiers::{
    display, display_relative_or_name, is_ident, is_symbol_used, python_module_specifier,
    relative_import_specifier,
};

/// Points the module specifier of an import line at `to`, leaving the imported names alone.
fn retarget_import(line: &str, from: &str, to: &str) -> String {
    let split = line.rfind(" from ").map_or(0, |at| at + 6);
    let (bindings, specifier) = line.split_at(split);
    let specifier = if specifier.contains(from) {
        specifier.replacen(from, to, 1)
    } else {
        specifier.replacen(&from.replace("./", ""), to, 1)
    };
    format!("{bindings}{specifier}")
}

pub fn rewrite_caller_imports(
    caller_text: &str,
    decl_name: &str,
    caller_file: &Path,
    source_file: &Path,
    target_file: &Path,
    root: &Path,
    lang: Language,
) -> Option<(String, String)> {
    rewrite_caller_imports_checked(
        caller_text,
        decl_name,
        caller_file,
        source_file,
        target_file,
        root,
        lang,
    )
    .ok()
    .flatten()
}

pub(crate) fn rewrite_caller_imports_checked(
    caller_text: &str,
    decl_name: &str,
    caller_file: &Path,
    source_file: &Path,
    target_file: &Path,
    root: &Path,
    lang: Language,
) -> Result<Option<(String, String)>> {
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
                        lines.push(retarget_import(line, &rel_to_source, &rel_to_target));
                        notes.push(format!("rewrote default import to `{rel_to_target}`"));
                        continue;
                    }
                    let (shrunk, _) = remove_from_braced_ts_import(line, decl_name);
                    if shrunk.is_empty() {
                        lines.push(retarget_import(line, &rel_to_source, &rel_to_target));
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
                return Ok(Some((out, notes.join("; "))));
            }

            if is_symbol_used(caller_text, decl_name) && !caller_text.contains(&rel_to_target) {
                let import_line = format!("import {{ {decl_name} }} from \"{rel_to_target}\";\n");
                let mut out = import_line;
                out.push_str(caller_text);
                return Ok(Some((
                    out,
                    format!("added import `{decl_name}` from `{rel_to_target}`"),
                )));
            }

            Ok(None)
        }
        Language::Python => {
            let src_mod = python_module_specifier(caller_file, source_file, root);
            let tgt_mod = python_module_specifier(caller_file, target_file, root);

            let mut found = false;
            let mut lines = Vec::new();
            let mut notes = Vec::new();

            for line in caller_text.lines() {
                let trimmed = line.trim();
                if py_from_module(trimmed) == Some(src_mod.as_str())
                    && is_symbol_used(line, decl_name)
                {
                    found = true;
                    let (shrunk, _) = remove_from_py_from_import(line, decl_name);
                    if shrunk.is_empty() {
                        let replaced = line.replacen(&src_mod, &tgt_mod, 1);
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
                return Ok(Some((out, notes.join("; "))));
            }

            Ok(None)
        }
        Language::Go => {
            if source_file.parent() == target_file.parent() {
                return Ok(None);
            }
            let src_pkg = get_go_package(source_file);
            let tgt_pkg = get_go_package(target_file);
            let caller_pkg = get_go_package(caller_file);

            if caller_file.parent() == source_file.parent() && caller_pkg == src_pkg {
                if !is_symbol_used(caller_text, decl_name) {
                    return Ok(None);
                }
                let (rewritten, note) = rewrite_go_cross_pkg_in_source(
                    caller_text,
                    decl_name,
                    &tgt_pkg,
                    caller_file,
                    target_file,
                    root,
                )?;
                return Ok((!note.is_empty()).then_some((rewritten, note)));
            }

            if src_pkg == tgt_pkg {
                return Ok(None);
            }
            let old_call = format!("{src_pkg}.{decl_name}");
            let new_call = format!("{tgt_pkg}.{decl_name}");
            if caller_text.contains(&old_call) {
                let mut replaced = caller_text.replace(&old_call, &new_call);
                let src_pkg_path = display(root, source_file.parent().unwrap_or(root));
                let tgt_pkg_path = display(root, target_file.parent().unwrap_or(root));
                if !go_package_is_used(&replaced, &src_pkg) && replaced.contains(&src_pkg_path) {
                    replaced = replaced.replace(&src_pkg_path, &tgt_pkg_path);
                } else if let Ok(target_import_path) =
                    go_module_import_path(target_file.parent().unwrap_or(root))
                    && !replaced.contains(&format!("\"{target_import_path}\""))
                {
                    replaced = add_go_import(&replaced, &target_import_path);
                }
                return Ok(Some((
                    replaced,
                    format!("rewrote `{old_call}` to `{new_call}`"),
                )));
            }
            Ok(None)
        }
        Language::Cpp | Language::C => {
            let src_hdr = display_relative_or_name(caller_file, source_file);
            let tgt_hdr = display_relative_or_name(caller_file, target_file);
            let old_inc = format!("#include \"{src_hdr}\"");
            let new_inc = format!("#include \"{tgt_hdr}\"");
            if caller_text.contains(&old_inc) && !caller_text.contains(&new_inc) {
                let replaced = caller_text.replace(&old_inc, &format!("{old_inc}\n{new_inc}"));
                return Ok(Some((replaced, format!("included \"{tgt_hdr}\""))));
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}
