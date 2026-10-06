/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Inverting a predicate: a function returning `bool` gets the opposite name and meaning, and every
//! caller keeps doing what it did.
//!
//! `is_valid` becomes `is_invalid`: the body returns the negation of what it returned, and every call
//! becomes `!is_invalid(…)` — or loses the `!` it already had, since two negations cancel. Nothing is
//! renamed or negated textually by name: the calls are the analyzer's references, and the result is
//! type-checked before anything is written.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use crate::parameter_object::Language;

/// What the inversion did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Inverted {
    pub was: String,
    pub now: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    /// What was inverted: `function`, `field` or `variable`.
    pub kind: String,
    /// Calls (or reads) that gained a `!`.
    pub negated: usize,
    /// Calls (or reads) whose `!` was removed, because it and the inversion cancel.
    pub cancelled: usize,
    /// Writes that now store the negation of what they stored: an assignment, a `let` initialiser,
    /// a field in a struct literal.
    pub writes: usize,
    /// Uses that cannot keep their meaning under the inversion; nothing is written while any
    /// remains, unless `force`.
    pub blocked: Vec<String>,
    /// References that were not negated; nothing is written while any remains, forced or not.
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Inverted {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = if self.kind == "function" {
            format!(
                "`{}` → `{}` ({})\n\n- the body returns the negation of what it returned\n- {} \
                 call(s) gain a `!`, {} lose the `!` they had\n\n",
                self.was, self.now, self.file, self.negated, self.cancelled
            )
        } else {
            format!(
                "`{}` → `{}` ({}, a boolean {})\n\n- {} read(s) gain a `!`, {} lose the `!` they \
                 had\n- {} write(s) now store the negation of what they stored\n\n",
                self.was, self.now, self.file, self.kind, self.negated, self.cancelled, self.writes
            )
        };
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
                "\n{} use(s) cannot keep their meaning under the inversion; nothing is written \
                 while any remains:\n",
                self.blocked.len()
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) that are not a call — a function used as a value \
                 keeps its old meaning under its new name; nothing is written while any remains):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
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

fn is_in_string_or_comment(content: &str, at: usize, lang: Language) -> bool {
    let mut chars = content[..at].chars().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    while let Some(ch) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if lang == Language::Python && ch == '#' {
            line_comment = true;
        } else if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            line_comment = true;
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            block_comment = true;
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
    }
    quote.is_some() || line_comment || block_comment
}

