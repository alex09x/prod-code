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

use super::go_imports::{add_go_import, get_go_package, rewrite_go_cross_pkg_in_source};
use super::specifiers::{
    display_relative_or_name, is_symbol_used, python_module_specifier, relative_import_specifier,
};

pub(crate) fn top_import_insertion_pos(content: &str) -> usize {
    let mut pos = 0;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("#!")
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed == "\"use strict\";"
            || trimmed == "'use strict';"
        {
            pos += line.len() + 1;
            continue;
        }
        break;
    }
    pos.min(content.len())
}

pub fn insert_or_merge_ts_import(content: &str, sym: &str, rel_path: &str) -> (String, bool) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") && trimmed.contains(rel_path) {
            if is_symbol_used(line, sym) {
                return (content.to_string(), false);
            }
            if let Some(open) = line.find('{')
                && let Some(close) = line.find('}')
            {
                let inside = line[open + 1..close].trim();
                let new_inside = if inside.is_empty() {
                    sym.to_string()
                } else {
                    format!("{inside}, {sym}")
                };
                let new_line = format!("{}{new_inside}{}", &line[..open + 1], &line[close..]);
                let replaced = content.replace(line, &new_line);
                return (replaced, true);
            }
        }
    }
    let import_line = format!("import {{ {sym} }} from \"{rel_path}\";\n");
    let insert_pos = top_import_insertion_pos(content);
    let mut out = String::with_capacity(content.len() + import_line.len());
    out.push_str(&content[..insert_pos]);
    out.push_str(&import_line);
    out.push_str(&content[insert_pos..]);
    (out, true)
}

pub fn insert_or_merge_py_import(content: &str, sym: &str, mod_spec: &str) -> (String, bool) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("from ")
            && trimmed.contains(mod_spec)
            && trimmed.contains(" import ")
        {
            if is_symbol_used(line, sym) {
                return (content.to_string(), false);
            }
            let new_line = format!("{line}, {sym}");
            let replaced = content.replace(line, &new_line);
            return (replaced, true);
        }
    }
    let import_line = format!("from {mod_spec} import {sym}\n");
    let insert_pos = top_import_insertion_pos(content);
    let mut out = String::with_capacity(content.len() + import_line.len());
    out.push_str(&content[..insert_pos]);
    out.push_str(&import_line);
    out.push_str(&content[insert_pos..]);
    (out, true)
}

pub(crate) fn remove_from_braced_ts_import(line: &str, sym: &str) -> (String, bool) {
    let Some(open) = line.find('{') else {
        return (line.to_string(), false);
    };
    let Some(close) = line.find('}') else {
        return (line.to_string(), false);
    };
    if open >= close {
        return (line.to_string(), false);
    }
    let inside = &line[open + 1..close];
    let items: Vec<&str> = inside
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let mut new_items = Vec::new();
    let mut removed = false;
    for item in items {
        let first_word = item.split_whitespace().next().unwrap_or("");
        if first_word == sym {
            removed = true;
        } else {
            new_items.push(item);
        }
    }
    if !removed {
        return (line.to_string(), false);
    }
    if new_items.is_empty() {
        return (String::new(), true);
    }
    let indent = &line[..line.len() - line.trim_start().len()];
    let after_close = &line[close + 1..];
    let rejoined = format!("{indent}import {{ {} }}{after_close}", new_items.join(", "));
    (rejoined, true)
}

pub(crate) fn remove_from_py_from_import(line: &str, sym: &str) -> (String, bool) {
    let Some(pos) = line.find(" import ") else {
        return (line.to_string(), false);
    };
    let before = &line[..pos + 8];
    let after = &line[pos + 8..];
    let items: Vec<&str> = after
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let mut new_items = Vec::new();
    let mut removed = false;
    for item in items {
        let first_word = item.split_whitespace().next().unwrap_or("");
        if first_word == sym {
            removed = true;
        } else {
            new_items.push(item);
        }
    }
    if !removed {
        return (line.to_string(), false);
    }
    if new_items.is_empty() {
        return (String::new(), true);
    }
    let rejoined = format!("{before}{}", new_items.join(", "));
    (rejoined, true)
}

