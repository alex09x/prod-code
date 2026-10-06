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
use super::types::{
    PolyglotShape, find_matching_paren, split_comma_top_level, strip_comments, strip_go_tags,
};

/// Parses Go `struct` and `interface` declarations.
pub fn parse_go_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    let open = t.find('{')?;
    let close = t.rfind('}')?;
    let header = &t[..open];
    let body = &t[open + 1..close];

    if header.contains("interface") {
        let mut methods = Vec::new();
        for line in body.lines() {
            let line = strip_comments(line);
            if line.is_empty() {
                continue;
            }
            if let Some(m) = parse_go_method_signature(line) {
                methods.push(m);
            }
        }
        return Some(PolyglotShape::Interface { methods });
    }

    if header.contains("struct") {
        let mut fields = Vec::new();
        for line in body.lines() {
            let line = strip_comments(line);
            let line = strip_go_tags(line);
            if line.is_empty() {
                continue;
            }
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.is_empty() {
                continue;
            }
            if tokens.len() == 1 {
                // Embedded type e.g. `*Config` or `sync.Mutex`
                let raw = tokens[0];
                let name = raw
                    .trim_start_matches('*')
                    .rsplit('.')
                    .next()
                    .unwrap_or(raw);
                fields.push((name.to_string(), raw.to_string()));
            } else {
                let ty = tokens.last()?.to_string();
                let names_part = tokens[..tokens.len() - 1].join(" ");
                for name in names_part.split(',') {
                    let name = name.trim();
                    if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        fields.push((name.to_string(), ty.clone()));
                    }
                }
            }
        }
        return Some(PolyglotShape::Record(fields));
    }

    None
}

fn parse_go_method_signature(line: &str) -> Option<MethodSignature> {
    let line = line.trim();
    let open_paren = line.find('(')?;
    let name = line[..open_paren].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let rest = &line[open_paren + 1..];
    let close_paren = find_matching_paren(rest)?;
    let params_str = &rest[..close_paren];
    let returns_str = rest[close_paren + 1..].trim();

    let params = parse_go_parameters(params_str)?;

    let return_type = if returns_str.is_empty() {
        None
    } else {
        Some(parse_go_result_types(returns_str)?)
    };

    Some(MethodSignature {
        name: name.to_string(),
        params,
        return_type,
    })
}

fn parse_go_parameters(parameters: &str) -> Option<Vec<(String, String)>> {
    let parts = split_comma_top_level(parameters);
    let mut params = Vec::new();
    let mut pending_names = Vec::new();
    for (index, raw) in parts.iter().enumerate() {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((names_part, ty)) = part.split_once(char::is_whitespace) {
            let mut names = pending_names.drain(..).collect::<Vec<_>>();
            names.extend(names_part.split_whitespace().map(str::to_string));
            let ty = ty.trim();
            if names.is_empty() || ty.is_empty() {
                return None;
            }
            for name in names {
                params.push((name, ty.to_string()));
            }
        } else if parts
            .get(index + 1)
            .is_some_and(|next| next.split_once(char::is_whitespace).is_some())
            && !go_type_form(part)
        {
            pending_names.push(part.to_string());
        } else {
            if !pending_names.is_empty() {
                return None;
            }
            params.push((String::new(), part.to_string()));
        }
    }
    if !pending_names.is_empty() {
        return None;
    }
    Some(params)
}