fn one_based_lsp_position(text: &str, byte_offset: usize) -> (u32, u32) {
    let before = &text[..byte_offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column_text = before.rsplit('\n').next().unwrap_or_default();
    let character = column_text.encode_utf16().count() as u32 + 1;
    (line, character)
}

fn is_import_export_call_context(text: &str, at: usize, lang: Language) -> bool {
    if !matches!(lang, Language::TypeScript | Language::JavaScript) {
        return crate::inline_parameter::is_import_or_export_context(text, at, lang);
    }
    let line_start = text[..at].rfind('\n').map_or(0, |position| position + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |offset| at + offset);
    let line = text[line_start..line_end].trim_start();
    if line.starts_with("import ")
        || line.starts_with("import{")
        || line.starts_with("export {")
        || line.starts_with("export{")
        || line.starts_with("export *")
        || line.starts_with("from ")
        || line.contains("require(")
    {
        return true;
    }
    let before = &text[..at];
    before
        .rfind("import {")
        .or_else(|| before.rfind("export {"))
        .is_some_and(|start| !before[start..].contains('}'))
}

fn nested_method_header(header: &str) -> bool {
    let header = header.rsplit('{').next().unwrap_or(header).trim();
    let Some(open) = header.rfind('(') else {
        return false;
    };
    let name = header[..open]
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .trim_start_matches('*')
        .trim_start_matches('&');
    if matches!(name, "if" | "for" | "while" | "switch" | "catch" | "with") {
        return false;
    }
    let rest = header[open..].trim_end();
    rest.ends_with(')')
        || [" const", " async", " throws", " rethrows", " noexcept", " override", " final"]
            .iter()
            .any(|suffix| rest.ends_with(suffix))
}

/// The `return` keywords that return from the function whose body is `inner`: not one inside a
/// closure or an `async` block, where `return` returns from that instead.
pub fn own_returns(inner: &str) -> Vec<usize> {
    let bytes = inner.as_bytes();
    let mut out = Vec::new();
    let mut stack: Vec<bool> = Vec::new(); // true: a closure or async block
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if inner[i..].starts_with("//") {
            i = inner[i..].find('\n').map_or(bytes.len(), |n| i + n);
            continue;
        }
        if inner[i..].starts_with("/*") {
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if inner[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if inner[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += inner[i..].chars().next().unwrap().len_utf8();
                }
            }
            continue;
        }
        if matches!(c, b'"' | b'\'' | b'`') {
            let quote = c;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 1;
                    if i < bytes.len() {
                        i += inner[i..].chars().next().unwrap().len_utf8();
                    }
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += inner[i..].chars().next().unwrap().len_utf8();
                }
            }
            continue;
        }
        if c == b'{' {
            let head = inner[..i].trim_end();
            let line = &head[head.rfind('\n').map_or(0, |n| n + 1)..];
            // A closure, an `async` block or a nested `fn` has returns of its own.
            let opaque = head.ends_with("async")
                || head.ends_with("async move")
                || head.ends_with('|')
                || (line.contains('|') && line.contains("->"))
                || line.trim_start().starts_with("fn ")
                || line.contains(" fn ")
                || line.contains("func ")
                || line.contains("function ")
                || line.contains("=>")
                || nested_method_header(line);
            stack.push(opaque);
            i += 1;
            continue;
        }
        if c == b'}' {
            stack.pop();
            i += 1;
            continue;
        }
        if inner[i..].starts_with("return")
            && !inner[..i].chars().next_back().is_some_and(is_ident)
            && !inner[i + 6..].chars().next().is_some_and(is_ident)
            && !stack.iter().any(|opaque| *opaque)
            && !inner[..i].trim_end().ends_with('|')
        {
            out.push(i);
        }
        i += inner[i..].chars().next().unwrap().len_utf8();
    }
    out
}

/// Where the expression after `return` at `at` ends: its `;`, or the end of the text.
fn return_value_end(inner: &str, at: usize) -> usize {
    let bytes = inner.as_bytes();
    let mut i = at + "return".len();
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => match crate::parameter_object::matching_bracket(inner, i) {
                Some(close) => i = close + 1,
                None => return bytes.len(),
            },
            b';' | b'}' | b'\n' => return i,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// The body of the inverted function, from the body of the original (the text between its braces).
pub fn negated_body(inner: &str) -> String {
    let mut body = inner.to_string();
    for at in own_returns(inner).into_iter().rev() {
        let end = return_value_end(inner, at);
        let value = inner[at + "return".len()..end].trim();
        body.replace_range(at..end, &format!("return !({value})"));
    }
    let trimmed = body.trim();
    // A body that is one expression reads best negated in place; anything with statements is
    // negated as a block, whose value is its tail.
    if !trimmed.contains(';') && !trimmed.contains("return") && !trimmed.is_empty() {
        return format!("\n    !({trimmed})\n");
    }
    let indented: String = body
        .trim_matches('\n')
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("    {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n    !{{\n{indented}\n    }}\n")
}

/// Where a call whose callee name starts at `at` begins: the start of its receiver chain for a
/// method call, of its path for a path call, or the name itself.
fn call_start(text: &str, at: usize) -> usize {
    let before = text[..at].trim_end();
    if let Some(dot_end) = before.strip_suffix('.').map(|b| b.len()) {
        return crate::encapsulate_field::chain_start(text, dot_end);
    }
    if let Some(arrow_end) = before.strip_suffix("->").map(|b| b.len()) {
        return crate::encapsulate_field::chain_start(text, arrow_end);
    }
    let mut start = at;
    loop {
        let head = &text[..start];
        let Some(rest) = head.strip_suffix("::") else {
            return start;
        };
        let seg_start = rest
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c))
            .last()
            .map_or(rest.len(), |(i, _)| i);
        if seg_start == rest.len() {
            return start;
        }
        start = seg_start;
    }
}

