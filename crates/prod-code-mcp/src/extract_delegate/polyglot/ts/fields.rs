/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::extract_delegate::common::{is_ident, is_ident_str};
use crate::parameter_object::{
    Language,
    syntax::{close_in, walk_code},
};
use anyhow::{Result, ensure};
use std::collections::BTreeMap;

pub(super) type Declarations = BTreeMap<String, (String, Option<String>)>;

pub(super) fn declarations(body: &str, lang: Language) -> Declarations {
    let mut declarations = BTreeMap::new();
    let mut depth = 0i32;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }
        if depth == 0
            && let Some(decl) = trimmed.strip_suffix(';')
        {
            let head = decl.split('=').next().unwrap_or(decl).trim();
            let (lhs, ty) = head
                .split_once(':')
                .map_or((head, None), |(lhs, ty)| (lhs, Some(ty.trim().to_string())));
            let name = lhs.split_whitespace().last().unwrap_or("");
            if is_ident_str(name) && name != "return" && name != "constructor" {
                declarations.insert(name.to_string(), (line.to_string(), ty));
            }
        }
        walk_code(line, 0, lang, |_, c| {
            if c == b'{' {
                depth += 1;
            }
            if c == b'}' {
                depth -= 1;
            }
            true
        });
    }
    declarations
}

pub(super) fn constructor_range(body: &str, lang: Language) -> Option<std::ops::Range<usize>> {
    let ctor = body
        .find("constructor(")
        .or_else(|| body.find("constructor ("))?;
    let open = body[ctor..].find('{')? + ctor;
    Some(open + 1..close_in(body, open, lang)?)
}

pub(super) fn constructor_assignments(body: &str, lang: Language) -> BTreeMap<String, String> {
    let mut assignments = BTreeMap::new();
    let Some(range) = constructor_range(body, lang) else {
        return assignments;
    };
    for line in body[range].lines() {
        if let Some(assignment) = line.trim().strip_suffix(';')
            && let Some((lhs, rhs)) = assignment.split_once('=')
            && let Some(name) = lhs.trim().strip_prefix("this.")
            && is_ident_str(name.trim())
        {
            assignments.insert(name.trim().to_string(), rhs.trim().to_string());
        }
    }
    assignments
}

pub(super) fn can_defer(body: &str, name: &str) -> bool {
    let Some(ctor) = body
        .find("constructor(")
        .or_else(|| body.find("constructor ("))
    else {
        return false;
    };
    let Some(open) = body[ctor..].find('{').map(|p| p + ctor) else {
        return false;
    };
    body[open + 1..]
        .trim_start()
        .starts_with(&format!("this.{name} ="))
        && body.matches(&format!("this.{name} =")).count() == 1
}

