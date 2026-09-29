//! Replace constructor / raw struct instantiations with named static factory methods or a fluent builder pattern (Roadmap 7.1.3).
//!
//! Rust has no class constructors, but factory methods (`Type::new(...)`) and fluent builder
//! patterns (`Type::builder().field(val)...build()`) are standard idioms. This module generates
//! idiomatic factory methods or fluent builders and rewrites raw struct/class instantiations
//! across the workspace.
//!
//! Polyglot support covers:
//! - Rust: struct declarations, `impl` generation, builder struct generation with fluent setters,
//!   and struct literal rewrites (`Type { f: v }` -> `Type::new(...)` / `Type::builder()...build()`),
//!   mapping reordered fields, shorthand syntax, and reporting unsupported struct update syntax (`..rest`).
//! - Go: `type T struct`, `func NewT(...) *T` or `TBuilder`, rewriting `&T{...}` and `T{...}`.
//! - TypeScript / JavaScript: classes and interfaces, static factory `create` or `TBuilder`, rewriting `new T(...)`.
//! - Python: `@classmethod def create(cls, ...)` or `TBuilder`, rewriting `T(...)`.
//! - C++: `struct T` / `class T`, static `create` or `TBuilder`, rewriting `T{...}`.
//! - Swift: `struct T` / `class T`, static `create` or `TBuilder`, rewriting `T(...)`.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Refactoring target mode: static factory method or fluent builder pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceMode {
    Factory,
    Builder,
}

impl std::fmt::Display for ReplaceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Factory => write!(f, "factory"),
            Self::Builder => write!(f, "builder"),
        }
    }
}

/// A parsed field of a struct, class, or interface.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FieldDecl {
    pub name: String,
    pub ty: String,
    pub vis: String,
}

/// A parsed struct or class declaration.
#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub language: String,
    pub fields: Vec<FieldDecl>,
    pub generics: Option<String>,
    pub is_pub: bool,
    pub decl_start: usize,
    pub decl_end: usize,
    pub line: u32,
    pub col: u32,
}

/// Result of replacing constructors with factory methods or builders.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReplaceConstructorResult {
    pub type_name: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub mode: ReplaceMode,
    pub target_name: String,
    pub declared_fields: Vec<String>,
    pub instantiations_rewritten: usize,
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
    pub language: String,
}

impl ReplaceConstructorResult {
    /// Render a human-readable report of the refactoring outcome.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({}) — replaced constructor with {}\n\n",
            self.type_name, self.file, self.mode
        );
        out.push_str(&format!(
            "- target: `{}` ({})\n",
            self.target_name, self.mode
        ));
        out.push_str(&format!(
            "- declared fields ({}): {}\n",
            self.declared_fields.len(),
            if self.declared_fields.is_empty() {
                "none".to_string()
            } else {
                self.declared_fields.join(", ")
            }
        ));
        out.push_str(&format!(
            "- {} instantiation(s) rewritten across {} file(s)\n\n",
            self.instantiations_rewritten,
            self.rewritten.len()
        ));

        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = crate::refactor::text_before_apply(Path::new(path));
            let rel = display(&self.root, Path::new(path));
            let diff = similar::TextDiff::from_lines(&old_text, new_text);
            changed_lines += diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            body.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        out.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            changed_lines,
            self.rewritten.len()
        ));
        if body.len() > diff_budget {
            let cut: String = body.chars().take(diff_budget).collect();
            out.push_str(&cut);
            out.push_str("\n… diff truncated\n");
        } else {
            out.push_str(&body);
        }

        if !self.blocked.is_empty() {
            out.push_str(&format!(
                "\n{} instantiation(s) could not be safely rewritten:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s)):\n",
                self.unmatched.len()
            ));
            for u in &self.unmatched {
                out.push_str(&format!("  {u}\n"));
            }
        }

        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }

        if self.applied {
            out.push_str(&format!(
                "\n[applied to {} file(s)]\n",
                self.rewritten.len()
            ));
        } else {
            out.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        out
    }
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Extracts generic parameters `<T, U>` starting at or after `from`.
pub fn extract_generics(text: &str, from: usize) -> Option<(String, usize)> {
    let rest = text[from..].trim_start();
    if !rest.starts_with('<') {
        return None;
    }
    let offset = text[from..].len() - rest.len();
    let open = from + offset;
    let mut depth = 0i32;
    let mut end = open;
    for (i, c) in text[open..].char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth == 0 {
        Some((text[open..end].to_string(), end))
    } else {
        None
    }
}

