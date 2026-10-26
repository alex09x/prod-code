/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    cpp_boundary::code,
    cpp_signature::{conditional, include_guard, tokens},
};
use std::collections::{HashMap, HashSet};

/// Narrow fallback facts, not a type resolver. Precise declaration references
/// remain authoritative. Unknown aliases/expressions never prove inequivalence.
#[derive(Default)]
pub(super) struct Types {
    aliases: HashMap<String, Option<Vec<String>>>,
    nominal: HashSet<String>,
    macros: HashSet<String>,
    guards: HashSet<String>,
}

impl Types {
    /// Preprocessor names have translation-unit scope even in namespace includes.
    /// Without expansion/order information, neither define nor undef can restore
    /// a spelling as an authoritative alias or nominal identity fact.
    pub(super) fn observe_macros(&mut self, text: &str, skip_guard: bool) {
        let mask = crate::caller_migration::mask::lexical_code_mask(
            text,
            crate::parameter_object::Language::Cpp,
        );
        let mut guard = skip_guard
            .then(|| include_guard(text))
            .flatten()
            .filter(|name| self.guards.insert(name.clone()));
        let mut offset = 0;
        for line in text.split_inclusive('\n') {
            let trimmed = line.trim_start();
            let hash = offset + line.len() - trimmed.len();
            offset += line.len();
            if !mask.get(hash).copied().unwrap_or(false) {
                continue;
            }
            let Some(directive) = trimmed.strip_prefix('#') else {
                continue;
            };
            let words = tokens(directive);
            if matches!(words.first().map(String::as_str), Some("define" | "undef"))
                && let Some(name) = words.get(1)
            {
                if words.first().map(String::as_str) == Some("define")
                    && guard.as_deref() == Some(name.as_str())
                {
                    guard = None;
                    continue;
                }
                self.macros.insert(name.clone());
            }
        }
    }

    pub(super) fn guard_suppressed(&self, text: &str) -> bool {
        include_guard(text).is_some_and(|guard| self.macros.contains(&guard))
    }

    /// Only unconditional global declarations are admitted. Namespace/local,
    /// dependent, chained and declarator aliases require semantic identity.
    pub(super) fn observe(&mut self, text: &str) {
        let clean = code(text);
        let global = |at| {
            let depth = clean[..at].bytes().fold(0isize, |depth, b| match b {
                b'{' => depth + 1,
                b'}' => depth - 1,
                _ => depth,
            });
            depth == 0 && !conditional(text, at)
        };
        let alias =
            regex::Regex::new(r"\busing\s+([A-Za-z_][A-Za-z_0-9]*)\s*=\s*([^;]+);").unwrap();
        let typedef =
            regex::Regex::new(r"\btypedef\s+([^;{}]+)\s+([A-Za-z_][A-Za-z_0-9]*)\s*;").unwrap();
        let declarations = alias
            .captures_iter(&clean)
            .map(|c| {
                (
                    c.get(0).unwrap().start(),
                    c[1].to_string(),
                    c[2].to_string(),
                )
            })
            .chain(typedef.captures_iter(&clean).map(|c| {
                (
                    c.get(0).unwrap().start(),
                    c[2].to_string(),
                    c[1].to_string(),
                )
            }));
        for (at, name, spelling) in declarations {
            if !global(at) {
                continue;
            }
            let value = builtin(&tokens(&spelling));
            self.aliases
                .entry(name)
                .and_modify(|prior| {
                    if *prior != value {
                        *prior = None;
                    }
                })
                .or_insert(value);
        }
        let imported = regex::Regex::new(
            r"\busing\s+(?:typename\s+)?(?:[A-Za-z_][A-Za-z_0-9]*\s*::\s*)+([A-Za-z_][A-Za-z_0-9]*)\s*;",
        )
        .unwrap();
        for found in imported.captures_iter(&clean) {
            self.aliases.insert(found[1].to_string(), None);
        }
        let nominal =
            regex::Regex::new(r"\b(?:struct|class|union)\s+([A-Za-z_][A-Za-z_0-9]*)\s*[{;]")
                .unwrap();
        for found in nominal.captures_iter(&clean) {
            if global(found.get(0).unwrap().start()) {
                self.nominal.insert(found[1].to_string());
            }
        }
    }

