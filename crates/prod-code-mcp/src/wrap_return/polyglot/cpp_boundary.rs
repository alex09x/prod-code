/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::cpp_includes::includes;
use super::cpp_search::Search;
use super::cpp_signature::{conditional, parameters, token_spans, tokens};
use super::cpp_types::Types;
use crate::wrap_return::{
    types::PolyglotFuncDecl,
    utils::{cpp_return_type_span, is_ident},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub(super) enum Identity {
    Selected,
    Unrelated,
    Unknown,
}

pub(super) struct Boundary {
    root: PathBuf,
    search: Search,
    file: PathBuf,
    name: String,
    scope: Option<Vec<String>>,
    parameters: Option<Vec<Vec<String>>>,
    result: Vec<String>,
    headers: HashSet<PathBuf>,
    reachable: HashSet<PathBuf>,
    unproven_context: HashSet<PathBuf>,
    types: Types,
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn code(text: &str) -> String {
    let mut bytes: Vec<_> = text
        .bytes()
        .map(|b| if b == b'\n' { b } else { b' ' })
        .collect();
    for (start, end) in token_spans(text) {
        let line = text[..start].rfind('\n').map_or(0, |at| at + 1);
        if !super::cpp_attributes::literal(&text[start..end])
            && !text[line..].trim_start().starts_with('#')
        {
            bytes[start..end].copy_from_slice(&text.as_bytes()[start..end]);
        }
    }
    String::from_utf8(bytes).expect("mask preserves UTF-8 code")
}

fn scope(text: &str, at: usize) -> Option<Vec<String>> {
    if conditional(text, at) {
        return None;
    }
    let code = super::cpp_attributes::declaration_code(text);
    let mut stack: Vec<(Option<String>, bool)> = Vec::new();
    let mut start = 0;
    for (i, c) in code[..at].char_indices() {
        match c {
            '{' => {
                let prefix = code[start..i].trim();
                let words: Vec<_> = prefix.split_whitespace().collect();
                if let Some(rest) = prefix.strip_prefix("namespace ") {
                    if rest.is_empty() || !rest.chars().all(|c| is_ident(c) || c == ':') {
                        return None;
                    }
                    stack.push((Some(rest.to_string()), false));
                } else if words
                    .iter()
                    .any(|w| matches!(*w, "class" | "struct" | "union" | "extern"))
                {
                    stack.push((None, true));
                } else {
                    stack.push((None, false));
                }
                start = i + 1;
            }
            '}' => {
                stack.pop()?;
                start = i + 1;
            }
            ';' => start = i + 1,
            _ => {}
        }
    }
    // A preceding, closed class does not change the scope of a later prototype.
    // A declaration still inside such a scope requires semantic ownership.
    if stack.iter().any(|(_, opaque)| *opaque) {
        return None;
    }
    let mut names: Vec<_> = stack
        .into_iter()
        .filter_map(|(name, _)| name)
        .flat_map(|s| s.split("::").map(str::to_string).collect::<Vec<_>>())
        .collect();
    let before = code[..at].trim_end();
    if before.ends_with("::") {
        let qualifier = before
            .trim_end_matches(':')
            .rsplit(|c: char| !is_ident(c) && c != ':')
            .next()?;
        if qualifier.is_empty() {
            return None;
        }
        names.extend(qualifier.split("::").map(str::to_string));
    }
    Some(names)
}

pub(super) fn prototype_span(text: &str, at: usize, name: &str) -> Option<(usize, usize, String)> {
    let code = super::cpp_attributes::declaration_code(text);
    let (close, _) = parameters(&code, at, name)?;
    if !code[close + 1..].trim_start().starts_with(';') {
        return None;
    }
    let start = code[..at].rfind([';', '{', '}']).map_or(0, |i| i + 1);
    let (result, span) = cpp_return_type_span(text, start, at);
    let (start, end) = span?;
    // Expressions such as `return calculate();` are calls, never prototypes.
    if tokens(&result)
        .iter()
        .any(|w| matches!(w.as_str(), "return" | "co_return" | "throw" | "="))
    {
        return None;
    }
    Some((start, end, result))
}

impl Boundary {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn visit(
        &self,
        path: &Path,
        text: &str,
        at: usize,
        now: &str,
        has_refs: bool,
        refs: &mut HashSet<(PathBuf, u32, u32)>,
        key: &(PathBuf, u32, u32),
        site: &str,
        edits: &mut Vec<(usize, usize, String)>,
        unmatched: &mut Vec<String>,
    ) -> bool {
        let prototype = prototype_span(text, at, &self.name);
        let member = refs.contains(key);
        if prototype.is_none() && has_refs && !member {
            return false;
        }
        match self.identity(path, text, at, member) {
            Identity::Unrelated => return false,
            Identity::Unknown => {
                unmatched.push(format!(
                    "{site} C/C++ declaration ownership cannot be established"
                ));
                return false;
            }
            Identity::Selected => {}
        }
        if let Some((start, end, _)) = prototype {
            if has_refs && !member {
                unmatched.push(format!(
                    "{site} owned prototype is missing from semantic references"
                ));
            } else {
                refs.remove(key);
                replace_type(text, start, end, now, edits);
            }
            return false;
        }
        true
    }

    pub(super) fn new(root: &Path, file: &Path, text: &str, decl: &PolyglotFuncDecl) -> Self {
        let mut boundary = Self {
            root: root.to_path_buf(),
            search: Search::new(root, file),
            file: canonical(file),
            name: decl.name.clone(),
            scope: scope(text, decl.name_start),
            parameters: parameters(text, decl.name_start, &decl.name).map(|(_, p)| p),
            result: tokens(&decl.was),
            headers: HashSet::new(),
            reachable: HashSet::new(),
            unproven_context: HashSet::new(),
            types: Types::default(),
        };
        let global = boundary.scope.as_ref().is_some_and(Vec::is_empty);
        boundary.types.observe_macros(text, false);
        if global {
            boundary.types.observe(text);
        }
        let mut pending = includes(file, text, &boundary.search);
        let mut seen = HashSet::new();
        let mut contents = Vec::new();
        while let Some((header, global_context)) = pending.pop() {
            if !seen.insert((header.clone(), global_context)) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&header) else {
                continue;
            };
            if boundary.reachable.insert(header.clone()) {
                boundary.types.observe_macros(&content, true);
            }
            if !global_context {
                boundary.unproven_context.insert(header.clone());
            }
            pending.extend(
                includes(&header, &content, &boundary.search)
                    .into_iter()
                    .map(|(path, local)| (path, global_context && local)),
            );
            contents.push((header, content));
        }
        for (header, content) in &contents {
            if boundary.types.guard_suppressed(content) {
                boundary.unproven_context.insert(header.clone());
            }
        }
        while contents.iter().any(|(header, content)| {
            boundary.unproven_context.contains(header)
                && includes(header, content, &boundary.search)
                    .into_iter()
                    .any(|(child, _)| boundary.unproven_context.insert(child))
        }) {}
        // Finish the include-context graph before trusting any fallback type
        // facts. A guard may suppress the global visit after a namespace visit;
        // neither aliases nor nominal declarations from that header are proven
        // global, regardless of traversal order. Precise membership still works.
        if global {
            for (header, content) in &contents {
                if !boundary.unproven_context.contains(header) {
                    boundary.types.observe(content);
                }
            }
        }
        for (header, content) in contents {
            if boundary.unproven_context.contains(&header) {
                continue;
            }
            let clean = code(&content);
            if clean.match_indices(&decl.name).any(|(at, _)| {
                boundary.same_declaration(&content, at)
                    && prototype_span(&content, at, &decl.name).is_some()
            }) {
                boundary.headers.insert(header.clone());
            }
        }
        boundary
    }

    fn same_declaration(&self, text: &str, at: usize) -> bool {
        let Some((_, params)) = parameters(text, at, &self.name) else {
            return false;
        };
        self.scope.is_some()
            && scope(text, at) == self.scope
            && self
                .parameters
                .as_ref()
                .is_some_and(|selected| self.types.same(selected, &params) == Some(true))
            && prototype_span(text, at, &self.name)
                .is_some_and(|(_, _, ret)| tokens(&ret) == self.result)
    }

    pub(super) fn imports(&self, path: &Path, text: &str) -> bool {
        if self.headers.contains(&canonical(path)) {
            return true;
        }
        let search = Search::new(&self.root, path);
        let mut pending = includes(path, text, &search);
        let mut seen = HashSet::new();
        while let Some((header, _)) = pending.pop() {
            if self.headers.contains(&header) {
                return true;
            }
            if !seen.insert(header.clone()) {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&header) {
                pending.extend(includes(&header, &content, &search));
            }
        }
        false
    }

    pub(super) fn identity(&self, path: &Path, text: &str, at: usize, semantic: bool) -> Identity {
        let prototype = prototype_span(text, at, &self.name).is_some();
        // A precise declaration reference resolves aliases and declarator spelling
        // beyond the fallback key. Reachability still prevents an unrelated loose
        // header from acquiring ownership merely through its filename.
        if prototype
            && semantic
            && (self.reachable.contains(&canonical(path)) || canonical(path) == self.file)
        {
            return Identity::Selected;
        }
        if prototype && self.unproven_context.contains(&canonical(path)) {
            return Identity::Unknown;
        }
        let Some(selected) = &self.scope else {
            return Identity::Unknown;
        };
        let Some(candidate) = scope(text, at) else {
            return Identity::Unknown;
        };
        if candidate != *selected {
            let clean = code(text);
            let qualified = clean[..at].trim_end().ends_with("::");
            let definition = parameters(&clean, at, &self.name)
                .is_some_and(|(close, _)| clean[close + 1..].trim_start().starts_with('{'));
            if qualified || definition || prototype_span(text, at, &self.name).is_some() {
                return Identity::Unrelated;
            }
            // Unqualified lookup can find an outer namespace's declaration.
            return if semantic {
                Identity::Selected
            } else {
                Identity::Unknown
            };
        }
        if prototype {
            if let (Some(selected), Some((_, candidate))) =
                (&self.parameters, parameters(text, at, &self.name))
                && self.types.same(selected, &candidate) == Some(false)
            {
                return Identity::Unrelated;
            }
            return if (canonical(path) == self.file || self.headers.contains(&canonical(path)))
                && self.same_declaration(text, at)
            {
                Identity::Selected
            } else {
                Identity::Unknown
            };
        }
        if canonical(path) == self.file || semantic || self.imports(path, text) {
            Identity::Selected
        } else {
            Identity::Unknown
        }
    }
}

/// Replace code tokens in the return span while retaining every comment/whitespace byte.
pub(super) fn replace_type(
    text: &str,
    start: usize,
    end: usize,
    now: &str,
    edits: &mut Vec<(usize, usize, String)>,
) {
    let clean = super::cpp_attributes::type_code(text);
    let mut first = true;
    for (begin, finish) in token_spans(text) {
        if begin < start || finish > end || clean.as_bytes()[begin].is_ascii_whitespace() {
            continue;
        }
        edits.push((
            begin,
            finish - begin,
            if first {
                first = false;
                now.to_string()
            } else {
                String::new()
            },
        ));
    }
}