/// Splits comma-separated items while respecting brackets and string literals.
pub fn split_balanced_commas(inner: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = inner.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' | b'(' | b'[' | b'{' => depth += 1,
            b'>' | b')' | b']' | b'}' => depth = (depth - 1).max(0),
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b',' if depth == 0 => {
                let chunk = inner[start..i].trim();
                if !chunk.is_empty() {
                    items.push(chunk.to_string());
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = inner[start..].trim();
    if !tail.is_empty() {
        items.push(tail.to_string());
    }
    items
}

/// Parses Rust struct field declarations inside the struct body `{ ... }`.
pub fn parse_rust_struct_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    let chunks = split_balanced_commas(inner);
    for raw in chunks {
        let lines: Vec<&str> = raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("#["))
            .collect();
        let cleaned = lines.join(" ");
        let Some((left, right)) = cleaned.split_once(':') else {
            continue;
        };
        let ty = right.trim().to_string();
        let mut words = left.split_whitespace();
        let mut vis = String::new();
        let mut name = String::new();
        for word in words.by_ref() {
            if word.starts_with("pub") {
                vis = word.to_string();
            } else if is_ident_str(word) {
                name = word.to_string();
            }
        }
        if !name.is_empty() && !ty.is_empty() {
            fields.push(FieldDecl { name, ty, vis });
        }
    }
    fields
}

/// Parses Go struct field declarations inside `{ ... }`.
pub fn parse_go_struct_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        let stripped = trimmed.split("//").next().unwrap_or(trimmed).trim();
        let tokens: Vec<&str> = stripped.split_whitespace().collect();
        if tokens.len() >= 2 {
            let name = tokens[0].to_string();
            let ty = tokens[1].to_string();
            if is_ident_str(&name) {
                fields.push(FieldDecl {
                    name,
                    ty,
                    vis: String::new(),
                });
            }
        }
    }
    fields
}

/// Parses TypeScript / JavaScript class/interface fields.
pub fn parse_ts_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    if let Some(pos) = inner.find("constructor")
        && let Some(open) = inner[pos..].find('(')
        && let Some(close) = inner[pos + open..].find(')')
    {
        let params = &inner[pos + open + 1..pos + open + close];
        for p in split_balanced_commas(params) {
            let clean = p
                .trim_start_matches("public ")
                .trim_start_matches("private ")
                .trim_start_matches("protected ")
                .trim_start_matches("readonly ")
                .trim();
            if let Some((n, t)) = clean.split_once(':') {
                let name = n.trim().to_string();
                if is_ident_str(&name) && !fields.iter().any(|f: &FieldDecl| f.name == name) {
                    fields.push(FieldDecl {
                        name,
                        ty: t.trim().to_string(),
                        vis: String::new(),
                    });
                }
            }
        }
    }
    if !fields.is_empty() {
        return fields;
    }
    for line in inner.lines() {
        let trimmed = line.trim().trim_end_matches(';');
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with("this.")
            || trimmed.starts_with("return")
            || trimmed.contains('(')
        {
            continue;
        }
        if let Some((left, right)) = trimmed.split_once(':') {
            let left = left
                .trim_start_matches("public ")
                .trim_start_matches("private ")
                .trim_start_matches("protected ")
                .trim_start_matches("readonly ")
                .trim();
            if is_ident_str(left) && !fields.iter().any(|f: &FieldDecl| f.name == left) {
                fields.push(FieldDecl {
                    name: left.to_string(),
                    ty: right.trim().to_string(),
                    vis: String::new(),
                });
            }
        }
    }
    fields
}

/// Parses Python class fields from `def __init__` or dataclass annotations.
pub fn parse_python_fields(text: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    if let Some(init_pos) = text.find("def __init__")
        && let Some(open) = text[init_pos..].find('(')
        && let Some(close) = text[init_pos + open..].find(')')
    {
        let params = &text[init_pos + open + 1..init_pos + open + close];
            for p in split_balanced_commas(params) {
                let p = p.trim();
                if p == "self" || p.is_empty() {
                    continue;
                }
                if let Some((n, t)) = p.split_once(':') {
                    let n = n.trim().split('=').next().unwrap_or(n).trim();
                    let t = t.trim().split('=').next().unwrap_or(t).trim();
                    fields.push(FieldDecl {
                        name: n.to_string(),
                        ty: t.to_string(),
                        vis: String::new(),
                    });
                } else {
                    let n = p.split('=').next().unwrap_or(p).trim();
                    fields.push(FieldDecl {
                        name: n.to_string(),
                        ty: "Any".to_string(),
                        vis: String::new(),
                    });
                }
            }
    }
    if fields.is_empty() {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("def ") || trimmed.starts_with('@') || trimmed.is_empty() {
                continue;
            }
            if let Some((n, t)) = trimmed.split_once(':') {
                let n = n.trim();
                let t = t.split('=').next().unwrap_or(t).trim();
                if is_ident_str(n) {
                    fields.push(FieldDecl {
                        name: n.to_string(),
                        ty: t.to_string(),
                        vis: String::new(),
                    });
                }
            }
        }
    }
    fields
}

/// Parses C++ struct/class fields inside `{ ... }`.
pub fn parse_cpp_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim().trim_end_matches(';');
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.contains('(') {
            continue;
        }
        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.len() >= 2 {
            let name = tokens.last().unwrap().trim_matches(['*', '&']);
            let ty = tokens[..tokens.len() - 1].join(" ");
            if is_ident_str(name) {
                fields.push(FieldDecl {
                    name: name.to_string(),
                    ty,
                    vis: String::new(),
                });
            }
        }
    }
    fields
}

/// Parses Swift struct/class fields inside `{ ... }`.
pub fn parse_swift_fields(inner: &str) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("var ") || trimmed.starts_with("let ") {
            let decl = &trimmed[4..];
            if let Some((n, t)) = decl.split_once(':') {
                let n = n.trim();
                let t = t.split('=').next().unwrap_or(t).trim();
                if is_ident_str(n) {
                    fields.push(FieldDecl {
                        name: n.to_string(),
                        ty: t.to_string(),
                        vis: String::new(),
                    });
                }
            }
        }
    }
    fields
}

fn is_ident_str(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && s.chars().all(is_ident)
}

