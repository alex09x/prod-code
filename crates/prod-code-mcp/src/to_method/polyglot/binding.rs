/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::caller_migration::lexical_code_mask;
use crate::make_static::Language;
use crate::parameter_object::Language as MaskLanguage;
use anyhow::{Result, ensure};

pub(super) fn mask(text: &str, lang: Language) -> Vec<bool> {
    match lang {
        Language::Cpp => super::cpp_lex::scan(text).0,
        Language::Swift => super::swift_regex::executable_mask(text),
        Language::TypeScript => super::ts_mask::executable_mask(text),
        Language::Python => lexical_code_mask(text, MaskLanguage::Python),
        Language::Go => lexical_code_mask(text, MaskLanguage::Go),
    }
}

pub(super) fn ident(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

pub(super) fn parameter_close(text: &str, lang: Language) -> Result<usize> {
    let code = mask(text, lang);
    let open = text
        .find('(')
        .ok_or_else(|| anyhow::anyhow!("unsupported missing parameter list"))?;
    let mut depth = 1;
    for (i, byte) in text.bytes().enumerate().skip(open + 1) {
        if code[i] {
            if byte == b'(' {
                depth += 1;
            }
            if byte == b')' {
                depth -= 1;
            }
            if depth == 0 {
                return Ok(i);
            }
        }
    }
    anyhow::bail!("unsupported unclosed parameter list")
}

pub(super) fn words(text: &str, lang: Language) -> Vec<(usize, &str)> {
    let code = mask(text, lang);
    let mut result = Vec::new();
    let mut start = None;
    for (i, ch) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        if i < code.len() && code[i] && (ch.is_alphanumeric() || ch == '_' || ch == '$') {
            start.get_or_insert(i);
        } else if let Some(at) = start.take() {
            result.push((at, &text[at..i]));
        }
    }
    result
}

fn tokens(text: &str, lang: Language) -> Vec<(usize, usize, String)> {
    let code = mask(text, lang);
    let mut result = Vec::new();
    let mut start = None;
    let mut decoded = String::new();
    let mut at = 0;
    while at < text.len() {
        if code[at]
            && lang == Language::TypeScript
            && let Some((end, ch)) = super::ts_return::unicode_escape(text, at)
            && code[at..end].iter().all(|b| *b)
        {
            start.get_or_insert(at);
            decoded.push(ch);
            at = end;
            continue;
        }
        let ch = text[at..].chars().next().expect("character boundary");
        let valid = ch.is_alphanumeric()
            || ch == '_'
            || ch == '$'
            || (lang == Language::Python && unicode_ident::is_xid_continue(ch))
            || (lang == Language::TypeScript
                && !ch.is_ascii()
                && super::ts_return::identifier_continue(ch));
        if code[at] && valid {
            start.get_or_insert(at);
            decoded.push(ch);
        } else if let Some(from) = start.take() {
            result.push((from, at, std::mem::take(&mut decoded)));
        }
        at += ch.len_utf8();
    }
    if let Some(from) = start.take() {
        result.push((from, text.len(), decoded));
    }
    if lang == Language::Python {
        // Python identifier identity is NFKC, unlike Rust's NFC. Keep byte spans
        // in the original text, including compatibility spellings and marks.
        for (_, _, word) in &mut result {
            *word = icu_normalizer::ComposingNormalizerBorrowed::new_nfkc()
                .normalize(word)
                .into_owned();
        }
    }
    result
}

pub(super) fn parameters(first: &str, name: &str, rest: &str, lang: Language) -> Result<()> {
    ensure!(
        ident(name) && !matches!(name, "this" | "self"),
        "unsupported promoted binding: expected a plain parameter name"
    );
    ensure!(
        !first.contains('=') && !first.contains('?'),
        "unsupported promoted binding: default or optional receiver"
    );
    ensure!(
        !tokens(rest, lang).iter().any(|(_, _, w)| w == name
            || matches!(w.as_str(), "self" | "this")
            || (lang == Language::TypeScript && matches!(w.as_str(), "arguments" | "eval"))),
        "unsupported promoted binding in remaining parameter scope"
    );
    if lang == Language::Swift {
        ensure!(
            !first.contains("inout")
                && !first.contains("isolated")
                && !first.contains("consuming")
                && !first.contains("borrowing"),
            "unsupported Swift receiver ownership or isolation"
        );
    }
    Ok(())
}

