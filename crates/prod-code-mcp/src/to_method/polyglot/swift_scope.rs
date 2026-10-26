/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::ops::Range;

#[path = "swift_names.rs"]
mod names;

/// Primary declarations, extensions and inherited members share nominal scope.
/// Qualified paths exclude unrelated types with the same leaf name. An unknown
/// base cannot prove that a result name denotes the standard library's unit.
pub(super) fn visible_extensions(
    source: &str,
    method: usize,
    binding_name: &str,
    syntax: &[bool],
    tokens: &[(usize, &str)],
    scopes: &[Range<usize>],
    scope_at: &[usize],
) -> (Vec<bool>, bool) {
    let mut owners = vec![None::<String>; scopes.len()];
    let mut bases = vec![Vec::new(); scopes.len()];
    let mut extensions = vec![false; scopes.len()];
    for (index, pair) in tokens.windows(2).enumerate() {
        let [(_, keyword), (at, name)] = pair else {
            unreachable!()
        };
        let extension = *keyword == "extension";
        if !extension && !matches!(*keyword, "class" | "struct" | "enum" | "protocol" | "actor") {
            continue;
        }
        // `class func` and protocol class constraints do not declare a type.
        if !name.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
            || matches!(*name, "func" | "var" | "subscript")
        {
            continue;
        }
        let Some(body) = declaration_body(source, syntax, at + name.len()) else {
            continue;
        };
        let Some(scope) = scopes.iter().position(|range| range.start == body + 1) else {
            continue;
        };
        let parent = &owners[scope_at[*at]];
        let name = if extension {
            qualified_name(source, syntax, &tokens[index + 1..])
        } else {
            name.to_string()
        };
        owners[scope] = Some(match parent {
            Some(parent) => format!("{parent}.{name}"),
            None => name,
        });
        bases[scope] = inherited_names(source, syntax, at + pair[1].1.len(), body);
        extensions[scope] = extension;
    }
    let aliases = aliases(source, syntax, tokens, &owners, scope_at);
    let raw_owners = owners.clone();
    let names = names::Names::new(&raw_owners, &extensions, &aliases);
    let mut unresolved = false;
    for (scope, owner) in owners.iter_mut().enumerate() {
        if let Some(owner) = owner {
            if let Some(resolved) = names.absolute(owner) {
                *owner = resolved;
            } else {
                // An unknown target might be an alias in another file. Only
                // extensions contributing this binding (or inherited bindings)
                // can affect its lookup; empty and unrelated-member extensions
                // provide no reason to make the selected nominal uncertain.
                unresolved |= extensions[scope]
                    && (!bases[scope].is_empty()
                        || tokens.windows(2).any(|pair| {
                            let [(_, keyword), (at, word)] = pair else {
                                unreachable!()
                            };
                            *word == binding_name
                                && scope_at[*at] == scope
                                && matches!(
                                    *keyword,
                                    "class"
                                        | "struct"
                                        | "enum"
                                        | "protocol"
                                        | "actor"
                                        | "typealias"
                                        | "associatedtype"
                                )
                        }));
            }
        }
    }
    let mut visible: Vec<_> = scopes
        .iter()
        .zip(&owners)
        .filter(|(range, _)| range.contains(&method))
        .filter_map(|(_, owner)| owner.clone())
        .collect();
    let mut index = 0;
    while index < visible.len() {
        let owner = visible[index].clone();
        for (scope, _) in owners
            .iter()
            .enumerate()
            .filter(|(_, name)| name.as_deref() == Some(owner.as_str()))
        {
            for base in &bases[scope] {
                if let Some(base) = names.relative(base, owner.rsplit_once('.').map(|(p, _)| p)) {
                    // An extension provides known members, not a complete account
                    // of an external type's inherited bindings.
                    unresolved |= !owners
                        .iter()
                        .enumerate()
                        .any(|(scope, name)| !extensions[scope] && name.as_ref() == Some(&base));
                    if !visible.contains(&base) {
                        visible.push(base);
                    }
                } else {
                    unresolved = true;
                }
            }
        }
        index += 1;
    }
    let applicable = owners
        .iter()
        .map(|owner| owner.as_ref().is_some_and(|owner| visible.contains(owner)))
        .collect();
    (applicable, unresolved)
}