/// Parses the declaration of `type_name` in `text` given its language.
pub fn parse_struct_declaration(
    text: &str,
    type_name: &str,
    language: &str,
) -> Result<StructDecl> {
    match language {
        "rust" => parse_rust_struct_decl(text, type_name),
        "go" => parse_go_struct_decl(text, type_name),
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => {
            parse_ts_struct_decl(text, type_name, language)
        }
        "python" => parse_python_struct_decl(text, type_name),
        "cpp" | "c" => parse_cpp_struct_decl(text, type_name, language),
        "swift" => parse_swift_struct_decl(text, type_name),
        _ => anyhow::bail!("unsupported language `{language}` for replace_constructor"),
    }
}

pub fn parse_rust_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
    let mut candidate = None;
    for (at, _) in text.match_indices(type_name) {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let before = text[..at].trim_end();
        let is_struct = before.ends_with("struct")
            || before.ends_with("struct ")
            || before.contains("struct ") && before[before.rfind("struct ").unwrap()..].chars().all(|c| c.is_whitespace() || is_ident(c) || c == '(' || c == ')');
        if !is_struct {
            continue;
        }
        let struct_kw = before.rfind("struct").unwrap_or(at);
        let line_start = text[..struct_kw].rfind('\n').map_or(0, |i| i + 1);
        let is_pub = text[line_start..struct_kw].contains("pub");
        let generics = extract_generics(text, at + type_name.len());
        let open_from = generics.as_ref().map_or(at + type_name.len(), |(_, end)| *end);
        let Some(open_rel) = text[open_from..].find('{') else {
            continue;
        };
        let open = open_from + open_rel;
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let fields = parse_rust_struct_fields(&text[open + 1..close]);
        let (line, col) = crate::signature::position_at(text, at)?;
        candidate = Some(StructDecl {
            name: type_name.to_string(),
            language: "rust".to_string(),
            fields,
            generics: generics.map(|(g, _)| g),
            is_pub,
            decl_start: struct_kw,
            decl_end: close + 1,
            line,
            col,
        });
        break;
    }
    candidate.with_context(|| format!("cannot find declaration of struct `{type_name}` in file"))
}

fn parse_go_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
    for (at, _) in text.match_indices(type_name) {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let after = &text[at + type_name.len()..];
        if let Some(open) = after.find('{') {
            let head = &after[..open];
            if head.contains("struct") {
                let open_idx = at + type_name.len() + open;
                if let Some(close) = crate::parameter_object::matching_bracket(text, open_idx) {
                    let fields = parse_go_struct_fields(&text[open_idx + 1..close]);
                    let (line, col) = crate::signature::position_at(text, at)?;
                    let is_pub = type_name.chars().next().is_some_and(|c| c.is_uppercase());
                    return Ok(StructDecl {
                        name: type_name.to_string(),
                        language: "go".to_string(),
                        fields,
                        generics: None,
                        is_pub,
                        decl_start: at,
                        decl_end: close + 1,
                        line,
                        col,
                    });
                }
            }
        }
    }
    anyhow::bail!("cannot find declaration of struct `{type_name}` in Go file")
}

fn parse_ts_struct_decl(text: &str, type_name: &str, language: &str) -> Result<StructDecl> {
    for (at, _) in text.match_indices(type_name) {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let before = text[..at].trim_end();
        let is_target = before.ends_with("class") || before.ends_with("interface");
        if !is_target {
            continue;
        }
        let Some(open_rel) = text[at + type_name.len()..].find('{') else {
            continue;
        };
        let open = at + type_name.len() + open_rel;
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let fields = parse_ts_fields(&text[open + 1..close]);
        let (line, col) = crate::signature::position_at(text, at)?;
        return Ok(StructDecl {
            name: type_name.to_string(),
            language: language.to_string(),
            fields,
            generics: None,
            is_pub: before.contains("export"),
            decl_start: at,
            decl_end: close + 1,
            line,
            col,
        });
    }
    anyhow::bail!("cannot find declaration of class/interface `{type_name}` in TypeScript/JavaScript file")
}

fn parse_python_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
    for (at, _) in text.match_indices(type_name) {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let before = text[..at].trim_end();
        if before.ends_with("class") {
            let class_start = text[..at].rfind("class").unwrap_or(at);
            let fields = parse_python_fields(&text[at..]);
            let (line, col) = crate::signature::position_at(text, at)?;
            return Ok(StructDecl {
                name: type_name.to_string(),
                language: "python".to_string(),
                fields,
                generics: None,
                is_pub: !type_name.starts_with('_'),
                decl_start: class_start,
                decl_end: text.len(),
                line,
                col,
            });
        }
    }
    anyhow::bail!("cannot find declaration of class `{type_name}` in Python file")
}

fn parse_cpp_struct_decl(text: &str, type_name: &str, language: &str) -> Result<StructDecl> {
    for (at, _) in text.match_indices(type_name) {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let before = text[..at].trim_end();
        if before.ends_with("struct") || before.ends_with("class") {
            let Some(open_rel) = text[at + type_name.len()..].find('{') else {
                continue;
            };
            let open = at + type_name.len() + open_rel;
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            let fields = parse_cpp_fields(&text[open + 1..close]);
            let (line, col) = crate::signature::position_at(text, at)?;
            return Ok(StructDecl {
                name: type_name.to_string(),
                language: language.to_string(),
                fields,
                generics: None,
                is_pub: true,
                decl_start: at,
                decl_end: close + 1,
                line,
                col,
            });
        }
    }
    anyhow::bail!("cannot find declaration of struct/class `{type_name}` in C++ file")
}