/// Rebind the original parameter rather than changing its references across lexical scopes.
/// This preserves closures, shadowing, shorthand properties and local reassignment naturally.
pub(super) fn preserve(
    body: &str,
    name: &str,
    first: &str,
    lang: Language,
    swift_returns_void: bool,
) -> Result<(String, usize)> {
    let tokens = tokens(body, lang);
    let forbidden: &[&str] = match lang {
        Language::TypeScript => &["this", "super", "arguments", "eval", "with"],
        Language::Python => &[
            "self",
            "__class__",
            "super",
            "locals",
            "vars",
            "eval",
            "exec",
        ],
        Language::Cpp => &["this"],
        Language::Swift => &["self", "super"],
        Language::Go => &[],
    };
    ensure!(
        !tokens
            .iter()
            .any(|(_, _, w)| forbidden.contains(&w.as_str())),
        "unsupported existing receiver or dynamic binding/capture semantics"
    );
    // A function declaration can replace a parameter's binding during hoisting. An
    // entry initializer would overwrite that function; do not guess its scope here.
    ensure!(
        lang != Language::TypeScript
            || !tokens
                .windows(2)
                .any(|pair| pair[0].2 == "function" && pair[1].2 == name),
        "unsupported promoted binding sharing a function name"
    );
    let uses: Vec<(usize, usize)> = tokens
        .iter()
        .filter(|(_, _, w)| w == name)
        .map(|&(start, end, _)| (start, end))
        .collect();
    let receiver = match lang {
        Language::TypeScript => "this",
        Language::Python | Language::Swift => "self",
        Language::Cpp => "this->",
        Language::Go => unreachable!(),
    };

    // Only an un-nested return expression with member-only reads is substituted. All more
    // involved bodies retain the binding, so no scope or write analysis is guessed here.
    let code = mask(body, lang);
    let simple = body.trim_start().starts_with("return ")
        && !(lang == Language::Python && body.trim().lines().count() > 1)
        && !tokens.iter().any(|(_, _, w)| {
            matches!(w.as_str(), "lambda" | "def" | "func" | "function" | "class")
        })
        && !body.char_indices().any(|(i, c)| {
            code[i]
                && !tokens.iter().any(|&(start, end, _)| start <= i && i < end)
                && matches!(c, '{' | '}' | '[' | ']' | '=')
        })
        && !body.trim().trim_end_matches(';').contains(';')
        && (lang != Language::Cpp || first.contains('&') || first.contains('*'))
        && uses.iter().all(|&(start, end)| {
            let before = body[..start].trim_end();
            let after = &body[end..];
            !before.ends_with('.')
                && !before.ends_with("->")
                && (after.starts_with('.') || (lang == Language::Cpp && after.starts_with("->")))
        })
        && !body.contains("++")
        && !body.contains("--");
    if simple {
        let mut out = body.to_string();
        for &(start, end) in uses.iter().rev() {
            if lang == Language::Cpp {
                let member_len = if body[end..].starts_with("->") { 2 } else { 1 };
                out.replace_range(start..end + member_len, receiver);
            } else {
                out.replace_range(start..end, receiver);
            }
        }
        return Ok((out, uses.len()));
    }
    let binding = match lang {
        Language::TypeScript => {
            let annotation = first
                .split_once(':')
                .map(|(_, ty)| format!(": {}", ty.trim()))
                .unwrap_or_default();
            format!("var {name}{annotation} = this;")
        }
        Language::Python => format!("{name} = self"),
        Language::Swift => {
            let ty = first.split_once(':').map(|(_, ty)| ty.trim()).unwrap_or("");
            ensure!(!ty.is_empty(), "unsupported untyped Swift receiver");
            format!("let {name}: {ty} = self;")
        }
        Language::Cpp => {
            let value = if first.contains('*') { "this" } else { "*this" };
            format!("{first} = {value};")
        }
        Language::Go => unreachable!(),
    };
    let mut tail = body.to_string();
    if lang == Language::Swift && !swift_returns_void && swift_implicit_return(body)? {
        // Adding a declaration turns Swift's single-expression body into a statement body.
        tail = format!(" return {}", body.trim_start());
    }
    Ok((format!(" {binding}{tail}"), uses.len()))
}