fn parse_go_result_types(results: &str) -> Option<String> {
    let results = results.trim();
    let inside = if results.starts_with('(') && results.ends_with(')') {
        &results[1..results.len() - 1]
    } else {
        results
    };
    let mut types = Vec::new();
    let parts = split_comma_top_level(inside);
    let mut pending_names = Vec::new();
    for (index, raw) in parts.iter().enumerate() {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((names_part, ty)) = part.split_once(char::is_whitespace)
            && !go_type_form(part)
        {
            if !pending_names.is_empty() {
                types.extend(std::iter::repeat_n(
                    ty.trim().to_string(),
                    pending_names.len(),
                ));
                pending_names.clear();
            }
            types.extend(names_part.split_whitespace().map(|_| ty.trim().to_string()));
        } else if parts
            .get(index + 1)
            .is_some_and(|next| next.split_once(char::is_whitespace).is_some())
            && !go_type_form(part)
        {
            pending_names.push(part.to_string());
        } else {
            if !pending_names.is_empty() {
                return None;
            }
            types.push(part.to_string());
        }
    }
    if !pending_names.is_empty() {
        return None;
    }
    (!types.is_empty()).then(|| types.join(", "))
}

fn go_type_form(value: &str) -> bool {
    matches!(
        value,
        "bool"
            | "string"
            | "error"
            | "byte"
            | "rune"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
            | "any"
    ) || value.starts_with(['*', '['])
        || value.starts_with("map[")
        || value.starts_with("chan")
        || value.starts_with("<-chan")
        || value.starts_with("func")
        || value.starts_with("struct {")
        || value.starts_with("interface {")
        || value.contains('.')
        || value.chars().next().is_some_and(char::is_uppercase)
}

/// Parses TypeScript / JavaScript `interface`, `type`, and `enum`.
pub fn parse_ts_shape(decl: &str) -> Option<PolyglotShape> {
    let t = decl.trim();
    if t.contains("enum ") {
        let open = t.find('{')?;
        let close = t.rfind('}')?;
        let variants = t[open + 1..close]
            .lines()
            .map(strip_comments)
            .filter(|l| !l.is_empty())
            .map(|l| {
                let name = l.split(['=', ',']).next().unwrap_or(l).trim();
                name.to_string()
            })
            .filter(|n| !n.is_empty())
            .collect();
        return Some(PolyglotShape::Enum(variants));
    }

    let open = t.find('{')?;
    let close = t.rfind('}')?;
    let body = &t[open + 1..close];

    let mut fields = Vec::new();
    let mut methods = Vec::new();

    for raw in body.split([';', '\n']) {
        let line = strip_comments(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(open_paren) = line.find('(')
            && let Some(colon) = line.find(':')
            && open_paren < colon
        {
            let name = line[..open_paren]
                .trim()
                .trim_start_matches("async ")
                .trim();
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                let rest = &line[open_paren + 1..];
                if let Some(close_paren) = find_matching_paren(rest) {
                    let params_str = &rest[..close_paren];
                    let ret_str = rest[close_paren + 1..]
                        .trim()
                        .trim_start_matches(':')
                        .trim();
                    let mut params = Vec::new();
                    for part in split_comma_top_level(params_str) {
                        let part = part.trim();
                        if let Some((pn, pt)) = part.split_once(':') {
                            params.push((
                                pn.trim().trim_end_matches('?').to_string(),
                                pt.trim().to_string(),
                            ));
                        }
                    }
                    methods.push(MethodSignature {
                        name: name.to_string(),
                        params,
                        return_type: Some(ret_str.to_string()),
                    });
                    continue;
                }
            }
        }

        // Check for property: name: type or name?: type
        if let Some((name_part, ty_part)) = line.split_once(':') {
            let name = name_part
                .trim()
                .trim_start_matches("readonly ")
                .trim_start_matches("public ")
                .trim_end_matches('?')
                .trim();
            let ty = ty_part
                .trim()
                .trim_end_matches(',')
                .trim_end_matches(';')
                .trim();
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
            {
                fields.push((name.to_string(), ty.to_string()));
            }
        }
    }

    if !methods.is_empty() && !fields.is_empty() {
        Some(PolyglotShape::InterfaceWithFields { methods, fields })
    } else if !methods.is_empty() {
        Some(PolyglotShape::Interface { methods })
    } else if !fields.is_empty() {
        Some(PolyglotShape::Record(fields))
    } else {
        Some(PolyglotShape::Record(Vec::new()))
    }
}