fn parse_swift_struct_decl(text: &str, type_name: &str) -> Result<StructDecl> {
    for (at, _) in text.match_indices(type_name) {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let before = text[..at].trim_end();
        if before.ends_with("struct") || before.ends_with("class") {
            let Some(open_rel) = text[at + type_name.len()..].find('{') else {
                continue;
            };
            let open = at + type_name.len() + open_rel;
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            let fields = parse_swift_fields(&text[open + 1..close]);
            let (line, col) = crate::signature::position_at(text, at)?;
            return Ok(StructDecl {
                name: type_name.to_string(),
                language: "swift".to_string(),
                fields,
                generics: None,
                is_pub: before.contains("public") || before.contains("open"),
                decl_start: at,
                decl_end: close + 1,
                line,
                col,
            });
        }
    }
    anyhow::bail!("cannot find declaration of struct/class `{type_name}` in Swift file")
}

/// Generates the factory code to add to the target file.
pub fn generate_factory_code(decl: &StructDecl, factory_name: &str) -> String {
    let name = &decl.name;
    match decl.language.as_str() {
        "rust" => {
            let vis = if decl.is_pub { "pub " } else { "" };
            let generics_header = decl
                .generics
                .as_deref()
                .map(|g| format!("{g} "))
                .unwrap_or_default();
            let generics_name = decl.generics.as_deref().unwrap_or_default();
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let field_inits = decl
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
                .join(",\n            ");
            format!(
                "\n\nimpl {generics_header}{name}{generics_name} {{\n    {vis}fn {factory_name}({params}) -> Self {{\n        Self {{\n            {field_inits},\n        }}\n    }}\n}}"
            )
        }
        "go" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{} {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let field_inits = decl
                .fields
                .iter()
                .map(|f| format!("{}: {},", f.name, f.name))
                .collect::<Vec<_>>()
                .join("\n        ");
            format!(
                "\n\nfunc {factory_name}({params}) *{name} {{\n    return &{name}{{\n        {field_inits}\n    }}\n}}"
            )
        }
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let args = decl
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n    static {factory_name}({params}): {name} {{\n        return new {name}({args});\n    }}\n"
            )
        }
        "python" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let args = decl
                .fields
                .iter()
                .map(|f| format!("{}={}", f.name, f.name))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n    @classmethod\n    def {factory_name}(cls, {params}) -> \"{name}\":\n        return cls({args})\n"
            )
        }
        "cpp" | "c" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{} {}", f.ty, f.name))
                .collect::<Vec<_>>()
                .join(", ");
            let args = decl
                .fields
                .iter()
                .map(|f| f.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n    static {name} {factory_name}({params}) {{\n        return {name}{{{args}}};\n    }}\n"
            )
        }
        "swift" => {
            let params = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join(", ");
            let args = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}", f.name, f.name))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "\n    static func {factory_name}({params}) -> {name} {{\n        return {name}({args})\n    }}\n"
            )
        }
        _ => String::new(),
    }
}

/// Generates the fluent builder code to add to the target file.
pub fn generate_builder_code(decl: &StructDecl, builder_name: &str) -> String {
    let name = &decl.name;
    match decl.language.as_str() {
        "rust" => {
            let vis = if decl.is_pub { "pub " } else { "" };
            let generics_header = decl
                .generics
                .as_deref()
                .map(|g| format!("{g} "))
                .unwrap_or_default();
            let generics_name = decl.generics.as_deref().unwrap_or_default();

            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    {}: Option<{}>,", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    {vis}fn {}(mut self, value: {}) -> Self {{\n        self.{} = Some(value);\n        self\n    }}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_fields = decl
                .fields
                .iter()
                .map(|f| format!("            {}: self.{}.expect(\"{} is required\"),", f.name, f.name, f.name))
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "\n\n#[derive(Default)]\n{vis}struct {builder_name}{generics_name} {{\n{builder_fields}\n}}\n\nimpl {generics_header}{builder_name}{generics_name} {{\n    {vis}fn new() -> Self {{\n        Self::default()\n    }}\n\n{setters_text}\n\n    {vis}fn build(self) -> {name}{generics_name} {{\n        {name} {{\n{build_fields}\n        }}\n    }}\n}}\n\nimpl {generics_header}{name}{generics_name} {{\n    {vis}fn builder() -> {builder_name}{generics_name} {{\n        {builder_name}::default()\n    }}\n}}"
            )
        }
        "go" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    {} {}", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "func (b *{builder_name}) {}(v {}) *{builder_name} {{\n    b.{} = v\n    return b\n}}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_fields = decl
                .fields
                .iter()
                .map(|f| format!("        {}: b.{},", f.name, f.name))
                .collect::<Vec<_>>()
                .join("\n");

            format!(
                "\n\ntype {builder_name} struct {{\n{builder_fields}\n}}\n\nfunc New{builder_name}() *{builder_name} {{\n    return &{builder_name}{{}}\n}}\n\n{setters_text}\n\nfunc (b *{builder_name}) Build() *{name} {{\n    return &{name}{{\n{build_fields}\n    }}\n}}"
            )
        }
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    private _{}?: {};", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    {}(value: {}): this {{\n        this._{} = value;\n        return this;\n    }}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| format!("this._{}!", f.name))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nexport class {builder_name} {{\n{builder_fields}\n\n{setters_text}\n\n    build(): {name} {{\n        return new {name}({build_args});\n    }}\n}}\n"
            )
        }
        "python" => {
            let inits = decl
                .fields
                .iter()
                .map(|f| format!("        self._{} = None", f.name))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    def {}(self, value):\n        self._{} = value\n        return self",
                    f.name, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| format!("{}={}", f.name, f.name))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nclass {builder_name}:\n    def __init__(self):\n{inits}\n\n{setters_text}\n\n    def build(self) -> \"{name}\":\n        return {name}({build_args})\n"
            )
        }
        "cpp" | "c" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    {} {}_;", f.ty, f.name))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    {builder_name}& {}({} val) {{\n        {}_ = val;\n        return *this;\n    }}",
                    f.name, f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| format!("{}_", f.name))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nstruct {builder_name} {{\n{builder_fields}\n\n{setters_text}\n\n    {name} build() {{\n        return {name}{{{build_args}}};\n    }}\n}};\n"
            )
        }
        "swift" => {
            let builder_fields = decl
                .fields
                .iter()
                .map(|f| format!("    private var {}: {}?", f.name, f.ty))
                .collect::<Vec<_>>()
                .join("\n");

            let mut setters = Vec::new();
            for f in &decl.fields {
                setters.push(format!(
                    "    func set{}(_ value: {}) -> Self {{\n        self.{} = value\n        return self\n    }}",
                    capitalize(&f.name), f.ty, f.name
                ));
            }
            let setters_text = setters.join("\n\n");

            let build_args = decl
                .fields
                .iter()
                .map(|f| format!("{}: {}!", f.name, f.name))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "\n\nclass {builder_name} {{\n{builder_fields}\n\n{setters_text}\n\n    func build() -> {name} {{\n        return {name}({build_args})\n    }}\n}}\n"
            )
        }
        _ => String::new(),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + chars.as_str())
}