pub fn carry_imports_polyglot(
    source_text: &str,
    item_text: &str,
    target_text: &str,
    source_file: &Path,
    target_file: &Path,
    lang: Language,
    _root: &Path,
) -> (String, Vec<String>) {
    let mut out = target_text.to_string();
    let mut notes = Vec::new();

    match lang {
        Language::TypeScript | Language::JavaScript => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if !trimmed.starts_with("import ") {
                    continue;
                }
                let Some(from_idx) = trimmed.find(" from ") else {
                    continue;
                };
                let specifier = trimmed[from_idx + 6..]
                    .trim()
                    .trim_matches(['\x27', '"', ';'].as_slice());
                if let Some(open) = trimmed.find('{')
                    && let Some(close) = trimmed.find('}')
                {
                    let inside = &trimmed[open + 1..close];
                    for raw_sym in inside.split(',') {
                        let sym = raw_sym.split_whitespace().next().unwrap_or("");
                        if !sym.is_empty()
                            && is_symbol_used(item_text, sym)
                            && !is_symbol_used(&out, sym)
                        {
                            let target_spec = if specifier.starts_with('.') {
                                let source_dir = source_file.parent().unwrap_or(Path::new(""));
                                let resolved = source_dir.join(specifier);
                                relative_import_specifier(target_file, &resolved)
                            } else {
                                specifier.to_string()
                            };
                            let (new_out, added) =
                                insert_or_merge_ts_import(&out, sym, &target_spec);
                            if added {
                                out = new_out;
                                notes.push(format!("carried import `{sym}` from `{target_spec}`"));
                            }
                        }
                    }
                }
            }
        }
        Language::Python => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("from ") || trimmed.starts_with("import ") {
                    let words: Vec<&str> = trimmed.split_whitespace().collect();
                    for &w in &words {
                        let clean = w.trim_matches(['(', ')', ',', ':'].as_slice());
                        if !clean.is_empty()
                            && clean != "from"
                            && clean != "import"
                            && clean != "as"
                            && is_symbol_used(item_text, clean)
                            && !out.contains(clean)
                            && !out.contains(trimmed)
                        {
                            out = format!("{trimmed}\n{out}");
                            notes.push(format!("carried `{trimmed}`"));
                            break;
                        }
                    }
                }
            }
        }
        Language::Go => {
            let go_packages = [
                "fmt", "strings", "os", "io", "time", "errors", "sync", "bytes", "math", "path",
                "sort",
            ];
            for pkg in go_packages {
                let needle = format!("{pkg}.");
                if item_text.contains(&needle) && !out.contains(&format!("\"{pkg}\"")) {
                    out = add_go_import(&out, pkg);
                    notes.push(format!("carried import \"{pkg}\""));
                }
            }
        }
        Language::Swift => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    let mod_name = trimmed.strip_prefix("import ").unwrap_or("").trim();
                    if !mod_name.is_empty() && !out.contains(trimmed) {
                        out = format!("{trimmed}\n{out}");
                        notes.push(format!("carried `{trimmed}`"));
                    }
                }
            }
        }
        Language::Cpp | Language::C => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("#include") && !out.contains(trimmed) {
                    out = format!("{trimmed}\n{out}");
                    notes.push(format!("carried `{trimmed}`"));
                }
            }
        }
        Language::Rust => {}
        Language::Java => {
            for line in source_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") && !out.contains(trimmed) {
                    out = format!("{trimmed}\n{out}");
                    notes.push(format!("carried `{trimmed}`"));
                }
            }
        }
    }

    (out, notes)
}

pub fn update_source_imports(
    source_text: &str,
    decl_name: &str,
    source_file: &Path,
    target_file: &Path,
    root: &Path,
    lang: Language,
) -> Result<(String, Option<String>)> {
    if !is_symbol_used(source_text, decl_name) {
        return Ok((source_text.to_string(), None));
    }

    match lang {
        Language::TypeScript | Language::JavaScript => {
            let rel = relative_import_specifier(source_file, target_file);
            let (new_text, added) = insert_or_merge_ts_import(source_text, decl_name, &rel);
            let note = if added {
                Some(format!("imported `{decl_name}` from `{rel}`"))
            } else {
                None
            };
            Ok((new_text, note))
        }
        Language::Python => {
            let mod_spec = python_module_specifier(source_file, target_file, root);
            let (new_text, added) = insert_or_merge_py_import(source_text, decl_name, &mod_spec);
            let note = if added {
                Some(format!("from {mod_spec} import {decl_name}"))
            } else {
                None
            };
            Ok((new_text, note))
        }
        Language::Go => {
            let src_pkg = get_go_package(source_file);
            let tgt_pkg = get_go_package(target_file);
            if source_file.parent() == target_file.parent() {
                Ok((
                    source_text.to_string(),
                    Some(format!("same package `{src_pkg}`: direct access")),
                ))
            } else {
                let (new_text, note) = rewrite_go_cross_pkg_in_source(
                    source_text,
                    decl_name,
                    &tgt_pkg,
                    target_file,
                    root,
                )?;
                Ok((new_text, (!note.is_empty()).then_some(note)))
            }
        }
        Language::Cpp | Language::C => {
            let rel = display_relative_or_name(source_file, target_file);
            let include_line = format!("#include \"{rel}\"");
            if source_text.contains(&include_line) {
                Ok((source_text.to_string(), None))
            } else {
                let new_text = format!("{include_line}\n{source_text}");
                Ok((new_text, Some(format!("included \"{rel}\""))))
            }
        }
        Language::Swift => Ok((
            source_text.to_string(),
            Some("same module: direct access".to_string()),
        )),
        Language::Rust => Ok((source_text.to_string(), None)),
        Language::Java => Ok((source_text.to_string(), None)),
    }
}
