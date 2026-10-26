/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::rust::edits::{Setter, apply, compose};
use super::super::{
    case::{is_ident, to_pascal_case, to_snake_case},
    types::Language,
};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

pub(super) type Generated = (String, String, String, usize, usize, usize);

pub(super) fn generate(
    lang: Language,
    text: &str,
    owner: Option<&str>,
    field: &str,
    by_value: Option<bool>,
) -> Result<Generated> {
    match lang {
        Language::TypeScript => super::encapsulate_field_ts(text, owner, field),
        Language::JavaScript => super::encapsulate_field_js(text, owner, field),
        Language::Python => super::encapsulate_field_py(text, owner, field),
        Language::Cpp => super::encapsulate_field_cpp(text, owner, field, by_value),
        Language::Swift => super::encapsulate_field_swift(text, owner, field),
        Language::Go => super::encapsulate_field_go(text, owner, field),
    }
}

fn owner_range(
    text: &str,
    lang: Language,
    owner: &str,
    declaration: usize,
    code: &[bool],
) -> Result<std::ops::Range<usize>> {
    let start = text[..declaration]
        .rfind(&format!("class {owner}"))
        .or_else(|| text[..declaration].rfind(&format!("struct {owner}")))
        .or_else(|| text[..declaration].rfind(&format!("type {owner}")))
        .context("cannot determine declaring type scope")?;
    if lang == Language::Python {
        let mut end = text.len();
        let mut offset = start;
        for (n, line) in text[start..].split_inclusive('\n').enumerate() {
            if n > 0 && !line.trim().is_empty() && !line.starts_with([' ', '\t']) {
                end = offset;
                break;
            }
            offset += line.len();
        }
        return Ok(start..end);
    }
    let open = text[start..]
        .char_indices()
        .find(|(at, ch)| *ch == '{' && code[start + at])
        .map(|(at, _)| start + at)
        .context("declaring type has no body")?;
    let mut depth = 0;
    for (off, ch) in text[open..].char_indices() {
        let at = open + off;
        if code[at] {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(open..at);
                    }
                }
                _ => {}
            }
        }
    }
    anyhow::bail!("declaring type body does not close")
}

