/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lang::{Language, is_ident};
use super::tokenize::is_keyword;

pub fn infer_param_type(name: &str, scope: &str, lang: Language) -> Option<String> {
    match lang {
        Language::Python | Language::JavaScript => None,
        Language::TypeScript => {
            let pat = format!("{name}:");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| {
                        is_ident(*c) || *c == '<' || *c == '>' || *c == '[' || *c == ']'
                    })
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            let pat_eq = format!("{name} =");
            if let Some(pos) = scope.find(&pat_eq) {
                let rest = scope[pos + pat_eq.len()..].trim_start();
                if rest.starts_with('"') || rest.starts_with('\'') || rest.starts_with('`') {
                    return Some("string".into());
                }
                if rest.starts_with(|c: char| c.is_ascii_digit()) {
                    return Some("number".into());
                }
                if rest.starts_with("true") || rest.starts_with("false") {
                    return Some("boolean".into());
                }
            }
            Some("any".into())
        }
        Language::Go => {
            let pat = format!("{name} ");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| is_ident(*c) || *c == '[' || *c == ']' || *c == '*')
                    .collect();
                if !ty.is_empty() && !is_keyword(&ty, lang) {
                    return Some(ty);
                }
            }
            let pat_walrus = format!("{name} :=");
            if let Some(pos) = scope.find(&pat_walrus) {
                let rest = scope[pos + pat_walrus.len()..].trim_start();
                if rest.starts_with('"') {
                    return Some("string".into());
                }
                if rest.starts_with(|c: char| c.is_ascii_digit()) {
                    if rest
                        .split(|c: char| !c.is_ascii_digit() && c != '.')
                        .next()
                        .unwrap_or("")
                        .contains('.')
                    {
                        return Some("float64".into());
                    }
                    return Some("int".into());
                }
                if rest.starts_with("true") || rest.starts_with("false") {
                    return Some("bool".into());
                }
            }
            Some("int".into())
        }
        Language::Cpp | Language::C => {
            let pat = format!(" {name}");
            if let Some(pos) = scope.find(&pat) {
                let before = scope[..pos].trim_end();
                let ty: String = before
                    .chars()
                    .rev()
                    .take_while(|c| is_ident(*c) || *c == '*' || *c == '&')
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                let ty_trimmed = ty.trim_matches(|c| c == '*' || c == '&');
                if is_ident(ty_trimmed.chars().next().unwrap_or(' '))
                    && (!is_keyword(ty_trimmed, lang)
                        || matches!(
                            ty_trimmed,
                            "int"
                                | "double"
                                | "float"
                                | "bool"
                                | "char"
                                | "size_t"
                                | "long"
                                | "short"
                                | "unsigned"
                                | "signed"
                        ))
                {
                    return Some(ty);
                }
            }
            Some("int".into())
        }
        Language::Swift => {
            let pat = format!("{name}:");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| is_ident(*c) || *c == '<' || *c == '>')
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            Some("Double".into())
        }
        Language::Rust => Some("usize".into()),
        Language::Java => {
            let pat = format!(" {name}");
            if let Some(pos) = scope.find(&pat) {
                let before = scope[..pos].trim_end();
                let ty: String = before
                    .chars()
                    .rev()
                    .take_while(|c| {
                        is_ident(*c) || *c == '<' || *c == '>' || *c == '[' || *c == ']'
                    })
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if is_ident(ty.chars().next().unwrap_or(' '))
                    && (!is_keyword(&ty, lang)
                        || matches!(
                            ty.as_str(),
                            "int"
                                | "double"
                                | "float"
                                | "boolean"
                                | "char"
                                | "long"
                                | "short"
                                | "byte"
                                | "String"
                                | "Object"
                        ))
                {
                    return Some(ty);
                }
            }
            Some("Object".into())
        }
        Language::Csharp => {
            let pat = format!(" {name}");
            if let Some(pos) = scope.find(&pat) {
                let before = scope[..pos].trim_end();
                let ty: String = before
                    .chars()
                    .rev()
                    .take_while(|c| {
                        is_ident(*c)
                            || *c == '<'
                            || *c == '>'
                            || *c == '['
                            || *c == ']'
                            || *c == '?'
                    })
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                if is_ident(ty.chars().next().unwrap_or(' '))
                    && (!is_keyword(&ty, lang)
                        || matches!(
                            ty.as_str(),
                            "int"
                                | "double"
                                | "float"
                                | "bool"
                                | "char"
                                | "long"
                                | "short"
                                | "byte"
                                | "string"
                                | "object"
                        ))
                {
                    return Some(ty);
                }
            }
            Some("object".into())
        }
        Language::Kotlin => {
            let pat = format!("{name}:");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| {
                        is_ident(*c)
                            || *c == '<'
                            || *c == '>'
                            || *c == '['
                            || *c == ']'
                            || *c == '?'
                    })
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            let pat_space = format!("{name} : ");
            if let Some(pos) = scope.find(&pat_space) {
                let rest = scope[pos + pat_space.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| {
                        is_ident(*c)
                            || *c == '<'
                            || *c == '>'
                            || *c == '['
                            || *c == ']'
                            || *c == '?'
                    })
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            let pat_eq = format!("{name} =");
            if let Some(pos) = scope.find(&pat_eq) {
                let rest = scope[pos + pat_eq.len()..].trim_start();
                if rest.starts_with('"') {
                    return Some("String".into());
                }
                if rest.starts_with(|c: char| c.is_ascii_digit()) {
                    if rest.contains('.') {
                        return Some("Double".into());
                    }
                    return Some("Int".into());
                }
                if rest.starts_with("true") || rest.starts_with("false") {
                    return Some("Boolean".into());
                }
            }
            Some("Any".into())
        }
        Language::Zig => {
            let pat = format!("{name}:");
            if let Some(pos) = scope.find(&pat) {
                let rest = scope[pos + pat.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| {
                        is_ident(*c)
                            || *c == '['
                            || *c == ']'
                            || *c == '*'
                            || *c == '?'
                            || *c == '!'
                    })
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            let pat_space = format!("{name} : ");
            if let Some(pos) = scope.find(&pat_space) {
                let rest = scope[pos + pat_space.len()..].trim_start();
                let ty: String = rest
                    .chars()
                    .take_while(|c| {
                        is_ident(*c)
                            || *c == '['
                            || *c == ']'
                            || *c == '*'
                            || *c == '?'
                            || *c == '!'
                    })
                    .collect();
                if !ty.is_empty() {
                    return Some(ty);
                }
            }
            Some("anytype".into())
        }
    }
}