/// Discovered raw instantiation to rewrite.
#[derive(Debug, Clone)]
pub struct InstantiationSite {
    pub start: usize,
    pub end: usize,
    pub field_values: BTreeMap<String, String>,
    pub prefix: String,
    pub has_rest_pattern: bool,
}

/// Finds raw struct instantiations of `type_name` in `text` for Rust.
pub fn find_rust_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> (Vec<InstantiationSite>, Vec<String>) {
    let mut sites = Vec::new();
    let mut blocked = Vec::new();

    let matches: Vec<(usize, &str)> = text
        .match_indices(type_name)
        .chain(text.match_indices("Self"))
        .collect();

    for (at, name) in matches {
        if at >= decl_start && at < decl_end {
            continue;
        }
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let before = text[line_start..at].trim();
        if before.starts_with("struct")
            || before.starts_with("enum")
            || before.starts_with("impl")
            || before.starts_with("type ")
            || before.ends_with("->")
            || before.ends_with("for")
        {
            continue;
        }

        // Check if there is an opening brace following `name`
        let Some(brace) = crate::extract_field::constructor_brace(text, at, name) else {
            continue;
        };
        let Some(close) = crate::parameter_object::matching_bracket(text, brace) else {
            continue;
        };

        // Check if it is a pattern rather than an expression
        if matches!(
            crate::extract_field::braces_kind(text, brace, close),
            crate::extract_field::Braces::Pattern { .. }
        ) {
            continue;
        }

        let inner = &text[brace + 1..close];
        let has_rest = inner.contains("..");
        if has_rest {
            let (l, c) = crate::signature::position_at(text, at).unwrap_or((0, 0));
            blocked.push(format!(
                "{l}:{c} instantiation uses struct update syntax `..` which cannot be mapped to positional constructor arguments"
            ));
            continue;
        }

        let mut field_values = BTreeMap::new();
        let chunks = split_balanced_commas(inner);
        for chunk in chunks {
            let trimmed = chunk.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            if let Some((f, val)) = trimmed.split_once(':') {
                field_values.insert(f.trim().to_string(), val.trim().to_string());
            } else if is_ident_str(trimmed) {
                // Shorthand field: `name` is `name: name`
                field_values.insert(trimmed.to_string(), trimmed.to_string());
            }
        }

        // Determine path prefix before `name`, e.g. `crate::models::`
        let mut prefix_start = at;
        let bytes = text.as_bytes();
        while prefix_start >= 2 && &text[prefix_start - 2..prefix_start] == "::" {
            let mut j = prefix_start - 2;
            while j > 0 && is_ident(bytes[j - 1] as char) {
                j -= 1;
            }
            prefix_start = j;
        }
        let prefix = text[prefix_start..at].to_string();

        sites.push(InstantiationSite {
            start: prefix_start,
            end: close + 1,
            field_values,
            prefix,
            has_rest_pattern: false,
        });
    }

    sites.sort_by_key(|s| s.start);
    sites.dedup_by_key(|s| s.start);
    (sites, blocked)
}

/// Finds raw instantiations in Go (`&Type{...}` or `Type{...}`).
pub fn find_go_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> Vec<InstantiationSite> {
    let mut sites = Vec::new();
    for (at, _) in text.match_indices(type_name) {
        if at >= decl_start && at < decl_end {
            continue;
        }
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let after = &text[at + type_name.len()..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('{') {
            continue;
        }
        let open = at + type_name.len() + after.len() - trimmed_after.len();
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };

        let start = if at > 0 && text.as_bytes()[at - 1] == b'&' {
            at - 1
        } else {
            at
        };

        let mut field_values = BTreeMap::new();
        let inner = &text[open + 1..close];
        for chunk in split_balanced_commas(inner) {
            let trimmed = chunk.trim();
            if let Some((k, v)) = trimmed.split_once(':') {
                field_values.insert(k.trim().to_string(), v.trim().to_string());
            }
        }

        sites.push(InstantiationSite {
            start,
            end: close + 1,
            field_values,
            prefix: String::new(),
            has_rest_pattern: false,
        });
    }
    sites
}

