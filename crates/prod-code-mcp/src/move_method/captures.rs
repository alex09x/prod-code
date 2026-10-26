/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{is_ident, opaque_token};

/// Macro literals may resolve implicit captures without an identifier token in the
/// body. Until expansion/binding identity can be proved, neither leaving the capture
/// nor editing its literal is safe. This refusal is independent of diagnostics/force.
pub(super) fn ensure_capture_identity(text: &str, bindings: &[&str]) -> anyhow::Result<()> {
    let mut context = super::context::MacroContext::default();
    let mut i = 0;
    while i < text.len() {
        if let Some((end, comment)) = opaque_token(text, i)
            .map_err(|()| anyhow::anyhow!("incomplete token; cannot establish capture identity"))?
        {
            if !comment && context.in_macro() {
                let literal = &text[i..end];
                // Ordinary byte/C prefixes are scanned separately from the quote.
                let prefixed = text[..i].ends_with(['b', 'c']);
                if !prefixed && let Some(value) = string_value(literal) {
                    for binding in bindings {
                        anyhow::ensure!(
                            !captures(&value, binding),
                            "cannot establish binding identity for implicit format capture `{binding}` in macro literal; nothing was planned"
                        );
                    }
                }
            }
            if !comment {
                context.literal();
            }
            i = end;
            continue;
        }
        let c = text[i..].chars().next().unwrap();
        if let Some(word) = super::context::word_at(text, i) {
            i = word.end;
            context.word(word.name, word.raw);
            if let Some(end) = word.owner_end(text) {
                i = end;
            }
            continue;
        }
        context.punctuation(c);
        i += c.len_utf8();
    }
    Ok(())
}

fn captures(value: &str, binding: &str) -> bool {
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '{' {
            continue;
        }
        if chars.peek() == Some(&'{') {
            chars.next();
            continue;
        }
        let field: String = chars.by_ref().take_while(|c| *c != '}').collect();
        if field.split(|c| !is_ident(c)).any(|word| word == binding) {
            return true;
        }
    }
    false
}

/// Decode only str literals, without changing their source bytes. Rust escapes can
/// encode either the opening brace or the binding name, so raw-text checks are unsound.
fn string_value(literal: &str) -> Option<String> {
    if literal.starts_with('r') {
        let start = literal.find('"')? + 1;
        let end = literal.rfind('"')?;
        return Some(literal[start..end].to_string());
    }
    let content = literal.strip_prefix('"')?.strip_suffix('"')?;
    let mut chars = content.chars().peekable();
    let mut value = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            value.push(c);
            continue;
        }
        value.push(match chars.next()? {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            c @ ('\\' | '\'' | '"') => c,
            'x' => {
                let high = chars.next()?.to_digit(16)?;
                let low = chars.next()?.to_digit(16)?;
                char::from_u32(high * 16 + low)?
            }
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }
                let digits: String = chars
                    .by_ref()
                    .take_while(|c| *c != '}')
                    .filter(|c| *c != '_')
                    .collect();
                char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?
            }
            '\n' | '\r' => {
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                continue;
            }
            _ => return None,
        });
    }
    Some(value)
}
