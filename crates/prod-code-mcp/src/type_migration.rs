/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Changing a declared type, and reporting the whole shape of what that breaks.
//!
//! It moves the declaration and then tells the truth about the size of the job, site by site,
//! before any of it is done. Where a site's error is exactly the old type meeting the new one it
//! says what conversion would fix it. With `convert` it goes one step further and writes
//! `.into()` at those sites — but only where the analyzer, checking the whole overlay again,
//! accepts it. A wrong `.into()` inserted at forty call sites is the kind of plausible damage the
//! rest of these tools exist to avoid, so a conversion that does not type-check is taken back and
//! its site stays in the report, and a set of conversions that breaks anything else is dropped.

#![allow(clippy::collapsible_if, clippy::needless_range_loop)]

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use crate::parameter_object::Language;

/// One place the new type does not fit, with enough context to judge it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Site {
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// The analyzer's message, first line.
    pub message: String,
    pub code: Option<String>,
    /// The line of source, trimmed.
    pub source: String,
    /// What would fix this site, when the shape of the error says so plainly.
    pub suggestion: Option<String>,
    /// Where the analyzer's range for it ends, 1-based line and column.
    #[serde(skip)]
    pub end: Option<(u32, u32)>,
}

/// A conversion written at a site: the value that was there, and the value that is there now.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Conversion {
    pub file: String,
    pub line: u32,
    pub was: String,
    pub now: String,
}

/// What the migration would do, and what it would leave to be done.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Migration {
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub was: String,
    pub now: String,
    /// The declaration's file, rewritten.
    pub rewritten: Vec<(String, String)>,
    /// Every site the new type does not fit, in file order.
    pub sites: Vec<Site>,
    /// How many diagnostics the change caused on an attribute rather than on code: an error
    /// inside what a derive generates is reported at the derive, and none of them is a place
    /// anyone can edit. Ones the file already had are not counted at all (#79).
    pub in_attributes: usize,
    /// The sites `convert` turned into conversions the analyzer accepts.
    pub converted: Vec<Conversion>,
    /// Why `convert` wrote nothing although it had candidates, when it did not.
    pub conversion_note: Option<String>,
    pub applied: bool,
    #[serde(default)]
    pub transitive_count: usize,
    #[serde(default)]
    pub transitively_migrated: Vec<String>,
}

impl Migration {
    /// The report: the declaration, then the work the change creates.
    pub fn render(&self, budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: `{}`\n- now: `{}`\n",
            self.symbol, self.file, self.was, self.now
        );
        if !self.converted.is_empty() {
            let label = if self.converted.iter().all(|c| c.now.ends_with(".into()")) {
                "converted with `.into()`"
            } else {
                "converted with language-idiomatic conversion"
            };
            out.push_str(&format!(
                "\n{} site(s) {label}, each accepted by the analyzer:\n",
                self.converted.len()
            ));
            for c in &self.converted {
                out.push_str(&format!(
                    "  {}:{}  `{}` → `{}`\n",
                    c.file, c.line, c.was, c.now
                ));
            }
        }
        if self.transitive_count > 0 {
            out.push_str(&format!(
                "\n{} declaration(s) transitively migrated along data-flow graph:\n",
                self.transitive_count
            ));
            for decl in &self.transitively_migrated {
                out.push_str(&format!("  {decl}\n"));
            }
        }
        if let Some(note) = &self.conversion_note {
            out.push_str(&format!("\n{note}\n"));
        }
        if self.sites.is_empty() {
            out.push_str("\nnothing else has to change: the analyzer accepts the new type\n");
        } else {
            let files = self
                .sites
                .iter()
                .map(|s| s.file.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len();
            out.push_str(&format!(
                "\n{} site(s) in {files} file(s) do not fit the new type:\n",
                self.sites.len()
            ));
            let mut current = String::new();
            for (written, site) in self.sites.iter().enumerate() {
                if site.file != current {
                    current = site.file.clone();
                    out.push_str(&format!("\n{current}\n"));
                }
                if written >= budget {
                    out.push_str(&format!(
                        "  … {} more site(s)\n",
                        self.sites.len() - written
                    ));
                    break;
                }
                out.push_str(&format!(
                    "  {}:{}  {}{}\n      {}\n",
                    site.line,
                    site.col,
                    site.message,
                    site.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    site.source
                ));
                if let Some(fix) = &site.suggestion {
                    out.push_str(&format!("      try: {fix}\n"));
                }
            }
            out.push_str(
                "\nthese are not failures, they are the migration: each site needs a decision \
                 about how the old value becomes the new one. Nothing is suggested that this \
                 cannot see plainly, and a conversion is written only when `convert` is set and \
                 the analyzer accepts it.\n",
            );
        }
        if self.in_attributes > 0 {
            out.push_str(&format!(
                "\n{} further diagnostic(s) landed on a `#[derive(…)]` line rather than on code. \
                 An error inside what a derive generates is reported at the derive, and there \
                 is nothing at those positions to edit; they are left out of the list above.\n",
                self.in_attributes
            ));
        }
        if self.applied && !self.converted.is_empty() {
            let rest = if self.sites.is_empty() {
                ""
            } else {
                "; the sites above were not"
            };
            out.push_str(&format!(
                "\n[the declaration and {} conversion(s) were written{rest}]\n",
                self.converted.len()
            ));
        } else if self.applied {
            out.push_str("\n[the declaration was written; the sites above were not]\n");
        } else if !self.converted.is_empty() {
            out.push_str(
                "\nnothing was written; pass `apply: true` to write the declaration and the \
                 conversions, and `force: true` while sites remain\n",
            );
        } else {
            out.push_str(
                "\nnothing was written; pass `apply: true` to write the declaration alone, and \
                 `force: true` while sites remain\n",
            );
        }
        out
    }
}

/// The span of the type in a declaration, given the offset of the declared name.
///
/// Four shapes cover what can be migrated: a field or a parameter or an annotated `let`, which
/// are `name: Type` and end at the first `,`, `)`, `;` or `=` that is not inside brackets; and
/// a function, whose type is what follows `->`.
pub fn declared_type_span(text: &str, name_offset: usize) -> Option<(usize, usize)> {
    declared_type_span_polyglot(text, name_offset, Language::Rust)
}

/// The span of the type in a declaration across polyglot languages (Rust, TypeScript/JavaScript,
/// Python, C++, Swift, Go).
pub fn declared_type_span_polyglot(
    text: &str,
    name_offset: usize,
    lang: Language,
) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = name_offset;
    while i < bytes.len() && (bytes[i] == b'_' || (bytes[i] as char).is_alphanumeric()) {
        i += 1;
    }
    let after_name = i;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }

    // Function return type
    if bytes.get(i) == Some(&b'(') {
        let close = matching(text, i)?;
        match lang {
            Language::Rust | Language::Swift => {
                let arrow = text[close..].find("->")? + close;
                let body = text[close..].find(['{', ';']).map(|b| b + close);
                if body.is_some_and(|b| b < arrow) {
                    return None;
                }
                let start = arrow + 2;
                let start = start + text[start..].len() - text[start..].trim_start().len();
                let end = end_of_type(text, start, b"{;\n")?;
                return Some((start, end));
            }
            Language::Python => {
                let colon = text[close..].find(':')? + close;
                let between = &text[close..colon];
                let arrow = between.find("->")? + close;
                let start = arrow + 2;
                let start = start + text[start..].len() - text[start..].trim_start().len();
                let end = colon - (text[start..colon].len() - text[start..colon].trim_end().len());
                return Some((start, end));
            }
            Language::TypeScript | Language::JavaScript => {
                let body = text[close..].find(['{', ';']).map(|b| b + close);
                let arrow = text[close..].find("=>").map(|b| b + close);
                let end_header = match (body, arrow) {
                    (Some(b), Some(a)) => b.min(a),
                    (Some(b), None) => b,
                    (None, Some(a)) => a,
                    (None, None) => return None,
                };
                let colon = text[close..end_header].find(':')? + close;
                let start = colon + 1;
                let start = start + text[start..].len() - text[start..].trim_start().len();
                let end = end_of_type(text, start, b"{;=\n")?;
                return Some((start, end));
            }
            Language::Go => {
                let body = text[close..].find('{')? + close;
                let header = text[close + 1..body].trim();
                if header.is_empty() {
                    return None;
                }
                let start = close + 1 + (text[close + 1..body].len() - text[close + 1..body].trim_start().len());
                let end = body - (text[close + 1..body].len() - text[close + 1..body].trim_end().len());
                return Some((start, end));
            }
            Language::Cpp | Language::C | Language::Java => {
                // In C/C++, return type is before function name
                let line_start = text[..name_offset].rfind(['\n', ';', '{', '}']).map_or(0, |p| p + 1);
                let before = text[line_start..name_offset].trim();
                let words: Vec<&str> = before.split_whitespace().collect();
                if words.is_empty() {
                    return None;
                }
                let filtered: Vec<&str> = words.into_iter()
                    .filter(|w| !matches!(*w, "virtual" | "static" | "inline" | "constexpr" | "friend"))
                    .collect();
                if filtered.is_empty() {
                    return None;
                }
                let start = text[line_start..name_offset].find(filtered[0])? + line_start;
                let last = filtered.last().unwrap();
                let end_rel = text[start..name_offset].rfind(last)? + last.len();
                return Some((start, start + end_rel));
            }
        }
    }

    // Parameters, fields, and variables
    match lang {
        Language::Rust | Language::Swift | Language::TypeScript | Language::JavaScript | Language::Python => {
            let mut check_pos = after_name;
            while check_pos < bytes.len() && (bytes[check_pos] as char).is_whitespace() {
                check_pos += 1;
            }
            if bytes.get(check_pos) == Some(&b'?') {
                check_pos += 1;
                while check_pos < bytes.len() && (bytes[check_pos] as char).is_whitespace() {
                    check_pos += 1;
                }
            }
            if bytes.get(check_pos) == Some(&b':') {
                let start = check_pos + 1;
                let start = start + text[start..].len() - text[start..].trim_start().len();
                let end = end_of_type(text, start, b",);=\n#")?;
                return Some((start, end));
            }
            None
        }
        Language::Go => {
            if i < bytes.len() && !matches!(bytes[i], b'=' | b':' | b',' | b')' | b'{' | b';' | b'\n') {
                let start = i;
                let end = end_of_type(text, start, b",);=\n{`")?;
                return Some((start, end));
            }
            None
        }
        Language::Cpp | Language::C | Language::Java => {
            let line_start = text[..name_offset].rfind(['\n', ';', '{', '}', '(', ',']).map_or(0, |p| p + 1);
            let before = text[line_start..name_offset].trim();
            if before.is_empty() {
                return None;
            }
            let words: Vec<&str> = before.split_whitespace().collect();
            let filtered: Vec<&str> = words.into_iter()
                .filter(|w| !matches!(*w, "auto" | "register" | "static" | "extern" | "public:" | "private:" | "protected:"))
                .collect();
            if filtered.is_empty() {
                return None;
            }
            let start = text[line_start..name_offset].find(filtered[0])? + line_start;
            let last = filtered.last().unwrap();
            let end_rel = text[start..name_offset].rfind(last)? + last.len();
            Some((start, start + end_rel))
        }
    }
}