/// Protect every occurrence the semantic service did not select, including literal text.
/// Unique markers carry original identity through owner generation, which can insert/delete
/// whole lines. They are never reconciled by counts or reapplied at stale offsets.
#[allow(clippy::too_many_arguments)]
pub(super) fn rewrite(
    text: &str,
    path: &Path,
    lang: Language,
    field: &str,
    positions: &[(u32, u32)],
    declaration: Option<(usize, &str)>,
    by_value: Option<bool>,
) -> Result<(String, usize, usize, usize, Vec<String>)> {
    const PREFIX: &str = "__prod_code_encapsulation_slot_";
    anyhow::ensure!(
        !text.contains(PREFIX),
        "source contains reserved encapsulation marker"
    );
    let mask_lang =
        crate::parameter_object::Language::of(path).context("unsupported lexical language")?;
    let code = crate::caller_migration::lexical_code_mask(text, mask_lang);
    let mut selected = BTreeSet::new();
    let mut unmatched = Vec::new();
    for &(line, col) in positions {
        let at = crate::signature::offset_of(text, line, col);
        match at {
            Some(at)
                if text[at..].starts_with(field)
                    && (at == 0 || !text[..at].ends_with(is_ident))
                    && !text[at + field.len()..].starts_with(is_ident)
                    && code[at..at + field.len()].iter().all(|c| *c) =>
            {
                selected.insert(at);
            }
            _ => unmatched.push(format!(
                "{line}:{col}: analyzer reference does not point at executable `{field}`"
            )),
        }
    }
    let region = declaration
        .map(|(at, owner)| owner_range(text, lang, owner, at, &code))
        .transpose()?;
    let mut shielded = String::new();
    let mut slots = Vec::new();
    let mut from = 0;
    for (at, _) in text.match_indices(field).filter(|(at, _)| {
        (*at == 0 || !text[..*at].ends_with(is_ident))
            && !text[*at + field.len()..].starts_with(is_ident)
    }) {
        shielded.push_str(&text[from..at]);
        let before = text[..at].trim_end();
        let receiver = before
            .strip_suffix("->")
            .or_else(|| before.strip_suffix('.'))
            .unwrap_or("")
            .trim_end()
            .rsplit(|c: char| !is_ident(c))
            .next()
            .unwrap_or("");
        let internal_receiver = receiver == "this" || receiver == "self";
        let bare_cpp = lang == Language::Cpp && !before.ends_with(['.', '>']);
        let internal = selected.contains(&at)
            && region.as_ref().is_some_and(|r| r.contains(&at))
            && (internal_receiver || bare_cpp);
        let type_name = declaration.is_some_and(|(_, owner)| owner == field)
            && ["class", "struct", "type"]
                .iter()
                .any(|word| before.ends_with(word));
        if declaration.is_some_and(|(decl, _)| decl == at) || type_name {
            shielded.push_str(field);
        } else {
            let marker = format!("{PREFIX}{}__", slots.len());
            shielded.push_str(&marker);
            slots.push((marker, selected.contains(&at), at, internal));
        }
        from = at + field.len();
    }
    shielded.push_str(&text[from..]);
    let (mut output, mut reads, mut writes, mut internal) = if let Some((_, owner)) = declaration {
        let (_, _, generated, r, w, i) = generate(lang, &shielded, Some(owner), field, by_value)?;
        (generated, r, w, i)
    } else {
        (shielded, 0, 0, 0)
    };
    let mut edits = Vec::new();
    let mut setters = Vec::new();
    let output_code = crate::caller_migration::lexical_code_mask(&output, mask_lang);
    for (marker, selected, original, tracked_internal) in &slots {
        let at = output
            .find(marker)
            .context("owner generation lost a protected occurrence; nothing was written")?;
        anyhow::ensure!(
            output.matches(marker).count() == 1,
            "owner generation duplicated a protected occurrence"
        );
        if !selected {
            continue;
        }
        if *tracked_internal {
            // Keep the original receiver and evaluation semantics, including callable
            // fields. Every language carries selected internal occurrences through generation.
            let storage = match lang {
                Language::JavaScript => format!("#{field}"),
                Language::Cpp => format!("{field}_"),
                Language::Go => super::super::case::lowercase_first(field),
                _ => format!("_{field}"),
            };
            edits.push((at, marker.len(), storage));
            internal += 1;
            continue;
        }
        let before = text[..*original].trim_end();
        let next = output[at + marker.len()..]
            .char_indices()
            .find(|(off, ch)| output_code[at + marker.len() + off] && !ch.is_whitespace())
            .map_or(output.len(), |(off, _)| at + marker.len() + off);
        let after = &output[next..];
        let refusal =
            if !before.ends_with('.') && !(lang == Language::Cpp && before.ends_with("->")) {
                Some("not a supported member access")
            } else if callable_access(text, *original, field.len()) {
                Some("callable field requires preserving receiver/evaluation semantics")
            } else if [
                "+=", "-=", "*=", "/=", "%=", "++", "--", "&=", "|=", "^=", "<<=", ">>=", "**=",
                "//=", "&&=", "||=", "??=", ">>>=",
            ]
            .iter()
            .any(|op| after.starts_with(op))
            {
                Some("compound field update cannot become an accessor call")
            } else {
                None
            };
        if let Some(reason) = refusal {
            unmatched.push(format!("byte {original}: {reason}"));
            continue;
        }
        let pascal = to_pascal_case(field);
        let snake = to_snake_case(field);
        let (getter, setter) = match lang {
            Language::Python | Language::Cpp => (format!("get_{snake}"), format!("set_{snake}")),
            Language::Go => (pascal.clone(), format!("Set{pascal}")),
            _ => (format!("get{pascal}"), format!("set{pascal}")),
        };
        if after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>") {
            let start = output.len() - after.len() + 1;
            let end = super::expressions::rhs_end(&output, start, mask_lang)?;
            if !super::expressions::standalone_assignment(text, *original, mask_lang) {
                unmatched.push(format!("byte {original}: assignment value may be consumed"));
                continue;
            }
            // A Python expression-list is one assigned value. Group the whole RHS,
            // including lambda parameters/body, so tuple commas cannot become call
            // argument separators. These child edits compose with selected reads.
            if lang == Language::Python && output[start..end].contains(',') {
                edits.push((start, 0, "(".into()));
                edits.push((end, 0, ")".into()));
            }
            let trivia = output[at + marker.len()..start - 1].trim_matches([' ', '\t', '\r']);
            setters.push(Setter::new(
                &output,
                at,
                start,
                end,
                format!("{setter}{trivia}"),
                mask_lang,
            ));
            writes += 1;
        } else {
            edits.push((at, marker.len(), format!("{getter}()")));
            reads += 1;
        }
    }
    compose(&output, &mut edits, setters)?;
    output = apply(&output, edits)?;
    for (marker, _, _, _) in slots {
        output = output.replace(&marker, field);
    }
    Ok((output, reads, writes, internal, unmatched))
}

fn skip_comments(mut text: &str) -> &str {
    loop {
        text = text.trim_start();
        if text.starts_with("/*") {
            let Some(end) = text.find("*/") else {
                return text;
            };
            text = &text[end + 2..];
        } else if text.starts_with("//") {
            let Some(end) = text.find('\n') else {
                return "";
            };
            text = &text[end..];
        } else {
            return text;
        }
    }
}

fn callable_access(text: &str, at: usize, len: usize) -> bool {
    let suffix = skip_comments(&text[at + len..]);
    let called = |s: &str| s.starts_with('(') || s.starts_with("?.(") || s.starts_with('`');
    if called(suffix) {
        return true;
    }
    let before = text[..at].trim_end();
    if !before.ends_with('.') {
        return false;
    }
    let start = super::super::rust::chain_start(text, before.len() - 1);
    if !text[..start].trim_end().ends_with('(') {
        return false;
    }
    let mut rest = suffix;
    while let Some(after) = rest.strip_prefix(')') {
        rest = skip_comments(after);
    }
    called(rest)
}