/// Finds raw instantiations in TypeScript / JavaScript (`new Type(...)`).
pub fn find_ts_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> Vec<InstantiationSite> {
    let mut sites = Vec::new();
    let pat = format!("new {type_name}");
    for (at, _) in text.match_indices(&pat) {
        if at >= decl_start && at < decl_end {
            continue;
        }
        let after = &text[at + pat.len()..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('(') {
            continue;
        }
        let open = at + pat.len() + after.len() - trimmed_after.len();
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let mut field_values = BTreeMap::new();
        field_values.insert("__raw_args__".to_string(), text[open + 1..close].trim().to_string());
        sites.push(InstantiationSite {
            start: at,
            end: close + 1,
            field_values,
            prefix: String::new(),
            has_rest_pattern: false,
        });
    }
    sites
}

/// Finds raw instantiations in Python (`Type(...)`).
pub fn find_python_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> Vec<InstantiationSite> {
    let mut sites = Vec::new();
    for (at, _) in text.match_indices(type_name) {
        if at >= decl_start && at < decl_end {
            continue;
        }
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        let before = text[..at].trim_end();
        if before.ends_with("class") || before.ends_with("def") || before.ends_with("import") {
            continue;
        }
        let after = &text[at + type_name.len()..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('(') {
            continue;
        }
        let open = at + type_name.len() + after.len() - trimmed_after.len();
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let mut field_values = BTreeMap::new();
        field_values.insert("__raw_args__".to_string(), text[open + 1..close].trim().to_string());
        sites.push(InstantiationSite {
            start: at,
            end: close + 1,
            field_values,
            prefix: String::new(),
            has_rest_pattern: false,
        });
    }
    sites
}

/// Rewrites a single instantiation site based on mode and language.
pub fn rewrite_instantiation(
    site: &InstantiationSite,
    decl: &StructDecl,
    mode: ReplaceMode,
    target_name: &str,
) -> Result<String> {
    match decl.language.as_str() {
        "rust" => {
            let type_ref = format!("{}{}", site.prefix, decl.name);
            match mode {
                ReplaceMode::Factory => {
                    let mut args = Vec::new();
                    for f in &decl.fields {
                        let val = site
                            .field_values
                            .get(&f.name)
                            .with_context(|| format!("missing field `{}` in struct literal", f.name))?;
                        args.push(val.clone());
                    }
                    Ok(format!("{type_ref}::{target_name}({})", args.join(", ")))
                }
                ReplaceMode::Builder => {
                    let mut chain = String::new();
                    for f in &decl.fields {
                        if let Some(val) = site.field_values.get(&f.name) {
                            chain.push_str(&format!(".{}({})", f.name, val));
                        }
                    }
                    Ok(format!("{type_ref}::builder(){chain}.build()"))
                }
            }
        }
        "go" => {
            match mode {
                ReplaceMode::Factory => {
                    let mut args = Vec::new();
                    for f in &decl.fields {
                        let val = site
                            .field_values
                            .get(&f.name)
                            .cloned()
                            .unwrap_or_else(|| f.name.clone());
                        args.push(val);
                    }
                    Ok(format!("{target_name}({})", args.join(", ")))
                }
                ReplaceMode::Builder => {
                    let mut chain = String::new();
                    for f in &decl.fields {
                        if let Some(val) = site.field_values.get(&f.name) {
                            chain.push_str(&format!(".{}({})", f.name, val));
                        }
                    }
                    Ok(format!("New{target_name}(){chain}.Build()"))
                }
            }
        }
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => {
            let raw_args = site
                .field_values
                .get("__raw_args__")
                .map(String::as_str)
                .unwrap_or("");
            match mode {
                ReplaceMode::Factory => Ok(format!("{}.{target_name}({raw_args})", decl.name)),
                ReplaceMode::Builder => {
                    Ok(format!("{}.builder().build()", decl.name))
                }
            }
        }
        "python" => {
            let raw_args = site
                .field_values
                .get("__raw_args__")
                .map(String::as_str)
                .unwrap_or("");
            match mode {
                ReplaceMode::Factory => Ok(format!("{}.{target_name}({raw_args})", decl.name)),
                ReplaceMode::Builder => {
                    Ok(format!("{}.builder().build()", decl.name))
                }
            }
        }
        "cpp" | "c" => {
            match mode {
                ReplaceMode::Factory => Ok(format!("{}::{target_name}()", decl.name)),
                ReplaceMode::Builder => Ok(format!("{}::builder().build()", decl.name)),
            }
        }
        "swift" => {
            match mode {
                ReplaceMode::Factory => Ok(format!("{}.{target_name}()", decl.name)),
                ReplaceMode::Builder => Ok(format!("{}.builder().build()", decl.name)),
            }
        }
        _ => anyhow::bail!("unsupported language: {}", decl.language),
    }
}

/// Core implementation for replacing constructor with factory or builder.
#[allow(clippy::too_many_arguments)]
pub async fn replace_constructor_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    type_name: &str,
    mode: ReplaceMode,
    target_name_opt: Option<&str>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceConstructorResult> {
    let decl_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let decl = parse_struct_declaration(&decl_text, type_name, &language)?;

    let target_name = match target_name_opt {
        Some(n) => n.to_string(),
        None => match mode {
            ReplaceMode::Factory => match language.as_str() {
                "rust" => "new".to_string(),
                "go" => format!("New{type_name}"),
                _ => "create".to_string(),
            },
            ReplaceMode::Builder => format!("{type_name}Builder"),
        },
    };

    let generated_code = match mode {
        ReplaceMode::Factory => generate_factory_code(&decl, &target_name),
        ReplaceMode::Builder => generate_builder_code(&decl, &target_name),
    };

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), decl_text.clone());

    // Edits per file: (byte_offset, replace_len, replacement)
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();

    // 1. Add generated factory / builder declaration to the defining file
    edits
        .entry(file.to_path_buf())
        .or_default()
        .push((decl.decl_end, 0, generated_code));

    let mut all_blocked = Vec::new();
    let mut instantiations_rewritten = 0usize;

    // 2. Discover references to `type_name` across workspace
    let lsp_refs = crate::signature::references(remote, root, file, decl.line, decl.col)
        .await
        .unwrap_or_default();

    let mut target_files = BTreeMap::new();
    target_files.insert(file.to_path_buf(), ());
    for (ref_path, _, _) in lsp_refs {
        target_files.insert(ref_path, ());
    }

    // 3. Find and rewrite instantiations in all candidate files
    for (path, _) in target_files {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let (sites, blocked) = match language.as_str() {
            "rust" => {
                let (s, b) = find_rust_instantiations(
                    &body,
                    type_name,
                    if path == file { decl.decl_start } else { 0 },
                    if path == file { decl.decl_end } else { 0 },
                );
                (s, b)
            }
            "go" => (find_go_instantiations(&body, type_name, 0, 0), Vec::new()),
            "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => {
                (find_ts_instantiations(&body, type_name, 0, 0), Vec::new())
            }
            "python" => (
                find_python_instantiations(&body, type_name, 0, 0),
                Vec::new(),
            ),
            _ => (Vec::new(), Vec::new()),
        };

        all_blocked.extend(blocked.into_iter().map(|b| format!("{}: {b}", display(root, &path))));

        for site in sites {
            match rewrite_instantiation(&site, &decl, mode, &target_name) {
                Ok(replacement) => {
                    edits.entry(path.clone()).or_default().push((
                        site.start,
                        site.end - site.start,
                        replacement,
                    ));
                    instantiations_rewritten += 1;
                }
                Err(err) => {
                    all_blocked.push(format!(
                        "{} at {}: {err}",
                        display(root, &path),
                        site.start
                    ));
                }
            }
        }
    }

    // 4. Materialize rewritten texts in memory
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            if at + len <= body.len() {
                body.replace_range(at..at + len, &replacement);
            }
        }
        rewritten.insert(path, body);
    }

    // 5. Validate proposed edits against analyzer
    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    // 6. Optional compiler check for Rust
    if verify == Some("compile") && language == "rust" {
        let files: Vec<(String, String)> = rewritten
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect();
        let check = crate::compile_check::check(remote, root, &files).await?;
        if !check.passed {
            anyhow::bail!(
                "cargo check failed on the workspace: {} error(s):\n  {}",
                check.errors.len(),
                check.errors.join("\n  ")
            );
        }
    }

    // 7. Apply if requested and safe
    let mut applied = false;
    if apply {
        anyhow::ensure!(
            all_blocked.is_empty() || force,
            "{} instantiation(s) could not be safely rewritten; nothing was written:\n  {}",
            all_blocked.len(),
            all_blocked.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the refactoring produces {} diagnostic error(s); nothing was written. Pass `force: true` to bypass:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ReplaceConstructorResult {
        type_name: type_name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        mode,
        target_name,
        declared_fields: decl.fields.iter().map(|f| format!("{}: {}", f.name, f.ty)).collect(),
        instantiations_rewritten,
        blocked: all_blocked,
        unmatched: Vec::new(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
        language,
    })
}

/// Replace constructor/instantiations with a static factory method.
#[allow(clippy::too_many_arguments)]
pub async fn replace_constructor_with_factory(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    type_name: &str,
    factory_name: Option<&str>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceConstructorResult> {
    replace_constructor_impl(
        remote,
        root,
        file,
        type_name,
        ReplaceMode::Factory,
        factory_name,
        apply,
        force,
        verify,
    )
    .await
}

/// Replace constructor/instantiations with a fluent builder pattern.
#[allow(clippy::too_many_arguments)]
pub async fn replace_constructor_with_builder(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    type_name: &str,
    builder_name: Option<&str>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceConstructorResult> {
    replace_constructor_impl(
        remote,
        root,
        file,
        type_name,
        ReplaceMode::Builder,
        builder_name,
        apply,
        force,
        verify,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rust_struct_fields_and_generates_factory_and_builder() {
        let rust_code = r#"
pub struct User {
    pub name: String,
    pub age: u32,
    email: Option<String>,
}
"#;
        let decl = parse_rust_struct_decl(rust_code, "User").unwrap();
        assert_eq!(decl.name, "User");
        assert!(decl.is_pub);
        assert_eq!(decl.fields.len(), 3);
        assert_eq!(decl.fields[0].name, "name");
        assert_eq!(decl.fields[0].ty, "String");
        assert_eq!(decl.fields[1].name, "age");
        assert_eq!(decl.fields[1].ty, "u32");
        assert_eq!(decl.fields[2].name, "email");
        assert_eq!(decl.fields[2].ty, "Option<String>");

        let factory = generate_factory_code(&decl, "new");
        assert!(factory.contains("pub fn new(name: String, age: u32, email: Option<String>) -> Self"));
        assert!(factory.contains("Self {\n            name,\n            age,\n            email,\n        }"));

        let builder = generate_builder_code(&decl, "UserBuilder");
        assert!(builder.contains("pub struct UserBuilder"));
        assert!(builder.contains("pub fn name(mut self, value: String) -> Self"));
        assert!(builder.contains("pub fn age(mut self, value: u32) -> Self"));
        assert!(builder.contains("pub fn build(self) -> User"));
        assert!(builder.contains("pub fn builder() -> UserBuilder"));
    }

    #[test]
    fn finds_and_rewrites_rust_instantiations() {
        let code = r#"
struct Config {
    pub host: String,
    pub port: u16,
}

fn run() {
    let c1 = Config { host: "127.0.0.1".into(), port: 8080 };
    let c2 = Config { port: 9000, host: "0.0.0.0".into() };
    let host = "localhost".into();
    let port = 3000;
    let c3 = Config { host, port };
}
"#;
        let decl = parse_rust_struct_decl(code, "Config").unwrap();
        let (sites, blocked) = find_rust_instantiations(code, "Config", decl.decl_start, decl.decl_end);
        assert!(blocked.is_empty());
        assert_eq!(sites.len(), 3);

        // Factory rewrite
        let r1 = rewrite_instantiation(&sites[0], &decl, ReplaceMode::Factory, "new").unwrap();
        assert_eq!(r1, "Config::new(\"127.0.0.1\".into(), 8080)");

        // Reordered fields mapped to declared parameter order:
        let r2 = rewrite_instantiation(&sites[1], &decl, ReplaceMode::Factory, "new").unwrap();
        assert_eq!(r2, "Config::new(\"0.0.0.0\".into(), 9000)");

        // Shorthand fields:
        let r3 = rewrite_instantiation(&sites[2], &decl, ReplaceMode::Factory, "new").unwrap();
        assert_eq!(r3, "Config::new(host, port)");

        // Builder rewrite
        let b1 = rewrite_instantiation(&sites[0], &decl, ReplaceMode::Builder, "ConfigBuilder").unwrap();
        assert_eq!(b1, "Config::builder().host(\"127.0.0.1\".into()).port(8080).build()");
    }

    #[test]
    fn detects_and_blocks_struct_update_syntax() {
        let code = r#"
struct Point {
    x: i32,
    y: i32,
}

fn foo() {
    let base = Point { x: 1, y: 2 };
    let p2 = Point { x: 5, ..base };
}
"#;
        let decl = parse_rust_struct_decl(code, "Point").unwrap();
        let (sites, blocked) = find_rust_instantiations(code, "Point", decl.decl_start, decl.decl_end);
        assert_eq!(sites.len(), 1);
        assert_eq!(blocked.len(), 1);
        assert!(blocked[0].contains("struct update syntax `..`"));
    }

    #[test]
    fn polyglot_go_parsing_and_generation() {
        let go_code = r#"
package models

type Service struct {
    Name    string
    Timeout int
}
"#;
        let decl = parse_go_struct_decl(go_code, "Service").unwrap();
        assert_eq!(decl.fields.len(), 2);
        assert_eq!(decl.fields[0].name, "Name");
        assert_eq!(decl.fields[1].name, "Timeout");

        let factory = generate_factory_code(&decl, "NewService");
        assert!(factory.contains("func NewService(Name string, Timeout int) *Service"));
        assert!(factory.contains("return &Service{"));

        let builder = generate_builder_code(&decl, "ServiceBuilder");
        assert!(builder.contains("type ServiceBuilder struct"));
        assert!(builder.contains("func (b *ServiceBuilder) Name(v string) *ServiceBuilder"));
        assert!(builder.contains("func (b *ServiceBuilder) Build() *Service"));
    }

    #[test]
    fn polyglot_ts_parsing_and_generation() {
        let ts_code = r#"
export class Person {
    name: string;
    age: number;
    constructor(name: string, age: number) {
        this.name = name;
        this.age = age;
    }
}
"#;
        let decl = parse_ts_struct_decl(ts_code, "Person", "typescript").unwrap();
        assert_eq!(decl.fields.len(), 2);
        assert_eq!(decl.fields[0].name, "name");
        assert_eq!(decl.fields[1].name, "age");

        let factory = generate_factory_code(&decl, "create");
        assert!(factory.contains("static create(name: string, age: number): Person"));

        let builder = generate_builder_code(&decl, "PersonBuilder");
        assert!(builder.contains("export class PersonBuilder"));
        assert!(builder.contains("name(value: string): this"));
        assert!(builder.contains("build(): Person"));
    }

    #[test]
    fn polyglot_python_parsing_and_generation() {
        let py_code = r#"
class Car:
    def __init__(self, make: str, model: str):
        self.make = make
        self.model = model
"#;
        let decl = parse_python_struct_decl(py_code, "Car").unwrap();
        assert_eq!(decl.fields.len(), 2);
        assert_eq!(decl.fields[0].name, "make");
        assert_eq!(decl.fields[1].name, "model");

        let factory = generate_factory_code(&decl, "create");
        assert!(factory.contains("@classmethod"));
        assert!(factory.contains("def create(cls, make: str, model: str) -> \"Car\":"));

        let builder = generate_builder_code(&decl, "CarBuilder");
        assert!(builder.contains("class CarBuilder:"));
        assert!(builder.contains("def make(self, value):"));
        assert!(builder.contains("def build(self) -> \"Car\":"));
    }
}