/// The offset just past the `)` that closes the `(` at `open`.
fn matching(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    for (i, c) in bytes.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Where a type written at `start` ends: the first terminator at bracket depth zero.
fn end_of_type(text: &str, start: usize, terminators: &[u8]) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        let prev = if i > 0 { bytes[i - 1] } else { b' ' };
        let next = bytes.get(i + 1).copied().unwrap_or(b' ');
        // The terminator is read before the depth is touched: `{` ends a return type and also
        // opens a block, and deciding in the other order loses every function's type.
        if depth == 0 && terminators.contains(&c) && !(c == b'=' && (next == b'=' || next == b'>'))
        {
            return Some(text[..i].trim_end().len());
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            // `<` and `>` nest a type, but `->` and `=>` are not brackets.
            b'<' if prev != b'-' && prev != b'=' => depth += 1,
            b'>' if prev != b'-' && prev != b'=' && depth > 0 => depth -= 1,
            _ => {}
        }
        if depth == 0 && c == b'\n' && terminators.contains(&b',') {
            // A field or parameter written without its trailing comma still ends at its line.
            return Some(text[..i].trim_end().len());
        }
        i += 1;
    }
    None
}

/// What would make the old value fit the new type, when the error says so plainly.
///
/// Only the two shapes that are unambiguous: the new type meeting the old one, either way
/// round. Anything else gets no suggestion rather than a guess.
pub fn suggest(message: &str, was: &str, now: &str) -> Option<String> {
    let (expected, found) = parse_mismatch(message)?;
    let (expected, found) = (type_name(&expected), type_name(&found));
    let (was, now) = (type_name(was), type_name(now));
    if expected == now && found == was {
        return Some(format!(
            "the value here is still `{was}`; convert it to `{now}`"
        ));
    }
    if expected == was && found == now {
        return Some(format!(
            "this place still wants `{was}` and is now given `{now}`; migrate it too, or convert \
             back here"
        ));
    }
    None
}

/// A type as the analyzer names it. It names a type as it is in scope, not as the declaration
/// spells it — `std::time::Duration` comes back as `Duration`, at any depth — and it spells out
/// the default allocator: `Box<str>` comes back as `Box<str, Global>`.
pub fn type_name(ty: &str) -> String {
    let mut out = String::new();
    for c in ty.trim().replace(", Global>", ">").chars() {
        out.push(c);
        if out.ends_with("::") {
            out.truncate(out.len() - 2);
            while out.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                out.pop();
            }
        }
    }
    out
}

/// `expected X, found Y` out of an analyzer message across Rust and polyglot languages.
fn parse_mismatch(message: &str) -> Option<(String, String)> {
    // Rust: "expected X, found Y"
    if let Some(rest) = message.split("expected ").nth(1)
        && let Some((expected, rest)) = rest.split_once(", found ") {
            let found = rest
                .split(['\n', ' '])
                .next()
                .unwrap_or(rest)
                .trim_end_matches(['.', ',']);
            return Some((expected.trim().to_string(), found.trim().to_string()));
        }
    // TypeScript: "Type 'X' is not assignable to type 'Y'"
    if let Some((before, after)) = message.split_once(" is not assignable to type ") {
        let found = before.rsplit('\'').nth(1).or_else(|| before.split('\'').nth(1)).unwrap_or(before).trim();
        let expected = after.split('\'').nth(1).unwrap_or(after).trim();
        return Some((expected.to_string(), found.to_string()));
    }
    // Python (basedpyright): 'Expression of type "X" cannot be assigned to declared type "Y"'
    if message.contains("cannot be assigned to") {
        let parts: Vec<&str> = message.split('"').collect();
        if parts.len() >= 4 {
            let found = parts[1];
            let expected = parts[parts.len() - 2];
            return Some((expected.to_string(), found.to_string()));
        }
    }
    // Go: "cannot use X (variable of type A) as B value"
    if message.contains("cannot use") && message.contains(" as ")
        && let Some(of_type) = message.split("variable of type ").nth(1)
            && let Some((found, rest)) = of_type.split_once(')')
                && let Some(as_type) = rest.split(" as ").nth(1) {
                    let expected = as_type.split_whitespace().next().unwrap_or(as_type).trim();
                    return Some((expected.to_string(), found.trim().to_string()));
                }
    // Swift: "cannot convert value of type 'X' to specified type 'Y'"
    if message.contains("cannot convert value of type") {
        let parts: Vec<&str> = message.split('\'').collect();
        if parts.len() >= 4 {
            let found = parts[1];
            let expected = parts[parts.len() - 2];
            return Some((expected.to_string(), found.to_string()));
        }
    }
    // C++: "no viable conversion from 'X' to 'Y'"
    if message.contains("no viable conversion from") {
        let parts: Vec<&str> = message.split('\'').collect();
        if parts.len() >= 4 {
            let found = parts[1];
            let expected = parts[3];
            return Some((expected.to_string(), found.to_string()));
        }
    }
    None
}

/// Generates a language-idiomatic conversion expression when `convert: true`.
pub fn language_conversion(expr: &str, target_type: &str, lang: Language) -> String {
    let t = target_type.trim();
    match lang {
        Language::Rust => into_call(expr),
        Language::TypeScript | Language::JavaScript => {
            if t == "number" {
                format!("Number({expr})")
            } else if t == "string" {
                format!("String({expr})")
            } else if t == "boolean" {
                format!("Boolean({expr})")
            } else if t == "bigint" {
                format!("BigInt({expr})")
            } else if expr.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') {
                format!("{t}({expr})")
            } else {
                format!("({expr} as {t})")
            }
        }
        Language::Python => {
            format!("{t}({expr})")
        }
        Language::Go => {
            format!("{t}({expr})")
        }
        Language::Swift => {
            format!("{t}({expr})")
        }
        Language::Cpp | Language::C => {
            format!("static_cast<{t}>({expr})")
        }
        Language::Java => {
            if matches!(t, "int" | "long" | "float" | "double" | "byte" | "short" | "char") {
                format!("({t}) ({expr})")
            } else if t == "String" {
                format!("String.valueOf({expr})")
            } else {
                format!("({t}) ({expr})")
            }
        }
    }
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Changes the declared type of the symbol at `file:line:col` and reports what no longer fits.
#[allow(clippy::too_many_arguments)]
pub async fn migrate(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    to: &str,
    convert: bool,
    apply: bool,
    force: bool,
) -> Result<Migration> {
    migrate_ext(
        remote,
        root,
        file,
        None,
        Some(line),
        Some(col),
        to,
        convert,
        false,
        apply,
        force,
    )
    .await
}