fn aliases(
    source: &str,
    syntax: &[bool],
    tokens: &[(usize, &str)],
    owners: &[Option<String>],
    scope_at: &[usize],
) -> Vec<(String, Option<String>)> {
    let mut aliases = Vec::new();
    for pair in tokens.windows(3) {
        let [(_, keyword), (at, name), (target_at, _)] = pair else {
            unreachable!()
        };
        let scope = scope_at[*at];
        if *keyword != "typealias" || (scope != 0 && owners[scope].is_none()) {
            continue;
        }
        let owner = owners[scope].as_ref();
        let qualified = owner.map_or_else(|| name.to_string(), |owner| format!("{owner}.{name}"));
        let after_name = at + name.len();
        let equals = source[after_name..*target_at]
            .char_indices()
            .find(|(offset, ch)| syntax[after_name + offset] && *ch == '=')
            .map(|(offset, _)| after_name + offset);
        let Some(equals) = equals else {
            aliases.push((qualified, None));
            continue;
        };
        let simple_lhs = source[after_name..equals]
            .char_indices()
            .all(|(offset, ch)| !syntax[after_name + offset] || ch.is_whitespace());
        let target_at = source
            .bytes()
            .enumerate()
            .skip(equals + 1)
            .find(|(at, ch)| syntax[*at] && !ch.is_ascii_whitespace())
            .map_or(source.len(), |(at, _)| at);
        let end = source[target_at..]
            .char_indices()
            .find(|(offset, ch)| syntax[target_at + offset] && matches!(ch, '\n' | ';' | '}'))
            .map_or(source.len(), |(offset, _)| target_at + offset);
        let target: String = source[target_at..end]
            .char_indices()
            .filter(|(offset, ch)| syntax[target_at + offset] && !ch.is_whitespace())
            .map(|(_, ch)| ch)
            .collect();
        let target = super::ungroup(&target);
        let simple = simple_lhs
            && target.split('.').all(|part| {
                !part.is_empty() && part.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
            });
        aliases.push((qualified, simple.then(|| target.to_string())));
    }
    aliases
}

/// Keep only inheritance-list names. Generic constraints, generic arguments and
/// where clauses do not introduce superclass/member lookup scopes.
fn inherited_names(source: &str, syntax: &[bool], start: usize, end: usize) -> Vec<String> {
    let words =
        super::super::binding::words(&source[start..end], crate::make_static::Language::Swift);
    let end = words
        .iter()
        .find(|(_, word)| *word == "where")
        .map_or(end, |(at, _)| start + at);
    let mut depth = 0;
    let mut inherits = false;
    let mut names = Vec::new();
    let mut name = String::new();
    for (at, ch) in source[start..end].char_indices() {
        if !syntax[start + at] {
            continue;
        }
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ':' if depth == 0 => inherits = true,
            ',' | '&' if depth == 0 && inherits => {
                if !name.is_empty() {
                    names.push(std::mem::take(&mut name));
                }
            }
            _ if depth == 0 && inherits && !ch.is_whitespace() => name.push(ch),
            _ => {}
        }
    }
    if !name.is_empty() {
        names.push(name);
    }
    names
}

fn qualified_name(source: &str, syntax: &[bool], tokens: &[(usize, &str)]) -> String {
    let mut name = tokens[0].1.to_string();
    let mut end = tokens[0].0 + tokens[0].1.len();
    for &(at, word) in &tokens[1..] {
        let separator: String = source[end..at]
            .char_indices()
            .filter(|(offset, ch)| syntax[end + offset] && !ch.is_whitespace())
            .map(|(_, ch)| ch)
            .collect();
        if separator != "." {
            break;
        }
        name.push('.');
        name.push_str(word);
        end = at + word.len();
    }
    name
}

fn declaration_body(source: &str, syntax: &[bool], after_name: usize) -> Option<usize> {
    // Nominal/extension headers contain types, never closure expressions. Their
    // first code brace starts the body, including headers with function types.
    for (at, byte) in source.bytes().enumerate().skip(after_name) {
        if !syntax[at] {
            continue;
        }
        match byte {
            b'{' => return Some(at),
            b'}' | b';' => return None,
            _ => {}
        }
    }
    None
}