fn swift_implicit_return(body: &str) -> Result<bool> {
    let single_line = body.trim().lines().count() == 1;
    let proof = super::swift_regex::proof_body(body)?;
    let body = proof.as_str();
    let code = mask(body, Language::Swift);
    let tokens = words(body, Language::Swift);
    if body.trim().is_empty() {
        return Ok(false);
    }
    let mut delimiters = Vec::new();
    // Each scope records whether returns still belong to the method. Only
    // control blocks inherit that permission; closures and arguments do not.
    let mut control_pending = vec![false];
    let mut words = tokens.iter().peekable();
    let mut statement = false;
    let mut method_return = false;
    let mut separator = false;
    let mut previous = None;
    for (i, byte) in body.bytes().enumerate() {
        if words.peek().is_some_and(|(at, _)| *at == i) {
            let (_, word) = words.next().unwrap();
            let method_scope = delimiters.last().is_none_or(|(_, scope)| *scope);
            if !matches!(previous, Some(b'.' | b'`')) && method_scope {
                method_return |= *word == "return";
                if matches!(
                    *word,
                    "if" | "switch" | "else" | "for" | "while" | "guard" | "do" | "catch"
                ) {
                    *control_pending.last_mut().unwrap() = true;
                }
            }
            // Only method-scope statements prove this is already a statement body.
            // Closures, call arguments, nested functions and member names do not.
            statement |= delimiters.is_empty()
                && !matches!(previous, Some(b'.' | b'`'))
                && matches!(
                    *word,
                    "let"
                        | "var"
                        | "func"
                        | "if"
                        | "switch"
                        | "for"
                        | "while"
                        | "guard"
                        | "throw"
                        | "defer"
                        | "do"
                        | "return"
                );
        }
        if !code[i] {
            continue;
        }
        match byte {
            b'(' | b'[' | b'{' => {
                let scope = byte == b'{'
                    && delimiters.last().is_none_or(|(_, scope)| *scope)
                    && *control_pending.last().unwrap();
                if byte == b'{' {
                    *control_pending.last_mut().unwrap() = false;
                }
                let close = match byte {
                    b'(' => b')',
                    b'[' => b']',
                    _ => b'}',
                };
                delimiters.push((close, scope));
                control_pending.push(false);
            }
            b')' | b']' | b'}' => {
                ensure!(
                    delimiters.pop().map(|(close, _)| close) == Some(byte),
                    "unsupported Swift body delimiter"
                );
                control_pending.pop();
            }
            b';' if delimiters.is_empty() => separator = true,
            _ => {}
        }
        if !byte.is_ascii_whitespace() {
            previous = Some(byte);
        }
    }
    ensure!(delimiters.is_empty(), "unsupported Swift body delimiter");
    ensure!(
        !tokens
            .first()
            .is_some_and(|(_, w)| matches!(*w, "if" | "switch"))
            || method_return,
        "unsupported Swift control expression implicit return binding"
    );
    if statement {
        return Ok(false);
    }
    // Multiline implicit expressions still require a parser-proven boundary.
    // A one-line closure is balanced above; its internal semicolons are irrelevant.
    ensure!(
        single_line,
        "unsupported multiline Swift implicit return binding"
    );
    Ok(!separator)
}

pub(super) fn cpp_parameter(first: &str, owner: &str) -> Result<String> {
    let name = first
        .trim_end()
        .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .next()
        .unwrap_or("");
    ensure!(ident(name), "unsupported C++ promoted binding");
    let ty = first[..first.len() - name.len()].trim();
    let normalized = ty.replace(['&', '*'], " ");
    let parts: Vec<_> = normalized.split_whitespace().collect();
    ensure!(
        parts.iter().filter(|p| **p == owner).count() == 1
            && parts.iter().all(|p| *p == owner || *p == "const")
            && !ty.contains("&&")
            && ty.matches('*').count() <= 1
            && !(ty.contains('&') && ty.contains('*')),
        "unsupported C++ promoted receiver type"
    );
    // A method receiver does not perform parameter construction. Rebinding from
    // *this would force a copy even for prvalues (elision) and xvalues (moves).
    // Without construction/value-category analysis, keep by-value calls intact.
    ensure!(
        ty.contains('&') || ty.contains('*'),
        "unsupported C++ by-value receiver: cannot preserve parameter construction semantics"
    );
    Ok(name.to_string())
}