/// Polyglot, transitive type migration across Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.
#[allow(clippy::too_many_arguments)]
pub async fn migrate_ext(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    to: &str,
    convert: bool,
    transitive: bool,
    apply: bool,
    force: bool,
) -> Result<Migration> {
    anyhow::ensure!(!to.trim().is_empty(), "the new type is empty");
    let lang = Language::of(file).unwrap_or(Language::Rust);
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let (offset, name) = if let (Some(l), Some(c)) = (line, col) {
        let off = crate::signature::offset_of(&text, l, c)
            .context("the declaration is not at the resolved position")?;
        let n: String = text[off..]
            .chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect();
        anyhow::ensure!(!n.is_empty(), "there is no declared name at that position");
        (off, n)
    } else if let Some(sym) = symbol {
        let clean_sym = sym.rsplit("::").next().unwrap_or(sym).rsplit('.').next().unwrap_or(sym).trim();
        let off = find_symbol_decl_offset(&text, clean_sym, lang, line)
            .with_context(|| format!("could not locate declaration of `{clean_sym}` in {}", file.display()))?;
        (off, clean_sym.to_string())
    } else {
        anyhow::bail!("Missing 'symbol' or 'line' and 'character'");
    };

    let (start, end) = declared_type_span_polyglot(&text, offset, lang).with_context(|| {
        format!(
            "`{name}` has no declared type this understands: a field, a parameter, an annotated \
             variable or a function's return type"
        )
    })?;
    let was = text[start..end].trim().to_string();
    anyhow::ensure!(
        was != to.trim(),
        "`{name}` is already declared as `{to}`"
    );

    let mut new_text = text.clone();
    new_text.replace_range(start..end, to);
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), new_text);

    let mut also: Vec<PathBuf> = if let (Some(l), Some(c)) = (line, col) {
        crate::signature::references(remote, root, file, l, c)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(path, _, _)| path)
            .filter(|path| path != file)
            .collect()
    } else {
        Vec::new()
    };
    also.sort();
    also.dedup();

    // C/C++ prototype synchronization in headers
    if matches!(lang, Language::Cpp | Language::C) {
        sync_cpp_headers(root, file, &name, &was, to, &mut rewritten, &mut also, false);
    }

    let mut transitive_count = 0;
    let mut transitively_migrated = Vec::new();
    if transitive {
        let (t_count, t_migrated) = propagate_transitive(
            root,
            &mut rewritten,
            file,
            &name,
            &was,
            to,
            lang,
            &mut also,
        );
        transitive_count = t_count;
        transitively_migrated = t_migrated;
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &also).await?;

    let (mut sites, mut in_attributes) = collect_sites(root, &rewritten, &reports, &was, to);

    let mut converted = Vec::new();
    let mut conversion_note = None;
    if convert {
        let outcome = convert_sites(remote, root, &rewritten, &also, &sites, &was, to).await?;
        match outcome {
            Converted::Accepted {
                texts,
                conversions,
                reports,
                tried,
            } => {
                let (after, attrs) = collect_sites(root, &texts, &reports, &was, to);
                sites = after;
                in_attributes = attrs;
                rewritten = texts;
                converted = conversions;
                mark_tried(&mut sites, &tried);
            }
            Converted::Nothing { tried } => mark_tried(&mut sites, &tried),
            Converted::Dropped { note, tried } => {
                conversion_note = Some(note);
                mark_tried(&mut sites, &tried);
            }
        }
    }

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            sites.is_empty() || force,
            "{} site(s) do not fit the new type; nothing was written. Read them first, then \
             pass `force: true` to write the declaration{} and migrate the sites yourself",
            sites.len(),
            if converted.is_empty() {
                ""
            } else {
                " and the conversions"
            }
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(Migration {
        symbol: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now: to.to_string(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        sites,
        in_attributes,
        converted,
        conversion_note,
        applied,
        transitive_count,
        transitively_migrated,
    })
}

/// The errors of `reports` as sites, collapsed and with the ones on attributes counted apart.
fn collect_sites(
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    reports: &[crate::diagnostics::DiagnosticsReport],
    was: &str,
    to: &str,
) -> (Vec<Site>, usize) {
    let mut sites = Vec::new();
    for report in reports {
        let source_of = |line: u32| -> String {
            let path = root.join(&report.file);
            let text = rewritten
                .iter()
                .find(|(p, _)| **p == path || display(root, p) == report.file)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default());
            text.lines()
                .nth(line.saturating_sub(1) as usize)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        for item in report.items.iter().chain(&report.in_derive) {
            if item.severity != "error" {
                continue;
            }
            let message = item.message.lines().next().unwrap_or("").to_string();
            sites.push(Site {
                file: report.file.clone(),
                line: item.line,
                col: item.col,
                suggestion: suggest(&message, was, to),
                message,
                code: item.code.clone(),
                source: source_of(item.line),
                end: item.end,
            });
        }
    }
    sites.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.col.cmp(&b.col))
            .then(a.message.cmp(&b.message))
    });
    sites.dedup_by(|a, b| {
        a.file == b.file && a.line == b.line && a.col == b.col && a.message == b.message
    });
    let before = sites.len();
    sites.retain(|s| !s.source.trim_start().starts_with("#["));
    let in_attributes = before - sites.len();
    (sites, in_attributes)
}

/// Whether a language-idiomatic conversion can go on this site's expression.
fn is_candidate(site: &Site, was: &str, now: &str) -> bool {
    if site.file.ends_with(".rs") {
        if site.code.as_deref() != Some("E0308") {
            return false;
        }
        if !site.end.is_some_and(|(line, _)| line == site.line) {
            return false;
        }
    } else {
        let is_candidate_code = site.code.as_deref() == Some("E0308")
            || site.code.as_deref().is_some_and(|c| c.contains("2322") || c.contains("2345") || c.contains("type") || c.contains("error"))
            || site.code.is_none()
            || site.message.contains("expected")
            || site.message.contains("not assignable")
            || site.message.contains("cannot convert")
            || site.message.contains("conversion")
            || site.message.contains("cannot use");
        if !is_candidate_code {
            return false;
        }
        if !site.end.is_none_or(|(line, _)| line == site.line) {
            return false;
        }
    }

    let Some((expected, found)) = parse_mismatch(&site.message) else {
        return false;
    };
    let (expected, found) = (type_name(&expected), type_name(&found));
    let (was, now) = (type_name(was), type_name(now));
    (expected == now && found == was) || (expected == was && found == now)
}

/// `expr.into()`, with parentheses unless the expression is a path, a call chain or a literal
/// that a method call binds to as a whole.
pub fn into_call(expr: &str) -> String {
    let mut depth = 0i32;
    let mut simple = !expr.is_empty();
    for c in expr.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c == '_' || c == '.' || c == ':' => {}
            _ => simple = false,
        }
    }
    let starts_well = expr
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_');
    if simple && starts_well {
        format!("{expr}.into()")
    } else {
        format!("({expr}).into()")
    }
}

/// A site's expression in its file's text, as a byte range.
pub fn expression_span(text: &str, site: &Site) -> Option<(usize, usize)> {
    let start = crate::signature::offset_of(text, site.line, site.col)?;
    let mut end = if let Some((end_line, end_col)) = site.end {
        crate::signature::offset_of(text, end_line, end_col)?
    } else {
        let line_rest = &text[start..];
        let token_len: usize = line_rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
            .map(|c| c.len_utf8())
            .sum();
        if token_len == 0 {
            return None;
        }
        start + token_len
    };
    if text[end..].starts_with('(') {
        let mut depth = 0i32;
        for (i, c) in text[end..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end += i + 1;
                        break;
                    }
                }
                '\n' => return None,
                _ => {}
            }
        }
    }
    let expr = text.get(start..end)?;
    if start >= end || expr.contains('\n') || expr.trim().is_empty() {
        return None;
    }
    let file_lang = Language::of(Path::new(&site.file)).unwrap_or(Language::Rust);
    if file_lang == Language::Rust && text[..start].ends_with('.') && into_call(expr).starts_with('(') {
        return None;
    }
    Some((start, end))
}

enum Converted {
    Accepted {
        texts: BTreeMap<PathBuf, String>,
        conversions: Vec<Conversion>,
        reports: Vec<crate::diagnostics::DiagnosticsReport>,
        tried: BTreeSet<(String, u32, u32)>,
    },
    Nothing { tried: BTreeSet<(String, u32, u32)> },
    Dropped {
        note: String,
        tried: BTreeSet<(String, u32, u32)>,
    },
}

