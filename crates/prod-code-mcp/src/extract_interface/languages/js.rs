/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Extract-interface for JavaScript. JavaScript has no `interface` or `implements`, so the
//! contract is written the way a JavaScript checker reads one: a JSDoc `@typedef` of the
//! methods, and `@implements` on the class. The file stays plain JavaScript and runs as before;
//! `tsc --checkJs` and editors check the class against it (#1462).

use anyhow::{Context, Result, bail};

pub fn extract_interface_js(
    text: &str,
    symbol: &str,
    interface_name: &str,
    target_methods: &[String],
) -> Result<(String, Vec<String>)> {
    let class_pos = find_class(text, symbol)
        .with_context(|| format!("class `{symbol}` not found in JavaScript source"))?;
    let open_brace = text[class_pos..]
        .find('{')
        .map(|idx| class_pos + idx)
        .context("class body opening brace not found")?;
    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)
        .context("class body matching closing brace not found")?;

    let mut extracted = Vec::new();
    let mut properties = Vec::new();
    let mut depth = 0usize;
    for line in text[open_brace + 1..close_brace].lines() {
        let trimmed = line.trim();
        if depth == 0
            && let Some((name, params)) = method_head(trimmed)
            && (target_methods.is_empty() || target_methods.iter().any(|m| m == name))
            && !extracted.iter().any(|m| m == name)
        {
            properties.push(format!(
                " * @property {{({}) => any}} {name}",
                typed_params(params)
            ));
            extracted.push(name.to_string());
        }
        for c in line.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    if let Some(missing) = target_methods.iter().find(|m| !extracted.contains(m)) {
        bail!("`{missing}` is not a public method of class `{symbol}`");
    }
    if extracted.is_empty() {
        bail!("no matching methods found in class `{symbol}` to extract");
    }

    let line_start = text[..class_pos].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let mut contract = format!("{indent}/**\n{indent} * @typedef {{object}} {interface_name}\n");
    for p in &properties {
        contract.push_str(&format!("{indent}{p}\n"));
    }
    contract.push_str(&format!(
        "{indent} */\n\n{indent}/** @implements {{{interface_name}}} */\n"
    ));
    let mut out = text.to_string();
    out.insert_str(line_start, &contract);
    Ok((out, extracted))
}

/// Where `class symbol` is declared: the keyword, as a whole word, followed by the name.
fn find_class(text: &str, symbol: &str) -> Option<usize> {
    let marker = format!("class {symbol}");
    text.match_indices(&marker).map(|(at, _)| at).find(|&at| {
        let before = text[..at].chars().next_back();
        let after = text[at + marker.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
            && !after.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
    })
}

/// The name and parameter text of a public instance method declared on `line`:
/// `name(…) {`, `async name(…) {`, `*name(…) {`. Constructors, `static` and `#private`
/// members, accessors, fields and comments are not part of an instance's contract.
fn method_head(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix("async ").unwrap_or(line);
    let rest = rest.strip_prefix('*').unwrap_or(rest).trim_start();
    let open = rest.find('(')?;
    let name = rest[..open].trim();
    let close = open + rest[open..].find(')')?;
    let ident = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$');
    let opens_body = rest[close + 1..].trim_start().starts_with('{');
    (ident && opens_body && !matches!(name, "constructor" | "if" | "for" | "while" | "switch"))
        .then(|| (name, &rest[open + 1..close]))
}

/// `a, b = 1, ...rest` as `a: any, b?: any, ...rest: any[]`; a destructured parameter is named
/// by its position.
fn typed_params(params: &str) -> String {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut pieces = Vec::new();
    for (i, c) in params.char_indices() {
        match c {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                pieces.push(&params[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    pieces.push(&params[start..]);
    for (i, piece) in pieces.iter().map(|p| p.trim()).enumerate() {
        if piece.is_empty() {
            continue;
        }
        let (head, optional) = match piece.split_once('=') {
            Some((head, _)) => (head.trim(), true),
            None => (piece, false),
        };
        if let Some(name) = head.strip_prefix("...") {
            let name = if is_ident(name) {
                name.to_string()
            } else {
                format!("rest{i}")
            };
            out.push(format!("...{name}: any[]"));
        } else {
            let name = if is_ident(head) {
                head.to_string()
            } else {
                format!("arg{i}")
            };
            out.push(format!("{name}{}: any", if optional { "?" } else { "" }));
        }
    }
    out.join(", ")
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}