/// Deferral replaces the whole assignment, so its RHS must end at the statement
/// boundary and cannot depend on an owner whose helper has not been constructed.
pub(super) fn deferred_rhs(line: &str, lang: Language) -> Result<&str> {
    let rhs = line.split_once('=').unwrap().1.trim();
    let mut depth = 0i32;
    let mut end = None;
    walk_code(rhs, 0, lang, |i, c| {
        if depth == 0 && c == b';' {
            end = Some(i);
            return false;
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
        true
    });
    let end = end.filter(|end| super::super::lexical::trivia(&rhs[end + 1..], lang).is_empty());
    ensure!(
        end.is_some(),
        "deferred initializer requires one complete assignment statement"
    );
    let rhs = rhs[..end.unwrap()].trim();
    ensure!(
        !owner_dependent(rhs, lang, true),
        "deferred initializer has unproved owner-dependent evaluation"
    );
    Ok(rhs)
}

/// Literal owner text is harmless; executable interpolation and base evaluation
/// can observe a moved field indirectly through a retained/overridden method.
fn owner_dependent(text: &str, _lang: Language, expression: bool) -> bool {
    super::super::js_code::owner_dependent(text, expression)
}

/// A readonly constructor assignment must run in the helper's constructor, at
/// the original statement. Do not hoist its RHS across earlier owner effects.
pub(super) fn readonly_deferred(
    body: &str,
    fields: &[String],
    decls: &Declarations,
    assignments: &BTreeMap<String, String>,
) -> Result<bool> {
    let readonly: Vec<_> = fields
        .iter()
        .filter(|f| {
            decls.get(*f).is_some_and(|(d, _)| {
                d.split_once('=')
                    .map_or(d.as_str(), |(head, _)| head)
                    .split_whitespace()
                    .any(|word| word == "readonly")
            }) && assignments.contains_key(*f)
        })
        .collect();
    if readonly.is_empty() {
        return Ok(false);
    }
    let name = readonly[0];
    ensure!(
        fields.len() == 1 && !decls[name].0.contains('='),
        "readonly constructor initialization requires one field without a declaration initializer"
    );
    for (decl, _) in decls.values() {
        if let Some((_, rhs)) = decl.split_once('=') {
            ensure!(
                !owner_dependent(rhs, Language::TypeScript, true),
                "readonly initialization has unproved owner field-initializer effects"
            );
        }
    }
    let range = constructor_range(body, Language::TypeScript)
        .ok_or_else(|| anyhow::anyhow!("readonly constructor not found"))?;
    let needle = format!("this.{name}");
    let assignment = body[range.clone()]
        .lines()
        .find(|line| {
            line.trim().starts_with(&format!("{needle} ="))
                || line.trim().starts_with(&format!("{needle}="))
        })
        .ok_or_else(|| anyhow::anyhow!("readonly assignment is not a standalone statement"))?;
    let position = assignment.as_ptr() as usize - body.as_ptr() as usize;
    let prefix = &body[range.start..position];
    let mut depth = 0i32;
    let prior_owner_use = owner_dependent(prefix, Language::TypeScript, false);
    let mut early_return = false;
    let mut last_code = None;
    walk_code(prefix, 0, Language::TypeScript, |i, c| {
        match c {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if prefix
            .get(i..)
            .is_some_and(|rest| rest.starts_with("return"))
            && !prefix[..i].chars().next_back().is_some_and(is_ident)
            && !prefix[i + 6..].chars().next().is_some_and(is_ident)
        {
            early_return = true;
        }
        if !c.is_ascii_whitespace() {
            last_code = Some(c);
        }
        true
    });
    ensure!(
        depth == 0
            && !prior_owner_use
            && !early_return
            && matches!(last_code, None | Some(b';' | b'}')),
        "readonly initialization has unproved earlier owner/control-flow effects"
    );
    let assignments = body
        .lines()
        .filter(|line| {
            line.trim().starts_with(&format!("{needle} ="))
                || line.trim().starts_with(&format!("{needle}="))
        })
        .count();
    ensure!(
        assignments == 1,
        "readonly field has multiple constructor assignments"
    );
    Ok(true)
}

pub(super) fn initializer_args(
    body: &str,
    fields: &[String],
    decls: &Declarations,
    is_js: bool,
) -> Result<String> {
    // Combining separated declaration initializers would move evaluation across other
    // fields. Refuse that shape rather than silently reorder effects.
    let ranges: Vec<_> = fields
        .iter()
        .filter_map(|f| decls.get(f))
        .map(|(d, _)| {
            let start = body.find(d).unwrap();
            start..start + d.len()
        })
        .collect();
    for pair in ranges.windows(2) {
        let between = &body[pair[0].end..pair[1].start];
        ensure!(
            between
                .lines()
                .all(|l| l.trim().is_empty() || l.trim_start().starts_with("//")),
            "interleaved field declarations have unproved initializer evaluation order"
        );
    }
    Ok(fields
        .iter()
        .map(|f| {
            decls
                .get(f)
                .and_then(|(decl, _)| decl.split_once('='))
                .map(|(_, rhs)| rhs.trim().trim_end_matches(';').trim().to_string())
                .unwrap_or_else(|| {
                    if is_js {
                        "undefined".into()
                    } else {
                        "undefined as any".into()
                    }
                })
        })
        .collect::<Vec<_>>()
        .join(", "))
}