async fn convert_sites(
    remote: SocketAddr,
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    also: &[PathBuf],
    sites: &[Site],
    was: &str,
    now: &str,
) -> Result<Converted> {
    let known: BTreeSet<(String, u32, String)> = sites
        .iter()
        .map(|s| (s.file.clone(), s.line, s.message.clone()))
        .collect();
    let mut candidates: Vec<&Site> = sites.iter().filter(|s| is_candidate(s, was, now)).collect();
    let mut tried: BTreeSet<(String, u32, u32)> = BTreeSet::new();
    for _round in 0..4 {
        if candidates.is_empty() {
            return Ok(Converted::Nothing { tried });
        }
        let mut texts = rewritten.clone();
        let mut spans: BTreeMap<PathBuf, Vec<(usize, usize, &Site)>> = BTreeMap::new();
        for site in &candidates {
            let path = root.join(&site.file);
            let key = texts
                .keys()
                .find(|p| **p == path || display(root, p) == site.file)
                .cloned()
                .unwrap_or(path);
            let text = match texts.get(&key) {
                Some(t) => t.clone(),
                None => match std::fs::read_to_string(&key) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            texts.entry(key.clone()).or_insert(text.clone());
            if let Some((start, end)) = expression_span(&text, site) {
                spans.entry(key).or_default().push((start, end, site));
            }
        }
        let mut conversions = Vec::new();
        for (path, mut list) in spans {
            list.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
            let text = texts.get_mut(&path).expect("read above");
            let file_lang = Language::of(&path).unwrap_or(Language::Rust);
            for (start, end, site) in list {
                let expr = text[start..end].to_string();
                let call = language_conversion(&expr, now, file_lang);
                text.replace_range(start..end, &call);
                conversions.push(Conversion {
                    file: site.file.clone(),
                    line: site.line,
                    was: expr,
                    now: call,
                });
            }
        }
        conversions.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        let edits: Vec<(PathBuf, String)> =
            texts.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
        let others: Vec<PathBuf> = also
            .iter()
            .filter(|p| !texts.contains_key(*p))
            .cloned()
            .collect();
        let reports = crate::diagnostics::validate_texts(remote, root, &edits, &others).await?;
        let failing: BTreeSet<(String, u32)> = reports
            .iter()
            .flat_map(|r| {
                r.items
                    .iter()
                    .filter(|d| d.severity == "error")
                    .map(move |d| (r.file.clone(), d.line))
            })
            .collect();
        let before = candidates.len();
        candidates.retain(|s| {
            let bad = failing.contains(&(s.file.clone(), s.line));
            if bad {
                tried.insert((s.file.clone(), s.line, s.col));
            }
            !bad
        });
        if candidates.len() < before {
            continue;
        }
        let converted_lines: BTreeSet<(String, u32)> = conversions
            .iter()
            .map(|c| (c.file.clone(), c.line))
            .collect();
        let new: Vec<String> = reports
            .iter()
            .flat_map(|r| {
                r.items
                    .iter()
                    .filter(|d| d.severity == "error")
                    .map(move |d| (r.file.clone(), d.line, d.message.clone()))
            })
            .filter(|(f, l, m)| {
                let first = m.lines().next().unwrap_or("").to_string();
                !known.contains(&(f.clone(), *l, first))
                    && !converted_lines.contains(&(f.clone(), *l))
            })
            .map(|(f, l, m)| format!("{f}:{l} {}", m.lines().next().unwrap_or("")))
            .collect();
        if !new.is_empty() {
            return Ok(Converted::Dropped {
                note: format!(
                    "{} conversion(s) type-check where they are but cause an error elsewhere, so \
                     none was kept:\n  {}",
                    conversions.len(),
                    new.join("\n  ")
                ),
                tried,
            });
        }
        return Ok(Converted::Accepted {
            texts,
            conversions,
            reports,
            tried,
        });
    }
    Ok(Converted::Dropped {
        note: "the conversions did not settle in four rounds of checking, so none was kept"
            .to_string(),
        tried,
    })
}

fn mark_tried(sites: &mut [Site], tried: &BTreeSet<(String, u32, u32)>) {
    for site in sites {
        if tried.contains(&(site.file.clone(), site.line, site.col)) {
            site.suggestion = Some(
                "A conversion was tried here and does not type-check: there is no conversion the \
                 analyzer accepts, so this one is a decision (a narrowing, a fallible \
                 conversion, or a place that should be migrated too)"
                    .to_string(),
            );
        }
    }
}

/// Transitively propagate type changes along data-flow edges (variables, return signatures, parameters, fields).
#[allow(clippy::too_many_arguments)]
fn propagate_transitive(
    root: &Path,
    rewritten: &mut BTreeMap<PathBuf, String>,
    initial_file: &Path,
    initial_name: &str,
    was: &str,
    to: &str,
    lang: Language,
    also: &mut Vec<PathBuf>,
) -> (usize, Vec<String>) {
    let mut queue: VecDeque<(PathBuf, String, String, String)> = VecDeque::new();
    let mut visited: BTreeSet<(PathBuf, String)> = BTreeSet::new();
    let mut migrated_descriptions = Vec::new();

    queue.push_back((
        initial_file.to_path_buf(),
        initial_name.to_string(),
        was.to_string(),
        to.to_string(),
    ));
    visited.insert((initial_file.to_path_buf(), initial_name.to_string()));

    while let Some((cur_file, cur_name, cur_was, cur_to)) = queue.pop_front() {
        let cur_flang = Language::of(&cur_file).unwrap_or(lang);
        let text = match rewritten.get(&cur_file) {
            Some(t) => t.clone(),
            None => match std::fs::read_to_string(&cur_file) {
                Ok(t) => {
                    rewritten.insert(cur_file.clone(), t.clone());
                    t
                }
                Err(_) => continue,
            },
        };

        // 1. Downstream variable bindings in cur_file
        let var_matches = find_matching_vars(&text, &cur_name, &cur_was, cur_flang);
        if !var_matches.is_empty() {
            let mut updated_text = text.clone();
            let mut sorted_vars = var_matches;
            sorted_vars.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
            for (s, e, var_name) in sorted_vars {
                updated_text.replace_range(s..e, &cur_to);
                migrated_descriptions.push(format!(
                    "{}:{var_name} (var {cur_was} → {cur_to})",
                    display(root, &cur_file)
                ));
                if !visited.contains(&(cur_file.clone(), var_name.clone())) {
                    visited.insert((cur_file.clone(), var_name.clone()));
                    queue.push_back((
                        cur_file.clone(),
                        var_name,
                        cur_was.clone(),
                        cur_to.clone(),
                    ));
                }
            }
            rewritten.insert(cur_file.clone(), updated_text);
        }

        // 2. Return statements in cur_file
        let text_after_vars = rewritten.get(&cur_file).cloned().unwrap_or(text);
        if let Some((s, e, fn_name)) = find_matching_return(&text_after_vars, &cur_name, &cur_was, cur_flang) {
            let mut updated_text = text_after_vars.clone();
            updated_text.replace_range(s..e, &cur_to);
            rewritten.insert(cur_file.clone(), updated_text);
            migrated_descriptions.push(format!(
                "{}:{fn_name} (return {cur_was} → {cur_to})",
                display(root, &cur_file)
            ));

            if matches!(cur_flang, Language::Cpp | Language::C) {
                sync_cpp_headers(root, &cur_file, &fn_name, &cur_was, &cur_to, rewritten, also, true);
            }

            if !visited.contains(&(cur_file.clone(), fn_name.clone())) {
                visited.insert((cur_file.clone(), fn_name.clone()));
                queue.push_back((
                    cur_file.clone(),
                    fn_name,
                    cur_was.clone(),
                    cur_to.clone(),
                ));
            }
        }

        // 3. Call sites across workspace passing cur_name as argument to another function
        let candidate_files = collect_candidate_files(root, rewritten, &cur_name);
        for f in candidate_files {
            let f_text = match rewritten.get(&f) {
                Some(t) => t.clone(),
                None => match std::fs::read_to_string(&f) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            let f_lang = Language::of(&f).unwrap_or(lang);

            let call_param_matches = find_matching_call_params(
                root,
                &f_text,
                &cur_name,
                &cur_was,
                f_lang,
                rewritten,
            );
            for (callee_file, p_start, p_end, param_name, callee_name) in call_param_matches {
                let mut c_text = match rewritten.get(&callee_file) {
                    Some(t) => t.clone(),
                    None => match std::fs::read_to_string(&callee_file) {
                        Ok(t) => t,
                        Err(_) => continue,
                    },
                };
                c_text.replace_range(p_start..p_end, &cur_to);
                rewritten.insert(callee_file.clone(), c_text);
                if !also.contains(&callee_file) {
                    also.push(callee_file.clone());
                }
                migrated_descriptions.push(format!(
                    "{}:{callee_name}({param_name}) (param {cur_was} → {cur_to})",
                    display(root, &callee_file)
                ));

                let c_lang = Language::of(&callee_file).unwrap_or(lang);
                if matches!(c_lang, Language::Cpp | Language::C) {
                    sync_cpp_headers(root, &callee_file, &callee_name, &cur_was, &cur_to, rewritten, also, false);
                }

                if !visited.contains(&(callee_file.clone(), param_name.clone())) {
                    visited.insert((callee_file.clone(), param_name.clone()));
                    queue.push_back((
                        callee_file.clone(),
                        param_name,
                        cur_was.clone(),
                        cur_to.clone(),
                    ));
                }
            }

            // Callers assigning return value of cur_name
            let caller_var_matches = find_matching_caller_vars(&f_text, &cur_name, &cur_was, f_lang);
            if !caller_var_matches.is_empty() {
                let mut f_updated = f_text.clone();
                let mut sorted_cv = caller_var_matches;
                sorted_cv.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
                for (s, e, var_name) in sorted_cv {
                    f_updated.replace_range(s..e, &cur_to);
                    migrated_descriptions.push(format!(
                        "{}:{var_name} (caller var {cur_was} → {cur_to})",
                        display(root, &f)
                    ));
                    if !visited.contains(&(f.clone(), var_name.clone())) {
                        visited.insert((f.clone(), var_name.clone()));
                        queue.push_back((
                            f.clone(),
                            var_name,
                            cur_was.clone(),
                            cur_to.clone(),
                        ));
                    }
                }
                rewritten.insert(f.clone(), f_updated);
                if !also.contains(&f) {
                    also.push(f.clone());
                }
            }
        }
    }

    let count = migrated_descriptions.len();
    (count, migrated_descriptions)
}

fn contains_ident(haystack: &str, ident: &str) -> bool {
    for (idx, _) in haystack.match_indices(ident) {
        let before_ok = idx == 0 || !haystack[..idx].ends_with(|c: char| c.is_alphanumeric() || c == '_');
        let after_idx = idx + ident.len();
        let after_ok = after_idx == haystack.len() || !haystack[after_idx..].starts_with(|c: char| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

fn is_valid_ident(s: &str) -> bool {
    !s.is_empty()
        && (s.chars().next().unwrap().is_alphabetic() || s.starts_with('_'))
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn find_matching_vars(
    text: &str,
    sym: &str,
    old_ty: &str,
    lang: Language,
) -> Vec<(usize, usize, String)> {
    let mut results = Vec::new();
    let old_ty_norm = type_name(old_ty);

    for (line_no, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
            continue;
        }
        if !contains_ident(line, sym) {
            continue;
        }
        let line_offset = text.lines().take(line_no).map(|l| l.len() + 1).sum::<usize>();

        match lang {
            Language::Rust => {
                if let Some(rest) = trimmed.strip_prefix("let ") {
                    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
                    if let Some(colon) = rest.find(':') {
                        let var_name = rest[..colon].trim().to_string();
                        if let Some(eq) = rest.find('=')
                            && colon < eq {
                                let raw_ty = &rest[colon + 1..eq];
                                let ty_str = raw_ty.trim();
                                if type_name(ty_str) == old_ty_norm && contains_ident(&rest[eq + 1..], sym)
                                    && let Some(rel) = line.find(raw_ty) {
                                        let s_rel = rel + (raw_ty.len() - raw_ty.trim_start().len());
                                        let s = line_offset + s_rel;
                                        let e = s + ty_str.len();
                                        results.push((s, e, var_name));
                                    }
                            }
                    }
                }
            }
            Language::TypeScript | Language::JavaScript => {
                let rest_opt = trimmed.strip_prefix("const ")
                    .or_else(|| trimmed.strip_prefix("let "))
                    .or_else(|| trimmed.strip_prefix("var "));
                if let Some(rest) = rest_opt
                    && let Some(colon) = rest.find(':') {
                        let var_name = rest[..colon].trim().to_string();
                        if let Some(eq) = rest.find('=')
                            && colon < eq {
                                let raw_ty = &rest[colon + 1..eq];
                                let ty_str = raw_ty.trim();
                                if type_name(ty_str) == old_ty_norm && contains_ident(&rest[eq + 1..], sym)
                                    && let Some(rel) = line.find(raw_ty) {
                                        let s_rel = rel + (raw_ty.len() - raw_ty.trim_start().len());
                                        let s = line_offset + s_rel;
                                        let e = s + ty_str.len();
                                        results.push((s, e, var_name));
                                    }
                            }
                    }
            }
            Language::Python => {
                if let Some(colon) = trimmed.find(':') {
                    let var_name = trimmed[..colon].trim().to_string();
                    if !var_name.contains(' ') && is_valid_ident(&var_name)
                        && let Some(eq) = trimmed.find('=')
                            && colon < eq {
                                let raw_ty = &trimmed[colon + 1..eq];
                                let ty_str = raw_ty.trim();
                                if type_name(ty_str) == old_ty_norm && contains_ident(&trimmed[eq + 1..], sym)
                                    && let Some(rel) = line.find(raw_ty) {
                                        let s_rel = rel + (raw_ty.len() - raw_ty.trim_start().len());
                                        let s = line_offset + s_rel;
                                        let e = s + ty_str.len();
                                        results.push((s, e, var_name));
                                    }
                            }
                }
            }
            Language::Swift => {
                let rest_opt = trimmed.strip_prefix("let ")
                    .or_else(|| trimmed.strip_prefix("var "));
                if let Some(rest) = rest_opt
                    && let Some(colon) = rest.find(':') {
                        let var_name = rest[..colon].trim().to_string();
                        if let Some(eq) = rest.find('=')
                            && colon < eq {
                                let raw_ty = &rest[colon + 1..eq];
                                let ty_str = raw_ty.trim();
                                if type_name(ty_str) == old_ty_norm && contains_ident(&rest[eq + 1..], sym)
                                    && let Some(rel) = line.find(raw_ty) {
                                        let s_rel = rel + (raw_ty.len() - raw_ty.trim_start().len());
                                        let s = line_offset + s_rel;
                                        let e = s + ty_str.len();
                                        results.push((s, e, var_name));
                                    }
                            }
                    }
            }
            Language::Go => {
                if let Some(rest) = trimmed.strip_prefix("var ")
                    && let Some(eq) = rest.find('=') {
                        let before_eq = rest[..eq].trim();
                        let parts: Vec<&str> = before_eq.split_whitespace().collect();
                        if parts.len() >= 2 {
                            let var_name = parts[0].to_string();
                            let ty_str = parts[1..].join(" ");
                            if type_name(&ty_str) == old_ty_norm && contains_ident(&rest[eq + 1..], sym)
                                && let Some(var_pos) = line.find(&var_name) {
                                    let after_var = var_pos + var_name.len();
                                    if let Some(rel) = line[after_var..].find(&ty_str) {
                                        let s_rel = after_var + rel;
                                        let s = line_offset + s_rel;
                                        let e = s + ty_str.len();
                                        results.push((s, e, var_name));
                                    }
                                }
                        }
                    }
            }
            Language::Cpp | Language::C | Language::Java => {
                if let Some(eq) = trimmed.find('=') {
                    let before_eq = trimmed[..eq].trim();
                    let parts: Vec<&str> = before_eq.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let var_name = parts.last().unwrap().to_string();
                        let ty_str = parts[..parts.len() - 1].join(" ");
                        if type_name(&ty_str) == old_ty_norm && contains_ident(&trimmed[eq + 1..], sym)
                            && let Some(s_rel) = line.find(&ty_str) {
                                let s = line_offset + s_rel;
                                let e = s + ty_str.len();
                                results.push((s, e, var_name));
                            }
                    }
                }
            }
        }
    }

    results
}

struct EnclosingFn {
    name: String,
    return_type: String,
    ret_start: usize,
    ret_end: usize,
}

fn find_enclosing_fn(text: &str, at: usize, lang: Language) -> Option<EnclosingFn> {
    let prefix = &text[..at];
    match lang {
        Language::Rust => {
            let fn_idx = prefix.rfind("fn ")?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + 3 + header[3..].find('(')?;
            let close_p = matching(text, open_p)?;
            let name = text[fn_idx + 3..open_p].trim().to_string();
            let body_open = text[close_p..at].find('{')? + close_p;
            let arrow = text[close_p..body_open].find("->")? + close_p;
            let start = arrow + 2;
            let start = start + text[start..body_open].len() - text[start..body_open].trim_start().len();
            let end = body_open - (text[start..body_open].len() - text[start..body_open].trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn { name, return_type, ret_start: start, ret_end: end })
        }
        Language::Swift => {
            let fn_idx = prefix.rfind("func ")?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + 5 + header[5..].find('(')?;
            let close_p = matching(text, open_p)?;
            let name = text[fn_idx + 5..open_p].trim().to_string();
            let body_open = text[close_p..at].find('{')? + close_p;
            let arrow = text[close_p..body_open].find("->")? + close_p;
            let start = arrow + 2;
            let start = start + text[start..body_open].len() - text[start..body_open].trim_start().len();
            let end = body_open - (text[start..body_open].len() - text[start..body_open].trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn { name, return_type, ret_start: start, ret_end: end })
        }
        Language::Python => {
            let fn_idx = prefix.rfind("def ")?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + 4 + header[4..].find('(')?;
            let close_p = matching(text, open_p)?;
            let name = text[fn_idx + 4..open_p].trim().to_string();
            let colon = text[close_p..at].find(':')? + close_p;
            let between = &text[close_p..colon];
            let arrow = between.find("->")? + close_p;
            let start = arrow + 2;
            let start = start + text[start..].len() - text[start..].trim_start().len();
            let end = colon - (text[start..colon].len() - text[start..colon].trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn { name, return_type, ret_start: start, ret_end: end })
        }
        Language::TypeScript | Language::JavaScript => {
            let fn_idx = prefix.rfind("function ").or_else(|| prefix.rfind("const ")).or_else(|| prefix.rfind("let "))?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + header.find('(')?;
            let close_p = matching(text, open_p)?;
            let name_part = if prefix[fn_idx..].starts_with("function ") {
                &text[fn_idx + 9..open_p]
            } else {
                let eq = header.find('=')?;
                header[..eq].split_whitespace().last()?
            };
            let name = name_part.trim().to_string();
            let body_open = text[close_p..at].find('{')? + close_p;
            let colon = text[close_p..body_open].find(':')? + close_p;
            let start = colon + 1;
            let start = start + text[start..body_open].len() - text[start..body_open].trim_start().len();
            let end = body_open - (text[start..body_open].len() - text[start..body_open].trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn { name, return_type, ret_start: start, ret_end: end })
        }
        Language::Go => {
            let fn_idx = prefix.rfind("func ")?;
            let header = &text[fn_idx..at];
            let open_p = fn_idx + 5 + header[5..].find('(')?;
            let close_p = matching(text, open_p)?;
            let body_open = text[close_p..at].find('{')? + close_p;
            let between = &text[close_p + 1..body_open];
            let name = text[fn_idx + 5..open_p].trim().to_string();
            let start = close_p + 1 + (between.len() - between.trim_start().len());
            let end = body_open - (between.len() - between.trim_end().len());
            let return_type = text[start..end].trim().to_string();
            Some(EnclosingFn { name, return_type, ret_start: start, ret_end: end })
        }
        Language::Cpp | Language::C | Language::Java => {
            let body_open = prefix.rfind('{')?;
            let prev_close = text[..body_open].rfind(['}', ';']).map_or(0, |p| p + 1);
            let header = &text[prev_close..body_open];
            let open_p = prev_close + header.find('(')?;
            let seg_start = text[prev_close..open_p].rfind(['\n', ';', '}']).map_or(prev_close, |p| prev_close + p + 1);
            let before_paren = text[seg_start..open_p].trim();
            let words: Vec<&str> = before_paren.split_whitespace().collect();
            if words.len() < 2 {
                return None;
            }
            let name = words.last().unwrap().to_string();
            let ty_words = &words[..words.len() - 1];
            let ty_str = ty_words.join(" ");
            let start = text[seg_start..open_p].find(ty_words[0])? + seg_start;
            let last = ty_words.last().unwrap();
            let end_rel = text[start..open_p].rfind(last)? + last.len();
            Some(EnclosingFn { name, return_type: ty_str, ret_start: start, ret_end: start + end_rel })
        }
    }
}

fn find_matching_return(
    text: &str,
    sym: &str,
    old_ty: &str,
    lang: Language,
) -> Option<(usize, usize, String)> {
    let old_ty_norm = type_name(old_ty);

    for (idx, _) in text.match_indices(sym) {
        if !contains_ident(&text[idx..idx + sym.len()], sym) {
            continue;
        }
        let line_start = text[..idx].rfind('\n').map_or(0, |p| p + 1);
        let before = text[line_start..idx].trim();
        let line_end = text[idx..].find('\n').map_or(text.len(), |p| idx + p);
        let is_return = before.starts_with("return")
            || (lang == Language::Rust && {
                let after = text[idx + sym.len()..line_end].trim();
                after.is_empty() || after == "}"
            });
        if !is_return {
            continue;
        }

        if let Some(fn_decl) = find_enclosing_fn(text, idx, lang)
            && type_name(&fn_decl.return_type) == old_ty_norm {
                return Some((fn_decl.ret_start, fn_decl.ret_end, fn_decl.name));
            }
    }
    None
}

struct ParamInfo {
    name: String,
    ty: String,
    start: usize,
    end: usize,
}

fn extract_param_info(text: &str, fn_name: &str, arg_index: usize, lang: Language) -> Option<ParamInfo> {
    for (fn_idx, _) in text.match_indices(fn_name) {
        if !contains_ident(&text[fn_idx..fn_idx + fn_name.len()], fn_name) {
            continue;
        }
        let after_name = &text[fn_idx + fn_name.len()..];
        let trimmed = after_name.trim_start();
        if !trimmed.starts_with('(') {
            continue;
        }
        let open_p = fn_idx + fn_name.len() + (after_name.len() - trimmed.len());
        let close_p = matching(text, open_p)?;
        let params_text = &text[open_p + 1..close_p];
        let params: Vec<&str> = params_text.split(',').collect();
        if arg_index >= params.len() {
            return None;
        }
        let target_param = params[arg_index].trim();
        let param_offset = open_p + 1 + text[open_p + 1..close_p].find(target_param)?;
        let (s, e) = declared_type_span_polyglot(text, param_offset, lang)?;
        let name: String = target_param.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        return Some(ParamInfo {
            name,
            ty: text[s..e].trim().to_string(),
            start: s,
            end: e,
        });
    }
    None
}

fn find_matching_call_params(
    root: &Path,
    text: &str,
    sym: &str,
    old_ty: &str,
    lang: Language,
    rewritten: &BTreeMap<PathBuf, String>,
) -> Vec<(PathBuf, usize, usize, String, String)> {
    let mut results = Vec::new();
    let old_ty_norm = type_name(old_ty);

    for (idx, _) in text.match_indices(sym) {
        if !contains_ident(&text[idx..idx + sym.len()], sym) {
            continue;
        }
        let prefix = &text[..idx];
        let Some(open_p) = prefix.rfind('(') else { continue };
        let before_p = text[..open_p].trim_end();
        let callee_name: String = before_p
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if callee_name.is_empty() {
            continue;
        }

        let args_slice = &text[open_p + 1..idx];
        let arg_index = args_slice.split(',').count() - 1;

        let candidate_files = collect_candidate_files(root, rewritten, &callee_name);
        for cf in candidate_files {
            let cf_text = match rewritten.get(&cf) {
                Some(t) => t.clone(),
                None => match std::fs::read_to_string(&cf) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            let cf_lang = Language::of(&cf).unwrap_or(lang);
            if let Some(param_info) = extract_param_info(&cf_text, &callee_name, arg_index, cf_lang)
                && type_name(&param_info.ty) == old_ty_norm {
                    results.push((cf.clone(), param_info.start, param_info.end, param_info.name, callee_name.clone()));
                }
        }
    }
    results
}

fn find_matching_caller_vars(
    text: &str,
    fn_name: &str,
    old_ty: &str,
    lang: Language,
) -> Vec<(usize, usize, String)> {
    let mut results = Vec::new();

    for (idx, _) in text.match_indices(fn_name) {
        if !contains_ident(&text[idx..idx + fn_name.len()], fn_name) {
            continue;
        }
        let after = text[idx + fn_name.len()..].trim_start();
        if !after.starts_with('(') {
            continue;
        }
        let line_start = text[..idx].rfind('\n').map_or(0, |p| p + 1);
        let line_end = text[idx..].find('\n').map_or(text.len(), |p| idx + p);
        let line = &text[line_start..line_end];
        let vars = find_matching_vars(line, fn_name, old_ty, lang);
        for (s_rel, e_rel, var_name) in vars {
            results.push((line_start + s_rel, line_start + e_rel, var_name));
        }
    }
    results
}

fn collect_candidate_files(
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    name: &str,
) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    for p in rewritten.keys() {
        files.insert(p.clone());
    }
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let p = entry.path();
        if p.is_file() {
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            if matches!(ext, "rs" | "ts" | "js" | "tsx" | "jsx" | "py" | "go" | "cpp" | "c" | "h" | "hpp" | "swift")
                && let Ok(content) = std::fs::read_to_string(p)
                && content.contains(name)
            {
                files.insert(p.to_path_buf());
            }
        }
    }
    files.into_iter().collect()
}

#[allow(clippy::too_many_arguments)]
fn sync_cpp_headers(
    root: &Path,
    source_file: &Path,
    fn_name: &str,
    was: &str,
    to: &str,
    rewritten: &mut BTreeMap<PathBuf, String>,
    also: &mut Vec<PathBuf>,
    _is_return: bool,
) {
    let header_candidates = [
        source_file.with_extension("h"),
        source_file.with_extension("hpp"),
    ];
    for h in &header_candidates {
        let h_path = if h.exists() {
            Some(h.clone())
        } else {
            let stem = source_file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let inc_h = root.join("include").join(format!("{stem}.h"));
            if inc_h.exists() {
                Some(inc_h)
            } else {
                None
            }
        };
        if let Some(h_file) = h_path {
            let h_text = match rewritten.get(&h_file) {
                Some(t) => t.clone(),
                None => match std::fs::read_to_string(&h_file) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            for (idx, _) in h_text.match_indices(fn_name) {
                if let Some((start, end)) = declared_type_span_polyglot(&h_text, idx, Language::Cpp)
                    && h_text[start..end].trim() == was.trim() {
                        let mut updated_h = h_text.clone();
                        updated_h.replace_range(start..end, to);
                        rewritten.insert(h_file.clone(), updated_h);
                        if !also.contains(&h_file) {
                            also.push(h_file);
                        }
                        break;
                    }
            }
        }
    }
}

fn is_import_line(content: &str, at: usize, lang: Language) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let line_end = content[at..].find('\n').map_or(content.len(), |p| at + p);
    let line = content[line_start..line_end].trim();
    match lang {
        Language::Python => line.starts_with("import ") || line.starts_with("from "),
        Language::TypeScript | Language::JavaScript => {
            line.starts_with("import ") || line.starts_with("import{") || line.contains(" from ") || line.contains("require(")
        }
        Language::Go => line.starts_with("import ") || line.starts_with("import ("),
        Language::Cpp | Language::C => line.starts_with("#include") || line.starts_with("using "),
        Language::Swift => line.starts_with("import "),
        Language::Rust => {
            let without_pub = line.strip_prefix("pub ").or_else(|| line.strip_prefix("pub(crate) ")).unwrap_or(line);
            without_pub.starts_with("use ")
        }
        Language::Java => line.starts_with("import ") || line.starts_with("package "),
    }
}

pub fn find_symbol_decl_offset(
    text: &str,
    clean_name: &str,
    lang: Language,
    prefer_line: Option<u32>,
) -> Option<usize> {
    if let Some(l) = prefer_line {
        let lines: Vec<&str> = text.lines().collect();
        if l > 0 && (l as usize) <= lines.len() {
            let target_idx = (l - 1) as usize;
            let start_idx = target_idx.saturating_sub(2);
            let end_idx = (target_idx + 2).min(lines.len().saturating_sub(1));
            for i in start_idx..=end_idx {
                let line_str = lines[i];
                if let Some(pos) = line_str.find(clean_name) {
                    let line_start = text.lines().take(i).map(|l| l.len() + 1).sum::<usize>();
                    let abs_offset = line_start + pos;
                    if declared_type_span_polyglot(text, abs_offset, lang).is_some() {
                        return Some(abs_offset);
                    }
                }
            }
        }
    }

    for (idx, _) in text.match_indices(clean_name) {
        if idx > 0 && text[..idx].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let after = &text[idx + clean_name.len()..];
        if after.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        if crate::inline_parameter::is_in_comment(text, idx, lang) {
            continue;
        }
        if is_import_line(text, idx, lang) {
            continue;
        }
        if declared_type_span_polyglot(text, idx, lang).is_some() {
            return Some(idx);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span_of(text: &str, name: &str) -> String {
        let at = text.find(name).expect("the name is in the text");
        let (s, e) = declared_type_span(text, at).expect("a declared type");
        text[s..e].to_string()
    }

    #[test]
    fn a_declared_type_is_found_in_every_shape_that_can_have_one() {
        assert_eq!(
            span_of("    pub timeout_secs: u64,\n", "timeout_secs"),
            "u64"
        );
        assert_eq!(
            span_of("    pub map: BTreeMap<String, Vec<u8>>,\n", "map"),
            "BTreeMap<String, Vec<u8>>"
        );
        assert_eq!(span_of("fn f(a: &str, b: u32) {}", "b"), "u32");
        assert_eq!(
            span_of("fn compute(a: &str) -> Result<u32, Error> {\n", "compute"),
            "Result<u32, Error>"
        );
        assert_eq!(span_of("    let x: Vec<u8> = go();\n", "x"), "Vec<u8>");
        // The last field of a struct, written without a trailing comma.
        assert_eq!(span_of("    pub n: usize\n}\n", "n"), "usize");
    }

    #[test]
    fn a_function_without_a_return_type_has_nothing_to_migrate() {
        let text = "fn compute(a: u8) {\n    a;\n}\n";
        let at = text.find("compute").expect("the name");
        assert_eq!(declared_type_span(text, at), None);
    }

    #[test]
    fn a_suggestion_is_made_only_when_the_error_names_both_types() {
        assert_eq!(
            suggest("expected u64, found u32", "u32", "u64").as_deref(),
            Some("the value here is still `u32`; convert it to `u64`")
        );
        assert!(
            suggest("expected u32, found u64", "u32", "u64")
                .is_some_and(|s| s.contains("still wants"))
        );
        // Neither type is ours: no guess.
        assert_eq!(suggest("expected String, found &str", "u32", "u64"), None);
        // The declaration spells a path; the analyzer spells the name that is in scope.
        assert_eq!(
            suggest("expected Duration, found u64", "u64", "std::time::Duration").as_deref(),
            Some("the value here is still `u64`; convert it to `Duration`")
        );
        assert_eq!(
            suggest("cannot find value `n` in this scope", "u32", "u64"),
            None
        );
    }

    #[test]
    fn the_report_calls_the_sites_work_rather_than_failure() {
        let migration = Migration {
            symbol: "timeout_secs".into(),
            root: PathBuf::from("/root"),
            file: "src/lib.rs".into(),
            was: "u64".into(),
            now: "std::time::Duration".into(),
            rewritten: Vec::new(),
            sites: vec![Site {
                file: "src/other.rs".into(),
                line: 12,
                col: 5,
                message: "expected Duration, found u64".into(),
                code: Some("E0308".into()),
                source: "cfg.timeout_secs = 30;".into(),
                suggestion: Some("the value here is still `u64`".into()),
                end: None,
            }],
            in_attributes: 0,
            converted: Vec::new(),
            conversion_note: None,
            transitive_count: 0,
            transitively_migrated: Vec::new(),
            applied: false,
        };
        let text = migration.render(50);
        assert!(text.contains("1 site(s) in 1 file(s)"), "{text}");
        assert!(text.contains("src/other.rs"), "{text}");
        assert!(text.contains("cfg.timeout_secs = 30;"), "{text}");
        assert!(
            text.contains("try: the value here is still `u64`"),
            "{text}"
        );
        assert!(text.contains("they are the migration"), "{text}");
        assert!(text.contains("nothing was written"), "{text}");

        let clean = Migration {
            sites: Vec::new(),
            applied: true,
            ..migration
        };
        let text = clean.render(50);
        assert!(text.contains("nothing else has to change"), "{text}");
        assert!(text.contains("the declaration was written"), "{text}");
    }

    #[test]
    fn a_long_list_is_cut_at_the_budget_and_says_how_many_are_left() {
        let site = |line: u32| Site {
            file: "src/lib.rs".into(),
            line,
            col: 1,
            message: "expected A, found B".into(),
            code: None,
            source: "x".into(),
            suggestion: None,
            end: None,
        };
        let migration = Migration {
            symbol: "f".into(),
            root: PathBuf::from("/root"),
            file: "src/lib.rs".into(),
            was: "A".into(),
            now: "B".into(),
            rewritten: Vec::new(),
            sites: (1..=10).map(site).collect(),
            in_attributes: 0,
            converted: Vec::new(),
            conversion_note: None,
            transitive_count: 0,
            transitively_migrated: Vec::new(),
            applied: false,
        };
        let text = migration.render(3);
        assert!(text.contains("… 7 more site(s)"), "{text}");
    }

    #[test]
    fn into_goes_on_a_path_or_call_and_around_anything_else() {
        assert_eq!(into_call("secs"), "secs.into()");
        assert_eq!(into_call("l.timeout"), "l.timeout.into()");
        assert_eq!(
            into_call("build(1, \"x\").timeout"),
            "build(1, \"x\").timeout.into()"
        );
        assert_eq!(into_call("secs as u32"), "(secs as u32).into()");
        assert_eq!(into_call("l.timeout * 2"), "(l.timeout * 2).into()");
        assert_eq!(into_call("&name"), "(&name).into()");
        assert_eq!(into_call("-1"), "(-1).into()");
    }

    #[test]
    fn a_candidate_is_the_two_types_meeting_on_one_line() {
        let site = |message: &str, code: &str, end: Option<(u32, u32)>| Site {
            file: "src/lib.rs".into(),
            line: 4,
            col: 5,
            message: message.into(),
            code: Some(code.into()),
            source: String::new(),
            suggestion: None,
            end,
        };
        let one_line = Some((4, 9));
        assert!(is_candidate(
            &site("expected u64, found u32", "E0308", one_line),
            "u32",
            "u64"
        ));
        assert!(is_candidate(
            &site("expected u32, found u64", "E0308", one_line),
            "u32",
            "u64"
        ));
        assert!(!is_candidate(
            &site("expected u64, found u16", "E0308", one_line),
            "u32",
            "u64"
        ));
        assert!(!is_candidate(
            &site("expected u64, found u32", "E0277", one_line),
            "u32",
            "u64"
        ));
        assert!(!is_candidate(
            &site("expected u64, found u32", "E0308", Some((6, 2))),
            "u32",
            "u64"
        ));
        assert!(!is_candidate(
            &site("expected u64, found u32", "E0308", None),
            "u32",
            "u64"
        ));
    }

    #[test]
    fn a_converted_migration_lists_what_it_wrote() {
        let migration = Migration {
            symbol: "timeout".into(),
            root: PathBuf::from("/root"),
            file: "src/lib.rs".into(),
            was: "u32".into(),
            now: "u64".into(),
            rewritten: Vec::new(),
            sites: Vec::new(),
            in_attributes: 0,
            converted: vec![Conversion {
                file: "src/lib.rs".into(),
                line: 9,
                was: "secs".into(),
                now: "secs.into()".into(),
            }],
            conversion_note: Some("a note".into()),
            transitive_count: 0,
            transitively_migrated: Vec::new(),
            applied: true,
        };
        let text = migration.render(10);
        assert!(
            text.contains("1 site(s) converted with `.into()`"),
            "{text}"
        );
        assert!(
            text.contains("src/lib.rs:9  `secs` → `secs.into()`"),
            "{text}"
        );
        assert!(text.contains("a note"), "{text}");
        assert!(
            text.contains("the declaration and 1 conversion(s) were written"),
            "{text}"
        );
    }

    #[test]
    fn a_type_is_named_the_way_the_analyzer_names_it() {
        assert_eq!(type_name("std::time::Duration"), "Duration");
        assert_eq!(type_name("Vec<std::string::String>"), "Vec<String>");
        assert_eq!(type_name("Box<str, Global>"), "Box<str>");
        assert_eq!(type_name("std::boxed::Box<str>"), "Box<str>");
        assert_eq!(
            type_name("HashMap<u32, Vec<u8, Global>>"),
            "HashMap<u32, Vec<u8>>"
        );
    }

    #[test]
    fn a_method_name_range_takes_its_arguments_and_a_chain_is_not_cut() {
        let site = |line: u32, col: u32, end_col: u32| Site {
            file: "src/lib.rs".into(),
            line,
            col,
            message: String::new(),
            code: None,
            source: String::new(),
            suggestion: None,
            end: Some((line, end_col)),
        };
        let text = "    name: label.to_string(),\n    x: a.b + c,\n";
        let (s, e) = expression_span(text, &site(1, 17, 26)).expect("a span");
        assert_eq!(&text[s..e], "to_string()");
        assert!(expression_span(text, &site(2, 10, 15)).is_none());
        let (s, e) = expression_span(text, &site(2, 8, 11)).expect("a span");
        assert_eq!(&text[s..e], "a.b");
    }

    #[test]
    fn polyglot_declared_type_span_found_in_all_languages() {
        // TypeScript
        let ts_text = "function compute(val: number): number {\n  const doubled: number = val * 2;\n  return doubled;\n}";
        let val_at = ts_text.find("val").unwrap();
        let (s, e) = declared_type_span_polyglot(ts_text, val_at, Language::TypeScript).expect("ts param");
        assert_eq!(&ts_text[s..e], "number");

        let fn_at = ts_text.find("compute").unwrap();
        let (s, e) = declared_type_span_polyglot(ts_text, fn_at, Language::TypeScript).expect("ts ret");
        assert_eq!(&ts_text[s..e], "number");

        let d_at = ts_text.find("doubled").unwrap();
        let (s, e) = declared_type_span_polyglot(ts_text, d_at, Language::TypeScript).expect("ts var");
        assert_eq!(&ts_text[s..e], "number");

        // Python
        let py_text = "def compute(val: int) -> int:\n    doubled: int = val * 2\n    return doubled\n";
        let val_at = py_text.find("val").unwrap();
        let (s, e) = declared_type_span_polyglot(py_text, val_at, Language::Python).expect("py param");
        assert_eq!(&py_text[s..e], "int");

        let fn_at = py_text.find("compute").unwrap();
        let (s, e) = declared_type_span_polyglot(py_text, fn_at, Language::Python).expect("py ret");
        assert_eq!(&py_text[s..e], "int");

        let d_at = py_text.find("doubled").unwrap();
        let (s, e) = declared_type_span_polyglot(py_text, d_at, Language::Python).expect("py var");
        assert_eq!(&py_text[s..e], "int");

        // Go
        let go_text = "func compute(val int32) int32 {\n    var doubled int32 = val * 2\n    return doubled\n}\n";
        let val_at = go_text.find("val").unwrap();
        let (s, e) = declared_type_span_polyglot(go_text, val_at, Language::Go).expect("go param");
        assert_eq!(&go_text[s..e], "int32");

        let fn_at = go_text.find("compute").unwrap();
        let (s, e) = declared_type_span_polyglot(go_text, fn_at, Language::Go).expect("go ret");
        assert_eq!(&go_text[s..e], "int32");

        let d_at = go_text.find("doubled").unwrap();
        let (s, e) = declared_type_span_polyglot(go_text, d_at, Language::Go).expect("go var");
        assert_eq!(&go_text[s..e], "int32");

        // Swift
        let sw_text = "func compute(val: Int) -> Int {\n    let doubled: Int = val * 2\n    return doubled\n}\n";
        let val_at = sw_text.find("val").unwrap();
        let (s, e) = declared_type_span_polyglot(sw_text, val_at, Language::Swift).expect("swift param");
        assert_eq!(&sw_text[s..e], "Int");

        let fn_at = sw_text.find("compute").unwrap();
        let (s, e) = declared_type_span_polyglot(sw_text, fn_at, Language::Swift).expect("swift ret");
        assert_eq!(&sw_text[s..e], "Int");

        let d_at = sw_text.find("doubled").unwrap();
        let (s, e) = declared_type_span_polyglot(sw_text, d_at, Language::Swift).expect("swift var");
        assert_eq!(&sw_text[s..e], "Int");

        // C++
        let cpp_text = "int compute(int val) {\n    int doubled = val * 2;\n    return doubled;\n}\n";
        let val_at = cpp_text.find("val").unwrap();
        let (s, e) = declared_type_span_polyglot(cpp_text, val_at, Language::Cpp).expect("cpp param");
        assert_eq!(&cpp_text[s..e], "int");

        let fn_at = cpp_text.find("compute").unwrap();
        let (s, e) = declared_type_span_polyglot(cpp_text, fn_at, Language::Cpp).expect("cpp ret");
        assert_eq!(&cpp_text[s..e], "int");

        let d_at = cpp_text.find("doubled").unwrap();
        let (s, e) = declared_type_span_polyglot(cpp_text, d_at, Language::Cpp).expect("cpp var");
        assert_eq!(&cpp_text[s..e], "int");
    }

    #[test]
    fn language_conversion_syntax_across_polyglot() {
        assert_eq!(language_conversion("x", "u64", Language::Rust), "x.into()");
        assert_eq!(language_conversion("x", "number", Language::TypeScript), "Number(x)");
        assert_eq!(language_conversion("x", "MyType", Language::TypeScript), "MyType(x)");
        assert_eq!(language_conversion("x + 1", "MyType", Language::TypeScript), "(x + 1 as MyType)");
        assert_eq!(language_conversion("x", "int", Language::Python), "int(x)");
        assert_eq!(language_conversion("x", "int64", Language::Go), "int64(x)");
        assert_eq!(language_conversion("x", "Int64", Language::Swift), "Int64(x)");
        assert_eq!(language_conversion("x", "int64_t", Language::Cpp), "static_cast<int64_t>(x)");
    }

    #[test]
    fn parse_mismatch_polyglot_messages() {
        assert_eq!(
            parse_mismatch("Type 'string' is not assignable to type 'number'"),
            Some(("number".to_string(), "string".to_string()))
        );
        assert_eq!(
            parse_mismatch("Expression of type \"str\" cannot be assigned to declared type \"int\""),
            Some(("int".to_string(), "str".to_string()))
        );
        assert_eq!(
            parse_mismatch("cannot use x (variable of type int32) as int64 value"),
            Some(("int64".to_string(), "int32".to_string()))
        );
        assert_eq!(
            parse_mismatch("cannot convert value of type 'Int' to specified type 'Int64'"),
            Some(("Int64".to_string(), "Int".to_string()))
        );
        assert_eq!(
            parse_mismatch("no viable conversion from 'int' to 'double'"),
            Some(("double".to_string(), "int".to_string()))
        );
    }
}
