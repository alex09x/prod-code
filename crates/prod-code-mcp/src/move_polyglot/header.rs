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

pub(crate) fn is_polyglot_decl_header(line: &str, lang: Language) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    match lang {
        Language::Python => {
            let indent = line.len() - line.trim_start().len();
            indent == 0
                && (trimmed.starts_with("def ")
                    || trimmed.starts_with("async def ")
                    || trimmed.starts_with("class "))
        }
        Language::TypeScript | Language::JavaScript => {
            if trimmed.starts_with("import ")
                || trimmed.starts_with("export {")
                || trimmed.starts_with("export *")
            {
                return false;
            }
            trimmed.starts_with("export default ")
                || trimmed.starts_with("export function ")
                || trimmed.starts_with("export async function ")
                || trimmed.starts_with("function ")
                || trimmed.starts_with("async function ")
                || trimmed.starts_with("export class ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("export interface ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("export type ")
                || trimmed.starts_with("type ")
                || trimmed.starts_with("export enum ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("export const ")
                || trimmed.starts_with("const ")
                || trimmed.starts_with("export let ")
                || trimmed.starts_with("let ")
                || trimmed.starts_with("export var ")
                || trimmed.starts_with("var ")
        }
        Language::Go => {
            let indent = line.len() - line.trim_start().len();
            indent == 0
                && (trimmed.starts_with("func ")
                    || trimmed.starts_with("type ")
                    || trimmed.starts_with("var ")
                    || trimmed.starts_with("const "))
        }
        Language::Swift => {
            let rest = trimmed
                .strip_prefix("public ")
                .or_else(|| trimmed.strip_prefix("open "))
                .or_else(|| trimmed.strip_prefix("internal "))
                .or_else(|| trimmed.strip_prefix("fileprivate "))
                .or_else(|| trimmed.strip_prefix("private "))
                .unwrap_or(trimmed);
            rest.starts_with("func ")
                || rest.starts_with("class ")
                || rest.starts_with("struct ")
                || rest.starts_with("enum ")
                || rest.starts_with("protocol ")
        }
        Language::Cpp | Language::C => {
            let indent = line.len() - line.trim_start().len();
            indent == 0
                && !trimmed.starts_with('#')
                && !trimmed.starts_with("using ")
                && (trimmed.starts_with("class ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("enum ")
                    || (trimmed.contains('(') && (trimmed.contains(')') || trimmed.ends_with('{'))))
        }
        Language::Rust => false,
        Language::Java => {
            trimmed.starts_with("public ")
                || trimmed.starts_with("protected ")
                || trimmed.starts_with("private ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("interface ")
                || trimmed.starts_with("record ")
                || trimmed.starts_with("enum ")
        }
    }
}

pub(crate) fn extract_polyglot_decl_name(line: &str, lang: Language) -> Option<String> {
    let trimmed = line.trim();
    match lang {
        Language::Python => {
            if let Some(pos) = trimmed.find("def ") {
                let after = &trimmed[pos + 4..];
                let paren = after.find('(').unwrap_or(after.len());
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
            if let Some(pos) = trimmed.find("class ") {
                let after = &trimmed[pos + 6..];
                let paren = after
                    .find('(')
                    .or_else(|| after.find(':'))
                    .unwrap_or(after.len());
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
        }
        Language::TypeScript | Language::JavaScript => {
            let rest = trimmed
                .strip_prefix("export default ")
                .or_else(|| trimmed.strip_prefix("export "))
                .unwrap_or(trimmed);
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            for keyword in &[
                "function ",
                "class ",
                "interface ",
                "type ",
                "enum ",
                "const ",
                "let ",
                "var ",
            ] {
                if let Some(after) = rest.strip_prefix(keyword) {
                    let clean = after.trim_start();
                    let name = clean
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
        Language::Go => {
            if let Some(after) = trimmed.strip_prefix("func ") {
                let clean = after.trim_start();
                if clean.starts_with('(')
                    && let Some(close_recv) = clean.find(')')
                {
                    let after_recv = clean[close_recv + 1..].trim_start();
                    let name = after_recv
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    return Some(name.to_string());
                }
                let name = clean
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("");
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
            if let Some(after) = trimmed.strip_prefix("type ") {
                let name = after.split_whitespace().next().unwrap_or("");
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
            if let Some(after) = trimmed
                .strip_prefix("const ")
                .or_else(|| trimmed.strip_prefix("var "))
            {
                let name = after
                    .trim_start()
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("");
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
        Language::Swift => {
            let rest = trimmed
                .strip_prefix("public ")
                .or_else(|| trimmed.strip_prefix("open "))
                .or_else(|| trimmed.strip_prefix("internal "))
                .or_else(|| trimmed.strip_prefix("fileprivate "))
                .or_else(|| trimmed.strip_prefix("private "))
                .unwrap_or(trimmed);
            for keyword in &["func ", "class ", "struct ", "enum ", "protocol "] {
                if let Some(after) = rest.strip_prefix(keyword) {
                    let clean = after.trim_start();
                    let name = clean
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
        Language::Cpp | Language::C => {
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim_start_matches('*').trim_start_matches('&');
                    let member = clean.rsplit("::").next().unwrap_or(clean);
                    if !member.is_empty()
                        && !matches!(member, "if" | "while" | "for" | "switch" | "catch")
                    {
                        return Some(member.to_string());
                    }
                }
            }
            for keyword in &["class ", "struct ", "enum "] {
                if let Some(after) = trimmed.strip_prefix(keyword) {
                    let clean = after.trim_start();
                    let name = clean
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
        Language::Rust => {}
        Language::Java => {
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim();
                    if !clean.is_empty()
                        && !matches!(clean, "if" | "while" | "for" | "switch" | "catch")
                    {
                        return Some(clean.to_string());
                    }
                }
            }
            for keyword in &["class ", "interface ", "record ", "enum "] {
                if let Some(pos) = trimmed.find(keyword) {
                    let after = &trimmed[pos + keyword.len()..];
                    let clean = after.trim_start();
                    let name = clean
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .next()
                        .unwrap_or("");
                    if !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
    }
    None
}
