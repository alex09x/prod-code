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

use super::specifiers::is_symbol_used;

pub fn format_item_for_target(item: &str, lang: Language) -> String {
    if !matches!(lang, Language::TypeScript | Language::JavaScript) {
        return item.to_string();
    }
    let lines: Vec<&str> = item.lines().collect();
    let mut out = Vec::new();
    let mut exported = false;

    for line in lines {
        let trimmed = line.trim();
        if !exported
            && !trimmed.is_empty()
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("/*")
            && !trimmed.starts_with('*')
            && !trimmed.starts_with('@')
        {
            if trimmed.starts_with("export ") {
                exported = true;
                out.push(line.to_string());
            } else if trimmed.starts_with("function ")
                || trimmed.starts_with("async function ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("var ")
            {
                let indent = &line[..line.len() - line.trim_start().len()];
                out.push(format!("{indent}export {}", line.trim_start()));
                exported = true;
            } else {
                out.push(line.to_string());
            }
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

pub fn check_target_collision(target_text: &str, name: &str, lang: Language) -> Result<()> {
    if target_text.trim().is_empty() {
        return Ok(());
    }
    for (line_num, line) in target_text.lines().enumerate() {
        let trimmed = line.trim();
        let is_decl = match lang {
            Language::TypeScript | Language::JavaScript => {
                trimmed.starts_with("export function ")
                    || trimmed.starts_with("function ")
                    || trimmed.starts_with("export class ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("export interface ")
                    || trimmed.starts_with("interface ")
                    || trimmed.starts_with("export type ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("export const ")
                    || trimmed.starts_with("const ")
                    || trimmed.starts_with("export enum ")
                    || trimmed.starts_with("enum ")
            }
            Language::Python => {
                line.len() - line.trim_start().len() == 0
                    && (trimmed.starts_with("def ")
                        || trimmed.starts_with("async def ")
                        || trimmed.starts_with("class "))
            }
            Language::Go => {
                trimmed.starts_with("func ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("var ")
                    || trimmed.starts_with("const ")
            }
            Language::Swift => {
                trimmed.contains("func ")
                    || trimmed.contains("class ")
                    || trimmed.contains("struct ")
                    || trimmed.contains("enum ")
                    || trimmed.contains("protocol ")
            }
            Language::Cpp | Language::C | Language::Java => {
                trimmed.contains("class ")
                    || trimmed.contains("interface ")
                    || trimmed.contains("record ")
                    || trimmed.contains("enum ")
                    || trimmed.contains(name)
            }
            Language::Rust => false,
        };

        if is_decl && is_symbol_used(line, name) {
            anyhow::bail!("target already declares `{name}` at line {}", line_num + 1);
        }
    }
    Ok(())
}

pub(crate) fn initial_file_header(target: &Path, lang: Language) -> String {
    match lang {
        Language::Go => {
            let is_test_target = target
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with("_test.go"));
            let pkg = target
                .parent()
                .and_then(|dir| {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        let mut regular_pkg = None;
                        let mut test_pkg = None;
                        for e in entries.flatten() {
                            let p = e.path();
                            if p.extension().is_some_and(|ext| ext == "go")
                                && p != target
                                && let Ok(content) = std::fs::read_to_string(&p)
                            {
                                let is_test_file = p
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .is_some_and(|name| name.ends_with("_test.go"));
                                for line in content.lines() {
                                    let t = line.trim();
                                    if let Some(rest) = t.strip_prefix("package ") {
                                        let name = rest
                                            .split("//")
                                            .next()
                                            .unwrap_or(rest)
                                            .trim()
                                            .trim_end_matches(';');
                                        if !name.is_empty() {
                                            if is_test_file {
                                                if test_pkg.is_none() {
                                                    test_pkg = Some(name.to_string());
                                                }
                                            } else {
                                                regular_pkg = Some(name.to_string());
                                                break;
                                            }
                                        }
                                    }
                                }
                                if !is_test_target && regular_pkg.is_some() {
                                    break;
                                }
                            }
                        }
                        if is_test_target {
                            test_pkg.or(regular_pkg)
                        } else {
                            regular_pkg.or_else(|| {
                                test_pkg
                                    .as_deref()
                                    .and_then(|tp| tp.strip_suffix("_test"))
                                    .map(str::to_string)
                            })
                        }
                    } else {
                        None
                    }
                    .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
                })
                .unwrap_or_else(|| "main".to_string());
            format!("package {pkg}\n\n")
        }
        Language::Cpp | Language::C => {
            if let Some(ext) = target.extension().and_then(|e| e.to_str())
                && matches!(ext, "h" | "hpp" | "hxx")
            {
                "#pragma once\n\n".to_string()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

pub(crate) fn cpp_move_target_is_implementation(lang: Language, target: &Path) -> bool {
    matches!(lang, Language::Cpp | Language::C)
        && matches!(
            target.extension().and_then(|ext| ext.to_str()),
            Some("cpp" | "cc" | "cxx" | "c")
        )
}
