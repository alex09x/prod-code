/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::mock::MethodSignature;
use super::parsers_go_ts::{parse_go_shape, parse_ts_shape};
use super::types::{PolyglotShape, find_matching_paren, split_comma_top_level, strip_comments};
use crate::parameter_object::Language;

/// Parses a declaration into a `PolyglotShape` based on the file language.
pub fn parse_polyglot_shape(decl: &str, language: Language) -> Option<PolyglotShape> {
    match language {
        Language::Go => parse_go_shape(decl),
        Language::TypeScript | Language::JavaScript => parse_ts_shape(decl),
        Language::Python => parse_python_shape(decl),
        Language::Rust => parse_rust_shape(decl),
        Language::Cpp | Language::C => parse_cpp_shape(decl),
        Language::Swift => parse_swift_shape(decl),
        Language::Java => parse_cpp_shape(decl),
    }
}

/// Parses Python `@dataclass`, `BaseModel`, `class with __init__`, or `Protocol`.
pub fn parse_python_shape(decl: &str) -> Option<PolyglotShape> {
    let lines: Vec<&str> = decl.lines().collect();
    let mut fields = Vec::new();
    let mut methods = Vec::new();
    let mut in_init = false;

    for line in lines {
        let trimmed = strip_comments(line);
        if trimmed.is_empty() {
            continue;
        }

        // Check for __init__
        if trimmed.starts_with("def __init__") {
            in_init = true;
            if let Some(open) = trimmed.find('(') {
                let rest = trimmed[open + 1..]
                    .trim_end_matches(':')
                    .trim_end_matches(')');
                for part in split_comma_top_level(rest) {
                    let part = part.trim();
                    if part == "self" || part.is_empty() {
                        continue;
                    }
                    if let Some((name_part, ty_part)) = part.split_once(':') {
                        let name = name_part.trim();
                        let ty = ty_part.split('=').next().unwrap_or(ty_part).trim();
                        fields.push((name.to_string(), ty.to_string()));
                    } else if let Some((name_part, _)) = part.split_once('=') {
                        fields.push((name_part.trim().to_string(), String::new()));
                    } else {
                        fields.push((part.to_string(), String::new()));
                    }
                }
            }
            continue;
        }

        // Check for methods
        if trimmed.starts_with("def ") && !trimmed.starts_with("def __") {
            in_init = false;
            let sig = trimmed.trim_start_matches("def ");
            if let Some(open) = sig.find('(') {
                let name = sig[..open].trim();
                let rest = &sig[open + 1..];
                if let Some(close) = find_matching_paren(rest) {
                    let params_str = &rest[..close];
                    let ret_part = rest[close + 1..].trim().trim_end_matches(':').trim();
                    let return_type = ret_part.strip_prefix("->").map(|r| r.trim().to_string());

                    let mut params = Vec::new();
                    for part in split_comma_top_level(params_str) {
                        let part = part.trim();
                        if part == "self" || part.is_empty() {
                            continue;
                        }
                        if let Some((pn, pt)) = part.split_once(':') {
                            params.push((pn.trim().to_string(), pt.trim().to_string()));
                        } else {
                            params.push((part.to_string(), String::new()));
                        }
                    }

                    methods.push(MethodSignature {
                        name: name.to_string(),
                        params,
                        return_type,
                    });
                }
            }
            continue;
        }

        // Check for field: type
        if !in_init
            && !trimmed.starts_with("def ")
            && !trimmed.starts_with("class ")
            && !trimmed.starts_with('@')
            && let Some((name_part, ty_part)) = trimmed.split_once(':')
        {
            let name = name_part.trim();
            let ty = ty_part.split('=').next().unwrap_or(ty_part).trim();
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                fields.push((name.to_string(), ty.to_string()));
            }
        }
    }

    if !methods.is_empty() && fields.is_empty() {
        Some(PolyglotShape::Interface { methods })
    } else if !fields.is_empty() {
        Some(PolyglotShape::Record(fields))
    } else if !methods.is_empty() {
        Some(PolyglotShape::Interface { methods })
    } else {
        Some(PolyglotShape::Record(Vec::new()))
    }
}

/// Parses Rust `struct`, `enum`, or `trait`.
pub fn parse_rust_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    if t.starts_with("pub trait ") || t.starts_with("trait ") {
        let open = t.find('{')?;
        let close = t.rfind('}')?;
        let body = &t[open + 1..close];
        let mut methods = Vec::new();
        for line in body.lines() {
            let line = strip_comments(line);
            let line = line.trim();
            if line.starts_with("fn ") {
                let rest = line.trim_start_matches("fn ");
                if let Some(open_paren) = rest.find('(') {
                    let name = rest[..open_paren].trim();
                    let after = &rest[open_paren + 1..];
                    if let Some(close_paren) = find_matching_paren(after) {
                        let params_str = &after[..close_paren];
                        let ret_part = after[close_paren + 1..].trim().trim_end_matches(';').trim();
                        let return_type = ret_part.strip_prefix("->").map(|r| r.trim().to_string());

                        let mut params = Vec::new();
                        for part in split_comma_top_level(params_str) {
                            let part = part.trim();
                            if part.is_empty() {
                                continue;
                            }
                            if part == "&self" || part == "&mut self" || part == "self" {
                                params.push((part.to_string(), String::new()));
                            } else if let Some((pn, pt)) = part.split_once(':') {
                                params.push((pn.trim().to_string(), pt.trim().to_string()));
                            }
                        }

                        methods.push(MethodSignature {
                            name: name.to_string(),
                            params,
                            return_type,
                        });
                    }
                }
            }
        }
        return Some(PolyglotShape::Interface { methods });
    }

    if let Some(shape) = super::super::resolve::parse_shape(decl) {
        return Some(match shape {
            super::super::types::Shape::Record(f) => PolyglotShape::Record(f),
            super::super::types::Shape::Tuple(t) => PolyglotShape::Tuple(t),
            super::super::types::Shape::Unit => PolyglotShape::Unit,
            super::super::types::Shape::Enum(v) => PolyglotShape::Enum(v),
        });
    }
    None
}