    fn nominal<'a>(&self, ty: &'a [String]) -> Option<&'a str> {
        let words: Vec<_> = ty
            .iter()
            .map(String::as_str)
            .filter(|w| !matches!(*w, "const" | "volatile"))
            .collect();
        match words.as_slice() {
            [name] | ["struct" | "class" | "union", name]
                if self.nominal.contains(*name) && !self.aliases.contains_key(*name) =>
            {
                Some(name)
            }
            _ => None,
        }
    }

    fn builtin(&self, ty: &[String]) -> Option<Vec<String>> {
        if ty.iter().any(|word| self.macros.contains(word)) {
            return None;
        }
        // A single alias to a proven builtin is sufficient for ordinary header
        // spelling differences. Do not substitute names inside arbitrary syntax.
        if let [name] = ty
            && let Some(value) = self.aliases.get(name)
        {
            return value.clone();
        }
        builtin(ty)
    }

    /// Some(true): equal; Some(false): demonstrably distinct; None: unresolved.
    pub(super) fn same(&self, left: &[Vec<String>], right: &[Vec<String>]) -> Option<bool> {
        // A sole void type denotes an empty parameter list, including when its
        // spelling is a proven alias. Normalize before comparing lexical arity.
        let empty_if_void = |parameters: &[Vec<String>]| {
            parameters.len() == 1
                && self
                    .builtin(&parameters[0])
                    .is_some_and(|ty| ty == ["void"])
        };
        let left = if empty_if_void(left) { &[][..] } else { left };
        let right = if empty_if_void(right) { &[][..] } else { right };
        if left.len() != right.len() {
            // An unresolved token may expand to zero or several parameters.
            // Only proven value types establish the actual parameter count;
            // semantic declaration membership resolves everything else.
            return left
                .iter()
                .chain(right)
                .all(|ty| {
                    self.builtin(ty).is_some()
                        || (!ty.iter().any(|w| self.macros.contains(w))
                            && self.nominal(ty).is_some())
                })
                .then_some(false);
        }
        let mut unknown = false;
        for (left, right) in left.iter().zip(right) {
            if left
                .iter()
                .chain(right)
                .any(|word| self.macros.contains(word))
            {
                unknown = true;
                continue;
            }
            if left == right {
                continue;
            }
            // Distinct proven global class/struct/union names denote distinct value
            // types, whether written as bare tags or elaborated type specifiers.
            // Aliases and macro names cannot establish this identity.
            if let (Some(a), Some(b)) = (self.nominal(left), self.nominal(right)) {
                if a != b {
                    return Some(false);
                }
                continue;
            }
            if let (Some(a), Some(b)) = (self.builtin(left), self.builtin(right)) {
                if a != b {
                    return Some(false);
                }
                continue;
            }
            // A known class template with a single sizeof(string) argument has
            // distinct specializations for distinct string lengths. Keep literal
            // bytes and token boundaries; aliases/templates we cannot prove stay
            // unknown rather than silently skipping a selected prototype.
            if let (Some((a, n)), Some((b, m))) = (string_extent(left), string_extent(right))
                && a == b
                && n != m
                && self.nominal.contains(a)
                && !self.aliases.contains_key(a)
            {
                return Some(false);
            }
            unknown = true;
        }
        if unknown { None } else { Some(true) }
    }
}

/// Canonicalize only builtin value types. Top-level cv qualifiers are not part
/// of a function parameter's type; pointer/reference/dependent syntax is unknown.
fn builtin(ty: &[String]) -> Option<Vec<String>> {
    let mut words = ty
        .iter()
        .map(String::as_str)
        .filter(|w| !matches!(*w, "const" | "volatile"))
        .collect::<Vec<_>>();
    if words.is_empty()
        || words.iter().any(|w| {
            !matches!(
                *w,
                "void"
                    | "bool"
                    | "char"
                    | "short"
                    | "int"
                    | "long"
                    | "float"
                    | "double"
                    | "signed"
                    | "unsigned"
                    | "wchar_t"
                    | "char8_t"
                    | "char16_t"
                    | "char32_t"
                    | "__int128"
                    | "_Complex"
                    | "__complex__"
            )
        })
    {
        return None;
    }
    for word in &mut words {
        if *word == "__complex__" {
            *word = "_Complex";
        }
    }
    if words
        .iter()
        .any(|w| matches!(*w, "short" | "long" | "signed" | "unsigned"))
        && !words
            .iter()
            .any(|w| matches!(*w, "char" | "double") || w.starts_with("__int"))
        && !words.contains(&"int")
    {
        words.push("int");
    }
    if !words.contains(&"char") {
        words.retain(|w| *w != "signed");
    }
    words.sort_unstable();
    Some(words.into_iter().map(str::to_string).collect())
}

fn string_extent(ty: &[String]) -> Option<(&str, usize)> {
    let [name, open, size, paren, literal, close_paren, close] = ty else {
        return None;
    };
    if open != "<" || size != "sizeof" || paren != "(" || close_paren != ")" || close != ">" {
        return None;
    }
    let content = literal.strip_prefix('"')?.strip_suffix('"')?;
    if !content.is_ascii() || content.contains('\\') {
        return None;
    }
    Some((name, content.len() + 1))
}