/// Inverts the predicate declared at `line`:`col` of `file` under `new_name`.
#[allow(clippy::too_many_arguments)]
pub async fn invert(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    new_name: &str,
    apply: bool,
    force: bool,
) -> Result<Inverted> {
    anyhow::ensure!(
        !new_name.is_empty() && new_name.chars().all(is_ident),
        "`{new_name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    if !text[..start].trim_end().ends_with("fn") {
        let name: String = text[start..].chars().take_while(|c| is_ident(*c)).collect();
        let kind = crate::invert_value::value_kind(&text, start, &name).context(
            "the position is not the name of a function returning `bool`, a `bool` field or a \
             `let` binding",
        )?;
        return crate::invert_value::invert_value(
            remote, root, file, &text, start, kind, new_name, apply, force,
        )
        .await;
    }
    let (name, _, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    anyhow::ensure!(name != new_name, "the new name is the old one");
    let returns = crate::wrap_return::declared_return(&text, close)
        .map(|(s, e)| text[s..e].to_string())
        .unwrap_or_default();
    anyhow::ensure!(
        returns == "bool",
        "`{name}` returns `{}`, not `bool`",
        if returns.is_empty() { "()" } else { &returns }
    );
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{name}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the body does not close")?;

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((start, name.len(), new_name.to_string()));
    own.push((
        body_open + 1,
        body_close - body_open - 1,
        negated_body(&text[body_open + 1..body_close]),
    ));

    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let (mut negated, mut cancelled) = (0usize, 0usize);
    let mut unmatched = Vec::new();
    let (nl, nc) = crate::signature::position_at(&text, start)?;
    let refs = crate::signature::references(remote, root, file, nl, nc)
        .await
        .with_context(|| format!("cannot find the calls to `{name}`; nothing was planned"))?;
    for (path, l, c) in refs {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
        let site = format!("{}:{l}:{c}", display(root, &path));
        let Some(at) = crate::signature::offset_of(&body, l, c) else {
            unmatched.push(format!("{site} (the position is not in the file)"));
            continue;
        };
        if !body[at..].starts_with(name.as_str()) || body[at + name.len()..].starts_with(is_ident) {
            unmatched.push(format!(
                "{site} (the analyzer places `{name}` here, but the file says otherwise)"
            ));
            continue;
        }
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        anyhow::ensure!(
            !(same_file && body_open < at && at < body_close),
            "`{name}` calls itself; invert a recursive predicate by hand"
        );
        let Some((_, args_end)) = crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            unmatched.push(format!("{site} `{name}` used as a value"));
            continue;
        };
        let begin = call_start(&body, at);
        let after = body[args_end + 1..].trim_start();
        let continues = after.starts_with('.') || after.starts_with('?') || after.starts_with('[');
        let lead = body[..begin].trim_end();
        let spot = edits.entry(path.clone()).or_default();
        spot.push((at, name.len(), new_name.to_string()));
        if !continues && lead.ends_with('!') && !lead.ends_with("!=") {
            // `!is_valid(x)` becomes `is_invalid(x)`: the two negations cancel.
            spot.push((lead.len() - 1, 1, String::new()));
            cancelled += 1;
        } else if continues {
            spot.push((begin, 0, "(!".to_string()));
            spot.push((args_end + 1, 0, ")".to_string()));
            negated += 1;
        } else {
            spot.push((begin, 0, "!".to_string()));
            negated += 1;
        }
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        // Insertions at the same offset as a replacement go before it: sort by offset, then put
        // zero-length edits first.
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }

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

    let mut applied = false;
    if apply {
        // A function used as a value keeps its old meaning under the new name, and still
        // compiles; `force` overrides the analyzer, not a use this did not negate (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not negated and would mean the opposite; nothing \
             was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` \
             to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(Inverted {
        was: name,
        now: new_name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        kind: "function".to_string(),
        negated,
        cancelled,
        writes: 0,
        blocked: Vec::new(),
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}

fn negate_expr_python(expr: &str) -> String {
    let t = expr.trim();
    if t == "True" {
        return "False".to_string();
    }
    if t == "False" {
        return "True".to_string();
    }
    if let Some(after) = t.strip_prefix("not ") {
        let trimmed = after.trim();
        if trimmed.starts_with('(')
            && trimmed.ends_with(')')
            && let Some(close) = crate::parameter_object::matching_bracket(trimmed, 0)
            && close == trimmed.len() - 1
        {
            return trimmed[1..trimmed.len() - 1].trim().to_string();
        }
        return trimmed.to_string();
    }
    if t.starts_with("not(")
        && t.ends_with(')')
        && let inside = &t[3..]
        && let Some(close) = crate::parameter_object::matching_bracket(inside, 0)
        && close == inside.len() - 1
    {
        return inside[1..inside.len() - 1].trim().to_string();
    }
    format!("not ({t})")
}

fn negate_expr_c_like(expr: &str) -> String {
    let t = expr.trim();
    if t == "true" {
        return "false".to_string();
    }
    if t == "false" {
        return "true".to_string();
    }
    if t.starts_with('!') && !t.starts_with("!=") && !t.starts_with("!==") {
        let after = t[1..].trim();
        if after.starts_with('(')
            && after.ends_with(')')
            && let Some(close) = crate::parameter_object::matching_bracket(after, 0)
            && close == after.len() - 1
        {
            return after[1..after.len() - 1].trim().to_string();
        }
        return after.to_string();
    }
    format!("!({t})")
}

fn negate_python_body(body: &str) -> String {
    let mut out_lines = Vec::new();
    let mut min_def_indent = None;

    for line in body.lines() {
        let trimmed = line.trim();
        let indent = line.len() - line.trim_start().len();

        if let Some(def_ind) = min_def_indent {
            if indent > def_ind {
                out_lines.push(line.to_string());
                continue;
            } else if !trimmed.is_empty() {
                min_def_indent = None;
            }
        }

        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            min_def_indent = Some(indent);
            out_lines.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("return ") {
            let val = trimmed.strip_prefix("return ").unwrap().trim();
            let leading = &line[..indent];
            out_lines.push(format!("{leading}return {}", negate_expr_python(val)));
        } else {
            out_lines.push(line.to_string());
        }
    }
    out_lines.join("\n")
}

fn negate_c_like_body(body: &str) -> String {
    let mut out = body.to_string();
    let returns = own_returns(body);
    for at in returns.into_iter().rev() {
        let end = return_value_end(body, at);
        let val_with_semi = body[at + "return".len()..end].trim();
        let has_semi = val_with_semi.ends_with(';');
        let val = val_with_semi.trim_end_matches(';').trim();
        if !val.is_empty() {
            let neg = negate_expr_c_like(val);
            let semi = if has_semi { ";" } else { "" };
            out.replace_range(at..end, &format!("return {neg}{semi}"));
        }
    }
    let trimmed = out.trim();
    if !trimmed.contains(';') && !trimmed.contains("return") && !trimmed.is_empty() {
        return format!("\n    {}\n", negate_expr_c_like(trimmed));
    }
    out
}

struct PolyglotPredDecl {
    fn_name: String,
    decl_name_at: usize,
    close_paren: usize,
    body_open: usize,
    body_close: usize,
}

fn is_function_decl(line: &str, lang: Language, name: &str) -> bool {
    let trimmed = line.trim();
    match lang {
        Language::Go => {
            if let Some(rest) = trimmed.strip_prefix("func ") {
                if rest.starts_with('(') {
                    if let Some(close) = rest.find(')') {
                        let after_recv = rest[close + 1..].trim_start();
                        after_recv.starts_with(name)
                    } else {
                        false
                    }
                } else {
                    rest.starts_with(name)
                }
            } else {
                false
            }
        }
        Language::Python => {
            let rest = trimmed.strip_prefix("async ").unwrap_or(trimmed);
            rest.strip_prefix("def ").is_some_and(|after| after.trim_start().starts_with(name))
        }
        Language::Swift => {
            trimmed.contains("func ") && crate::inline_parameter::extract_decl_name_from_line(trimmed, lang).as_deref() == Some(name)
        }
        Language::TypeScript | Language::JavaScript => {
            let rest = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            let rest = rest.strip_prefix("default ").unwrap_or(rest);
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            if let Some(after) = rest.strip_prefix("function ") {
                after.trim_start().starts_with(name)
            } else {
                !trimmed.starts_with("const ")
                    && !trimmed.starts_with("let ")
                    && !trimmed.starts_with("var ")
                    && !trimmed.starts_with("return ")
                    && crate::inline_parameter::extract_decl_name_from_line(trimmed, lang).as_deref() == Some(name)
            }
        }
        Language::Cpp | Language::C => {
            !trimmed.starts_with("return ")
                && !trimmed.contains('=')
                && crate::inline_parameter::extract_decl_name_from_line(trimmed, lang).as_deref() == Some(name)
        }
        _ => false,
    }
}

fn find_polyglot_predicate_declaration(
    text: &str,
    lang: Language,
    line: Option<u32>,
    symbol: Option<&str>,
) -> Result<PolyglotPredDecl> {
    let clean_name = symbol
        .map(|s| {
            s.rsplit_once("::")
                .map(|(_, m)| m)
                .or_else(|| s.rsplit_once('.').map(|(_, m)| m))
                .unwrap_or(s)
                .trim()
                .to_string()
        })
        .or_else(|| {
            let l = line?;
            let lines: Vec<&str> = text.lines().collect();
            if l == 0 || l as usize > lines.len() {
                return None;
            }
            let target_idx = (l - 1) as usize;
            let start_idx = target_idx.saturating_sub(3);
            let end_idx = (target_idx + 3).min(lines.len().saturating_sub(1));
            for i in (start_idx..=end_idx).rev() {
                if let Some(name) = crate::inline_parameter::extract_decl_name_from_line(lines[i], lang) {
                    return Some(name);
                }
            }
            None
        })
        .context("could not determine predicate name to invert")?;

    let needle_paren = format!("{clean_name}(");
    let needle_space = format!("{clean_name} (");
    let needle_generic = format!("{clean_name}<");

    let mut candidates = Vec::new();
    for (pos, _) in text
        .match_indices(&needle_paren)
        .chain(text.match_indices(&needle_space))
        .chain(text.match_indices(&needle_generic))
    {
        if pos > 0 {
            let prev = text[..pos].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let after_name = pos + clean_name.len();
        let open_paren = match text[after_name..].find('(') {
            Some(p) => after_name + p,
            None => continue,
        };
        let close_paren = match crate::parameter_object::matching_bracket(text, open_paren) {
            Some(p) => p,
            None => continue,
        };

        let (body_open, body_close) = if lang == Language::Python {
            let colon = match text[close_paren..].find(':') {
                Some(c) => close_paren + c,
                None => continue,
            };
            let b_close = crate::inline_parameter::find_python_body_close(text, pos, colon);
            (colon, b_close)
        } else {
            let b_open = match text[close_paren..].find('{') {
                Some(b) => close_paren + b,
                None => continue,
            };
            let b_close = match crate::parameter_object::matching_bracket(text, b_open) {
                Some(b) => b,
                None => continue,
            };
            (b_open, b_close)
        };

        let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
        let line_text = text[line_start..].lines().next().unwrap_or("");
        let is_decl = is_function_decl(line_text, lang, &clean_name);
        let pos_line = text[..pos].split('\n').count() as u32;

        candidates.push((
            is_decl,
            pos_line,
            PolyglotPredDecl {
                fn_name: clean_name.clone(),
                decl_name_at: pos,
                close_paren,
                body_open,
                body_close,
            },
        ));
    }

    let best = if let Some(target_line) = line {
        candidates
            .into_iter()
            .min_by_key(|(is_decl, l, _)| {
                let dist = (*l as i64 - target_line as i64).abs();
                (!*is_decl, dist)
            })
            .map(|(_, _, decl)| decl)
    } else {
        candidates
            .into_iter()
            .min_by_key(|(is_decl, _, _)| !*is_decl)
            .map(|(_, _, decl)| decl)
    };

    best.with_context(|| format!("could not find declaration for predicate `{clean_name}`"))
}

/// Inverts the boolean predicate in polyglot languages: TypeScript/JavaScript, Python, C++, Swift, Go.
#[allow(clippy::too_many_arguments)]
pub async fn invert_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: Option<u32>,
    _character: Option<u32>,
    symbol: Option<&str>,
    new_name: &str,
    apply: bool,
    force: bool,
) -> Result<Inverted> {
    anyhow::ensure!(
        !new_name.is_empty() && new_name.chars().all(is_ident),
        "`{new_name}` is not an identifier"
    );
    let lang = Language::of(file).with_context(|| format!("unsupported language for {}", file.display()))?;
    let text = std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let decl = find_polyglot_predicate_declaration(&text, lang, line, symbol)?;
    let (selected_line, selected_col) = one_based_lsp_position(&text, decl.decl_name_at);
    let mut semantic_references = crate::signature::references(
        remote,
        root,
        file,
        selected_line,
        selected_col,
    )
    .await?
    .into_iter()
    .map(|(path, ref_line, ref_col)| {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        (path, ref_line, ref_col)
    })
    .collect::<std::collections::HashSet<_>>();
    let references_are_empty = semantic_references.is_empty();
    let selected_cpp_param_types = if matches!(lang, Language::Cpp | Language::C) {
        let after_name = decl.decl_name_at + decl.fn_name.len();
        let open_paren = after_name + text[after_name..decl.close_paren].find('(').unwrap_or_default();
        crate::parameter_object::parse_params(
            &text[open_paren + 1..decl.close_paren],
            lang,
        )
        .1
        .into_iter()
        .map(|param| param.ty)
        .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    anyhow::ensure!(decl.fn_name != new_name, "the new name is the old one");

    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());

    let (mut negated, mut cancelled) = (0usize, 0usize);
    let mut all_unmatched = Vec::new();

    // Declaration file edits: rename function at declaration and negate body
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((decl.decl_name_at, decl.fn_name.len(), new_name.to_string()));

    let body_slice = &text[decl.body_open + 1..decl.body_close];
    let new_body = if lang == Language::Python {
        negate_python_body(body_slice)
    } else {
        negate_c_like_body(body_slice)
    };
    own.push((decl.body_open + 1, decl.body_close - decl.body_open - 1, new_body));

    let canonical_file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());

    // Traverse workspace files for calls, prototypes, and imports
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !crate::inline_parameter::language_matches(lang, path) {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !other_content.contains(&decl.fn_name) {
            continue;
        }

        let is_decl_file = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) == canonical_file;
        let rel_path = display(root, path);

        let mut file_edits = Vec::new();

        for (at, _) in other_content.match_indices(&decl.fn_name) {
            if at > 0 {
                let prev = other_content[..at].chars().next_back().unwrap();
                if is_ident(prev) {
                    continue;
                }
            }
            let after = &other_content[at + decl.fn_name.len()..];
            if after.starts_with(is_ident) {
                continue;
            }
            if is_in_string_or_comment(&other_content, at, lang) {
                continue;
            }

            let source_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            let (line, col) = one_based_lsp_position(&other_content, at);
            let reference_key = (source_path, line, col);
            let site = format!("{rel_path}:{line}:{col}");

            if is_import_export_call_context(&other_content, at, lang) {
                if references_are_empty {
                    all_unmatched.push(format!(
                        "{site}: analyzer references for `{}` were empty; nothing was written",
                        decl.fn_name
                    ));
                    continue;
                }
                semantic_references.remove(&reference_key);
                let line_start = other_content[..at].rfind('\n').map_or(0, |n| n + 1);
                let line_end = other_content[at..]
                    .find('\n')
                    .map_or(other_content.len(), |n| at + n);
                if other_content[line_start..line_end].contains(" as ") {
                    all_unmatched.push(format!(
                        "{site} imports `{}` through an alias; alias call sites are not resolved",
                        decl.fn_name
                    ));
                } else {
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                }
                continue;
            }

            // Declaration check in declaring file
            if is_decl_file && at >= decl.decl_name_at && at <= decl.close_paren {
                semantic_references.remove(&reference_key);
                continue;
            }

            if references_are_empty {
                all_unmatched.push(format!(
                    "{site}: analyzer references for `{}` were empty; nothing was written",
                    decl.fn_name
                ));
                continue;
            }

            // Self-call check inside function's own body
            if is_decl_file && at > decl.body_open && at < decl.body_close {
                if semantic_references.remove(&reference_key) {
                    anyhow::bail!("`{}` calls itself; invert a recursive predicate by hand", decl.fn_name);
                }
                continue;
            }

            let candidate_args = crate::parameter_object::call_args_span(
                &other_content,
                at + decl.fn_name.len(),
            );
            if matches!(lang, Language::Cpp | Language::C)
                && let Some((args_start, args_end)) = candidate_args
                && crate::inline_parameter::is_c_cpp_prototype(
                    &other_content,
                    at,
                    args_end,
                )
            {
                let (_, proto_params) = crate::parameter_object::parse_params(
                    &other_content[args_start..args_end],
                    lang,
                );
                let proto_types = proto_params
                    .into_iter()
                    .map(|param| param.ty)
                    .collect::<Vec<_>>();
                if proto_types == selected_cpp_param_types {
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                }
                continue;
            }

            if !semantic_references.remove(&reference_key) {
                continue;
            }

            let Some((_args_start, args_end)) = candidate_args else {
                all_unmatched.push(format!("{site} `{}` used as a value", decl.fn_name));
                continue;
            };

            // Real call site!
            let begin = call_start(&other_content, at);
            let after_call = other_content[args_end + 1..].trim_start();
            let continues = after_call.starts_with('.') || after_call.starts_with('?') || after_call.starts_with('[');
            let lead = other_content[..begin].trim_end();

            if lang == Language::Python {
                let is_negated = if let Some(before_not) = lead.strip_suffix("not") {
                    before_not.chars().next_back().is_none_or(|c| !is_ident(c))
                } else {
                    false
                };
                if is_negated {
                    let not_start = lead.len() - 3;
                    file_edits.push((not_start, begin - not_start, String::new()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    cancelled += 1;
                } else if continues {
                    file_edits.push((begin, 0, "(not ".to_string()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    file_edits.push((args_end + 1, 0, ")".to_string()));
                    negated += 1;
                } else {
                    file_edits.push((begin, 0, "not ".to_string()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    negated += 1;
                }
            } else {
                let is_negated = lead.ends_with('!') && !lead.ends_with("!=") && !lead.ends_with("!==");
                if !continues && is_negated {
                    let not_start = lead.len() - 1;
                    file_edits.push((not_start, begin - not_start, String::new()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    cancelled += 1;
                } else if continues {
                    file_edits.push((begin, 0, "(!".to_string()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    file_edits.push((args_end + 1, 0, ")".to_string()));
                    negated += 1;
                } else {
                    file_edits.push((begin, 0, "!".to_string()));
                    file_edits.push((at, decl.fn_name.len(), new_name.to_string()));
                    negated += 1;
                }
            }
        }

        if !file_edits.is_empty() {
            texts.insert(path.to_path_buf(), other_content);
            edits.entry(path.to_path_buf()).or_default().extend(file_edits);
        }
    }

    for (path, ref_line, ref_col) in semantic_references {
        all_unmatched.push(format!(
            "{}:{ref_line}:{ref_col}: analyzer reference to `{}` could not be inverted safely",
            display(root, &path),
            decl.fn_name
        ));
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }

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

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            all_unmatched.is_empty(),
            "{} reference(s) to `{}` were not negated and would mean the opposite; nothing was written:\n  {}",
            all_unmatched.len(),
            decl.fn_name,
            all_unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(Inverted {
        was: decl.fn_name,
        now: new_name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        kind: "function".to_string(),
        negated,
        cancelled,
        writes: 0,
        blocked: Vec::new(),
        unmatched: all_unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_expression_body_is_negated_in_place() {
        assert_eq!(negated_body("\n    self.n > 0\n"), "\n    !(self.n > 0)\n");
    }

    #[test]
    fn a_body_with_statements_is_negated_as_a_block_and_its_returns_one_by_one() {
        let inner =
            "\n    if n == 0 {\n        return true;\n    }\n    let m = n % 2;\n    m == 0\n";
        let out = negated_body(inner);
        assert!(out.contains("return !(true);"), "{out}");
        assert!(out.trim_start().starts_with("!{"), "{out}");
        assert!(out.contains("m == 0"), "{out}");
    }

    #[test]
    fn a_return_inside_a_closure_or_an_async_block_is_not_the_functions() {
        let inner = "\n    let f = |x: u32| { return x > 1; };\n    let g = async { return 3; };\n    f(2)\n";
        assert!(own_returns(inner).is_empty(), "{:?}", own_returns(inner));
        let inner = "\n    fn helper() -> bool {\n        return true;\n    }\n    helper()\n";
        assert!(
            own_returns(inner).is_empty(),
            "a nested fn's return is its own"
        );
        let inner = "\n    if x { return false; }\n    true\n";
        assert_eq!(own_returns(inner).len(), 1);
        assert!(own_returns("\n    \"return\" == s\n").is_empty());
        assert!(own_returns("\n    returns_ok()\n").is_empty());
    }

    #[test]
    fn a_return_inside_a_nested_class_method_is_not_the_functions() {
        let body = "\n    class Local {\n        check() {\n            return true;\n        }\n    }\n    return false;\n";
        assert_eq!(own_returns(body), vec![body.rfind("return false").unwrap()]);
    }

    #[test]
    fn a_return_inside_single_quoted_template_or_comment_text_is_ignored() {
        let body = "let message = '🟦 return true;';\nlet template = `return false;`;\n/* return true; */\nreturn result;";
        assert_eq!(own_returns(body), vec![body.rfind("return result").unwrap()]);
    }

    #[test]
    fn a_call_starts_at_its_receiver_or_its_path() {
        let t = "if cfg.limits().is_valid(1) {}";
        assert_eq!(
            call_start(t, t.find("is_valid").unwrap()),
            t.find("cfg").unwrap()
        );
        let t = "let ok = crate::rules::is_valid(1);";
        assert_eq!(
            call_start(t, t.find("is_valid").unwrap()),
            t.find("crate").unwrap()
        );
        let t = "let ok = is_valid(1);";
        assert_eq!(
            call_start(t, t.find("is_valid").unwrap()),
            t.find("is_valid").unwrap()
        );
    }

    fn report() -> Inverted {
        Inverted {
            was: "is_valid".into(),
            now: "is_invalid".into(),
            root: "/root".into(),
            file: "src/lib.rs".into(),
            kind: "function".into(),
            negated: 2,
            cancelled: 1,
            writes: 0,
            blocked: Vec::new(),
            unmatched: vec!["src/app.rs:9:14 `is_valid` used as a value".into()],
            rewritten: vec![("/root/src/lib.rs".into(), "fn main() {}\n".into())],
            diagnostics: vec!["mismatched types (src/app.rs:4:5)".into()],
            applied: false,
        }
    }

    #[test]
    fn the_report_counts_the_negations_and_names_what_was_left() {
        let text = report().render(10_000);
        assert!(text.contains("`is_valid` → `is_invalid`"), "{text}");
        assert!(
            text.contains("2 call(s) gain a `!`, 1 lose the `!` they had"),
            "{text}"
        );
        assert!(text.contains("used as a value"), "{text}");
        assert!(text.contains("the analyzer rejects the result"), "{text}");
        let mut done = report();
        done.unmatched.clear();
        done.diagnostics.clear();
        done.applied = true;
        let text = done.render(10);
        assert!(
            text.contains("0 errors")
                && text.contains("[applied to 1 file(s)]")
                && text.contains("diff truncated"),
            "{text}"
        );

        let mut field = report();
        field.kind = "field".into();
        field.writes = 3;
        field.blocked = vec!["src/lib.rs:1:1 the struct derives `Default`".into()];
        let text = field.render(10_000);
        assert!(text.contains("a boolean field"), "{text}");
        assert!(
            text.contains("2 read(s) gain a `!`, 1 lose the `!`"),
            "{text}"
        );
        assert!(text.contains("3 write(s) now store the negation"), "{text}");
        assert!(
            text.contains("1 use(s) cannot keep their meaning"),
            "{text}"
        );
    }
}