/// Parses C / C++ struct, class, or pure-virtual interface.
pub fn parse_cpp_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    let open = t.find('{')?;
    let close = t.rfind('}')?;
    let body = &t[open + 1..close];

    let mut fields = Vec::new();
    let mut methods = Vec::new();

    for raw in body.split(';') {
        let line = strip_comments(raw);
        let line = line
            .trim()
            .trim_start_matches("public:")
            .trim_start_matches("private:")
            .trim_start_matches("protected:")
            .trim();
        if line.is_empty() {
            continue;
        }

        // Virtual method e.g. `virtual void run(int code) = 0`
        if line.starts_with("virtual ") {
            let rest = line.trim_start_matches("virtual ").trim();
            if let Some(open_paren) = rest.find('(') {
                let ret_and_name = rest[..open_paren].trim();
                let after = &rest[open_paren + 1..];
                if let Some(close_paren) = find_matching_paren(after) {
                    let params_str = &after[..close_paren];
                    let tokens: Vec<&str> = ret_and_name.split_whitespace().collect();
                    if tokens.len() >= 2 {
                        let name = tokens.last()?.trim_start_matches('*');
                        let ret = tokens[..tokens.len() - 1].join(" ");
                        let mut params = Vec::new();
                        for part in split_comma_top_level(params_str) {
                            let part = part.trim();
                            if !part.is_empty() {
                                let ptoks: Vec<&str> = part.split_whitespace().collect();
                                if ptoks.len() >= 2 {
                                    params.push((
                                        ptoks.last().unwrap().to_string(),
                                        ptoks[..ptoks.len() - 1].join(" "),
                                    ));
                                } else {
                                    params.push((String::new(), part.to_string()));
                                }
                            }
                        }
                        methods.push(MethodSignature {
                            name: name.to_string(),
                            params,
                            return_type: Some(ret),
                        });
                        continue;
                    }
                }
            }
        }

        // Field e.g. `std::string host` or `int port = 0`
        let decl_part = line.split('=').next().unwrap_or(line).trim();
        let tokens: Vec<&str> = decl_part.split_whitespace().collect();
        if tokens.len() >= 2 {
            let name = tokens.last()?.trim_start_matches('*');
            let ty = tokens[..tokens.len() - 1].join(" ");
            if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                fields.push((name.to_string(), ty));
            }
        }
    }

    if !methods.is_empty() && fields.is_empty() {
        Some(PolyglotShape::Interface { methods })
    } else {
        Some(PolyglotShape::Record(fields))
    }
}

/// Parses Swift `struct`, `class`, or `protocol`.
pub fn parse_swift_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    let open = t.find('{')?;
    let close = t.rfind('}')?;
    let header = &t[..open];
    let body = &t[open + 1..close];

    if header.contains("protocol ") {
        let mut methods = Vec::new();
        for line in body.lines() {
            let line = strip_comments(line);
            let line = line.trim();
            if line.starts_with("func ") {
                let rest = line.trim_start_matches("func ");
                if let Some(open_paren) = rest.find('(') {
                    let name = rest[..open_paren].trim();
                    let after = &rest[open_paren + 1..];
                    if let Some(close_paren) = find_matching_paren(after) {
                        let params_str = &after[..close_paren];
                        let ret_part = after[close_paren + 1..].trim();
                        let return_type = ret_part.strip_prefix("->").map(|r| r.trim().to_string());

                        let mut params = Vec::new();
                        for part in split_comma_top_level(params_str) {
                            let part = part.trim();
                            if !part.is_empty()
                                && let Some((pn, pt)) = part.split_once(':')
                            {
                                params.push((pn.trim().to_string(), pt.trim().to_string()));
                            }
                        }

                        methods.push(MethodSignature {
                            name: name.to_string(),
                            params,
                            return_type,
                        });
                    }
                }
            }
        }
        return Some(PolyglotShape::Interface { methods });
    }

    let mut fields = Vec::new();
    for line in body.lines() {
        let line = strip_comments(line);
        let line = line.trim();
        if line.starts_with("var ") || line.starts_with("let ") {
            let rest = line
                .trim_start_matches("var ")
                .trim_start_matches("let ")
                .trim();
            if let Some((name_part, ty_part)) = rest.split_once(':') {
                let name = name_part.trim();
                let ty = ty_part.split('=').next().unwrap_or(ty_part).trim();
                if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    fields.push((name.to_string(), ty.to_string()));
                }
            }
        }
    }

    Some(PolyglotShape::Record(fields))
}
