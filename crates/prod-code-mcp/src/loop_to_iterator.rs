//! Turning a loop that only builds up an accumulator into an iterator chain: `let mut sum = 0;
//! for p in prices { sum += p * 2; }` becomes `let sum: u64 = prices.into_iter().map(|p| p * 2)
//! .sum();`.
//!
//! rust-analyzer offers `for_each` and `while let`, which keep the mutable accumulator. This
//! recognises the shapes whose chain means the same: a sum from zero, a count of matches, and a
//! vector built with `push`, each with or without an `if` around the one statement. `for P in E`
//! iterates `E.into_iter()`, and the closure binds `P` exactly as the loop did. Anything else in
//! the body (a second statement, `break`, `?`, another use of the accumulator) is refused, and
//! the result is type-checked before anything is written.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use crate::parameter_object::Language;

/// What the loop builds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Shape {
    /// `acc += X`, maybe under `if C`.
    Sum { cond: Option<String>, value: String },
    /// `if C { acc += 1 }` into a `usize`.
    Count { cond: String },
    /// `acc.push(X)`, maybe under `if C`.
    Collect { cond: Option<String>, value: String },
    /// `if C { acc = Some(X); break; }` into `Option<T>`.
    Find { cond: String, value: String },
    /// `if C { acc = true; break; }` into `bool`.
    Any { cond: String },
    /// `if !C { acc = false; break; }` into `bool`.
    All { cond: String },
}

/// A recognised polyglot loop replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolyglotLoop {
    pub start: usize,
    pub end: usize,
    pub indent: String,
    pub replacement: String,
    pub statement: String,
}

/// A recognised loop and the statement that declares its accumulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccumulatorLoop {
    /// Where the `let` line starts and where the loop's closing brace ends.
    pub start: usize,
    pub end: usize,
    pub indent: String,
    pub acc: String,
    /// The declared type, when the `let` has one.
    pub declared: Option<String>,
    pub pattern: String,
    pub source: String,
    pub shape: Shape,
}

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Rewritten {
    pub statement: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub new_text: String,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Rewritten {
    pub fn render(&self) -> String {
        let full = self.root.join(&self.file);
        let old_text = if self.applied {
            crate::refactor::text_before_apply(&full)
        } else {
            std::fs::read_to_string(&full).unwrap_or_default()
        };
        let mut out = format!(
            "{}\n\n{}",
            self.statement.trim(),
            similar::TextDiff::from_lines(old_text.as_str(), self.new_text.as_str())
                .unified_diff()
                .context_radius(1)
                .header(&format!("a/{}", self.file), &format!("b/{}", self.file))
        );
        if self.diagnostics.is_empty() {
            out.push_str("\nthe analyzer accepts the result: 0 errors\n");
        } else {
            out.push_str("\nthe analyzer rejects the result:\n");
            for d in &self.diagnostics {
                out.push_str(&format!("  {d}\n"));
            }
        }
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written; pass `apply: true` to make this edit\n"
        });
        out
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The first `c` in `text[from..]` outside parentheses and brackets.
fn at_depth_zero(text: &str, from: usize, want: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = from;
    while i < text.len() {
        let rest = &text[i..];
        if depth == 0 && rest.starts_with(want) {
            return Some(i);
        }
        let c = rest.chars().next()?;
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

fn whole_word_count(text: &str, word: &str) -> usize {
    text.match_indices(word)
        .filter(|(i, _)| {
            !text[..*i].chars().next_back().is_some_and(is_ident)
                && !text[i + word.len()..].chars().next().is_some_and(is_ident)
        })
        .count()
}

/// The zero a sum starts from: `0`, `0.0`, `0u64`, `0_i32`, `0.0f64`.
fn is_zero(init: &str) -> bool {
    let s = init.trim().trim_end_matches([';', ',']);
    if s == "0" || s == "0.0" || s == "0n" || s == "0L" || s == "0.0f" {
        return true;
    }
    let number: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '_')
        .collect();
    let suffix = &s[number.len()..];
    !number.is_empty()
        && number.chars().all(|c| c == '0' || c == '.' || c == '_')
        && (suffix.is_empty()
            || matches!(
                suffix.trim_start_matches('_'),
                "i8" | "i16"
                    | "i32"
                    | "i64"
                    | "i128"
                    | "isize"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "u128"
                    | "usize"
                    | "f32"
                    | "f64"
                    | "L"
                    | "f"
            ))
}

/// The accumulating statement of a body, and what it adds: `acc += X;` or `acc.push(X);`.
fn accumulation(stmt: &str, acc: &str) -> Option<(bool, String)> {
    let stmt = stmt.trim().trim_end_matches(';').trim();
    if let Some(rest) = stmt.strip_prefix(acc) {
        let rest = rest.trim_start();
        if let Some(value) = rest.strip_prefix("+=") {
            return Some((false, value.trim().to_string()));
        }
        if let Some(args) = rest.strip_prefix(".push(") {
            let value = args.strip_suffix(')')?;
            return Some((true, value.trim().to_string()));
        }
    }
    None
}

/// Recognises the loop whose `for` is on the line holding `at`, with the `let mut` just above.
pub fn recognise(text: &str, at: usize) -> Result<AccumulatorLoop> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let for_at = text[line_start..line_end]
        .match_indices("for ")
        .map(|(i, _)| line_start + i)
        .find(|i| !text[..*i].chars().next_back().is_some_and(is_ident))
        .context("no `for` loop on this line")?;
    let in_at = at_depth_zero(text, for_at + 4, " in ").context("the `for` has no `in`")?;
    let pattern = text[for_at + 4..in_at].trim().to_string();
    let open = at_depth_zero(text, in_at + 4, "{").context("the loop has no body")?;
    let source = text[in_at + 4..open].trim().to_string();
    let close = crate::parameter_object::matching_bracket(text, open)
        .context("the loop's body is not closed")?;
    let body = text[open + 1..close].trim();

    // The statement just above: `let mut acc = init;` or `let mut acc: T = init;`, one line.
    let before = text[..line_start].trim_end_matches(['\n', ' ', '\t']);
    let let_start = before.rfind('\n').map_or(0, |i| i + 1);
    let let_line = before[let_start..].trim();
    let rest = let_line
        .strip_prefix("let mut ")
        .context("the statement above the loop is not `let mut <accumulator> = …;`")?;
    let rest = rest
        .strip_suffix(';')
        .context("the accumulator's declaration is not one line")?;
    let (lhs, init) = rest
        .split_once(" = ")
        .context("the accumulator has no start value")?;
    let (acc, declared) = match lhs.split_once(':') {
        Some((n, t)) => (n.trim().to_string(), Some(t.trim().to_string())),
        None => (lhs.trim().to_string(), None),
    };
    anyhow::ensure!(
        !acc.is_empty() && acc.chars().all(is_ident),
        "`{lhs}` is not a single variable"
    );
    let init = init.trim();

    for word in ["continue", "return"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }
    anyhow::ensure!(
        !body.contains('?') && !body.contains(".await"),
        "the loop's body can leave early (`?`) or await"
    );

    let shape = if whole_word_count(body, "break") == 1 {
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = at_depth_zero(rest, 0, "{").context("the `if` has no body")?;
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the loop's body has `break`, which an iterator chain cannot express"
                );
                (
                    rest[..brace].trim().to_string(),
                    body[inner_open + 1..inner_close].trim(),
                )
            }
            None => anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express"),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        let parts: Vec<&str> = stmt_trimmed.split(';').map(str::trim).filter(|s| !s.is_empty()).collect();
        anyhow::ensure!(
            parts.len() == 2 && parts[1] == "break",
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let assign = parts[0];
        let (assign_lhs, assign_rhs) = assign.split_once('=').context("expected assignment before break")?;
        anyhow::ensure!(
            assign_lhs.trim() == acc,
            "assignment target is not the accumulator"
        );
        let rhs = assign_rhs.trim();
        if init == "None" && rhs.starts_with("Some(") && rhs.ends_with(')') {
            let val = rhs[5..rhs.len() - 1].trim();
            Shape::Find { cond, value: val.to_string() }
        } else if init == "false" && rhs == "true" {
            Shape::Any { cond }
        } else if init == "true" && rhs == "false" {
            Shape::All { cond: format!("!({cond})") }
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        }
    } else {
        anyhow::ensure!(
            whole_word_count(body, "break") == 0,
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );

        // One statement, or one `if` holding one statement.
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = at_depth_zero(rest, 0, "{").context("the `if` has no body")?;
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (
                    Some(rest[..brace].trim().to_string()),
                    body[inner_open + 1..inner_close].trim(),
                )
            }
            None => (None, body),
        };
        anyhow::ensure!(
            stmt.matches(';').count() <= 1,
            "the loop's body has more than one statement"
        );
        let (pushes, value) = accumulation(stmt, &acc)
            .with_context(|| format!("the loop's body is not `{acc} += …;` or `{acc}.push(…);`"))?;
        if pushes {
            anyhow::ensure!(
                matches!(init, "Vec::new()" | "vec![]") || init.starts_with("Vec::with_capacity("),
                "`{acc}` does not start empty (`{init}`), so a collected vector would lose it"
            );
            Shape::Collect { cond, value }
        } else {
            anyhow::ensure!(
                is_zero(init),
                "`{acc}` starts at `{init}`, not zero, so a sum would lose it"
            );
            match cond {
                Some(cond) if value == "1" => Shape::Count { cond },
                cond => Shape::Sum { cond, value },
            }
        }
    };
    let indent: String = text[let_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    Ok(AccumulatorLoop {
        start: let_start,
        end: close + 1,
        indent,
        acc,
        declared,
        pattern,
        source,
        shape,
    })
}

/// What `for P in E` iterates, written as an iterator: `E.into_iter()`, `v.iter()` for `&v`,
/// and a range as it is.
pub fn iterator_of(source: &str, source_type: Option<&str>) -> String {
    let simple = |s: &str| !s.is_empty() && s.chars().all(|c| is_ident(c) || c == '.');
    if let Some(p) = source.strip_prefix("&mut ").filter(|p| simple(p)) {
        return format!("{p}.iter_mut()");
    }
    if let Some(p) = source.strip_prefix('&').filter(|p| simple(p)) {
        return format!("{p}.iter()");
    }
    // A name that holds a reference (`prices: &[u64]`): `into_iter` on it is `iter`, and clippy
    // says so (`into_iter_on_ref`).
    if simple(source) {
        match source_type {
            Some(t) if t.starts_with("&mut ") => return format!("{source}.iter_mut()"),
            Some(t) if t.starts_with('&') => return format!("{source}.iter()"),
            _ => {}
        }
    }
    if source.contains("..") {
        return format!("({source})");
    }
    if simple(source) || source.ends_with(')') && !source.contains(' ') {
        return format!("{source}.into_iter()");
    }
    format!("({source}).into_iter()")
}

/// The statement that replaces the `let` and the loop.
pub fn chain(l: &AccumulatorLoop, ty: &str, source_type: Option<&str>, mutable: bool) -> String {
    let p = &l.pattern;
    let simple = !p.is_empty() && p.chars().all(is_ident);
    let steps = match &l.shape {
        Shape::Sum { cond: None, value } if value == p => ".sum()".to_string(),
        Shape::Sum { cond: None, value } => format!(".map(|{p}| {value}).sum()"),
        Shape::Sum {
            cond: Some(c),
            value,
        } => {
            format!(".filter_map(|{p}| if {c} {{ Some({value}) }} else {{ None }}).sum()")
        }
        Shape::Count { cond } if simple => format!(".filter(|&{p}| {cond}).count()"),
        Shape::Count { cond } => {
            format!(".filter_map(|{p}| if {cond} {{ Some(()) }} else {{ None }}).count()")
        }
        Shape::Collect { cond: None, value } if value == p => ".collect()".to_string(),
        Shape::Collect { cond: None, value } => format!(".map(|{p}| {value}).collect()"),
        Shape::Collect {
            cond: Some(c),
            value,
        } => {
            format!(".filter_map(|{p}| if {c} {{ Some({value}) }} else {{ None }}).collect()")
        }
        Shape::Find { cond, value } if value == p && simple => {
            format!(".find(|&{p}| {cond})")
        }
        Shape::Find { cond, value } => {
            format!(".find_map(|{p}| if {cond} {{ Some({value}) }} else {{ None }})")
        }
        Shape::Any { cond } if simple => {
            format!(".any(|&{p}| {cond})")
        }
        Shape::Any { cond } => {
            format!(".any(|{p}| {cond})")
        }
        Shape::All { cond } if simple => {
            format!(".all(|&{p}| {cond})")
        }
        Shape::All { cond } => {
            format!(".all(|{p}| {cond})")
        }
    };
    format!(
        "{}let {}{}: {ty} = {}{steps};",
        l.indent,
        if mutable { "mut " } else { "" },
        l.acc,
        iterator_of(&l.source, source_type)
    )
}

/// The type in a hover over a binding: `let mut sum: u64` or `prices: &[u64]`.
pub fn binding_type(hover: &str) -> Option<String> {
    hover
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("```") && !l.is_empty())
        .find_map(|l| {
            let l = l.strip_prefix("let ").unwrap_or(l);
            let l = l.strip_prefix("mut ").unwrap_or(l);
            let (name, ty) = l.split_once(": ")?;
            name.chars()
                .all(is_ident)
                .then(|| ty.trim().trim_end_matches([',', ';']).to_string())
        })
}

/// The analyzer's type for the binding whose name starts at byte `at` of `text`.
async fn hover_type(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
) -> Option<String> {
    let (line, col) = crate::signature::line_col_at(text, at)?;
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let hover = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) }
        }),
    )
    .await
    .ok()?;
    binding_type(hover.pointer("/contents/value")?.as_str()?)
}

/// Rewrites the accumulator loop whose `for` is at `line`:`col` of `file`.
pub async fn loop_to_iterator(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<Rewritten> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at =
        crate::signature::offset_of(&text, line, col).context("the position is not in the file")?;
    let l = recognise(&text, at)?;

    // The type: declared, else the analyzer's for the accumulator; a vector may stay `Vec<_>`.
    let ty = match (&l.declared, &l.shape) {
        (Some(t), _) => t.clone(),
        (None, Shape::Collect { .. }) => "Vec<_>".to_string(),
        (None, Shape::Find { .. }) => "Option<_>".to_string(),
        (None, Shape::Any { .. } | Shape::All { .. }) => "bool".to_string(),
        (None, _) => {
            let name_at = l.start + text[l.start..].find(&l.acc).unwrap_or(0);
            hover_type(remote, root, file, &text, name_at)
                .await
                .with_context(|| format!("the analyzer gives no type for `{}`", l.acc))?
        }
    };
    // What the loop iterates, when it is a name: a reference is iterated with `iter()`.
    let source_at = text[l.start..l.end]
        .find(&format!(" in {}", l.source))
        .map(|i| l.start + i + 4);
    let source_type = match source_at {
        Some(at) if l.source.chars().all(|c| is_ident(c) || c == '.') => {
            let last = at + l.source.rfind('.').map_or(0, |i| i + 1);
            hover_type(remote, root, file, &text, last).await
        }
        _ => None,
    };
    if let Shape::Count { .. } = l.shape {
        anyhow::ensure!(
            ty == "usize",
            "`{}` is a `{ty}`, and `count()` gives a `usize`",
            l.acc
        );
    }
    if matches!(l.shape, Shape::Any { .. } | Shape::All { .. }) {
        anyhow::ensure!(
            ty == "bool",
            "`{}` is a `{ty}`, and predicate tests give a `bool`",
            l.acc
        );
    }
    if matches!(l.shape, Shape::Find { .. }) {
        anyhow::ensure!(
            ty.starts_with("Option<") || ty == "Option<_>",
            "`{}` is a `{ty}`, and find gives an `Option`",
            l.acc
        );
    }

    // Without `mut` first; the analyzer says when the variable is still changed afterwards.
    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string();
    let mut outcome = None;
    for mutable in [false, true] {
        let statement = chain(&l, &ty, source_type.as_deref(), mutable);
        let mut new_text = text.clone();
        new_text.replace_range(l.start..l.end, &statement);
        let reports = crate::diagnostics::validate_texts(
            remote,
            root,
            &[(file.to_path_buf(), new_text.clone())],
            &[],
        )
        .await?;
        let errors: Vec<&crate::diagnostics::DocDiagnostic> = reports
            .iter()
            .flat_map(|r| r.items.iter())
            .filter(|d| d.severity == "error")
            .collect();
        let needs_mut = errors
            .iter()
            .any(|d| matches!(d.code.as_deref(), Some("need-mut" | "E0596" | "E0384")));
        let diagnostics: Vec<String> = errors
            .iter()
            .map(|d| {
                format!(
                    "{}{} ({rel}:{}:{})",
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
        outcome = Some((statement, new_text, diagnostics));
        if !needs_mut {
            break;
        }
    }
    let (statement, new_text, diagnostics) = outcome.context("no attempt was made")?;
    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let files: BTreeMap<PathBuf, String> =
            std::iter::once((file.to_path_buf(), new_text.clone())).collect();
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }
    Ok(Rewritten {
        statement,
        root: root.to_path_buf(),
        file: rel,
        new_text,
        diagnostics,
        applied,
    })
}

/// Recognises a TypeScript / JavaScript loop replacement.
pub fn recognise_ts(text: &str, at: usize) -> Result<PolyglotLoop> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);

    let for_at = if let Some(idx) = text[line_start..line_end].find("for ") {
        line_start + idx
    } else if let Some(idx) = text[at..].find("for ") {
        at + idx
    } else if let Some(idx) = text[..at].rfind("for ") {
        idx
    } else {
        anyhow::bail!("no `for` loop found at or near this position");
    };

    let for_line_start = text[..for_at].rfind('\n').map_or(0, |i| i + 1);
    let before_for = text[for_line_start..for_at].trim();
    anyhow::ensure!(!before_for.ends_with("await"), "the loop's header awaits");

    let open_paren = at_depth_zero(text, for_at + 4, "(").context("the `for` has no `(`")?;
    let close_paren = crate::parameter_object::matching_bracket(text, open_paren)
        .context("the `for` header `(...)` is not closed")?;
    let header = text[open_paren + 1..close_paren].trim();
    anyhow::ensure!(
        header.contains(" of "),
        "only `for...of` loops over collections are supported"
    );
    let (lhs, source) = header.split_once(" of ").context("expected `of` in for-of loop")?;
    let source = source.trim().to_string();
    let pattern = lhs
        .strip_prefix("const ")
        .or_else(|| lhs.strip_prefix("let "))
        .or_else(|| lhs.strip_prefix("var "))
        .unwrap_or(lhs)
        .trim()
        .to_string();

    let open_brace = at_depth_zero(text, close_paren + 1, "{").context("the loop has no body `{`")?;
    let close_brace = crate::parameter_object::matching_bracket(text, open_brace)
        .context("the loop's body is not closed")?;
    let body = text[open_brace + 1..close_brace].trim();

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let dec_trimmed = dec_line.trim_end_matches(';').trim();
    let after_kw = dec_trimmed
        .strip_prefix("let ")
        .or_else(|| dec_trimmed.strip_prefix("const "))
        .or_else(|| dec_trimmed.strip_prefix("var "))
        .context("the statement above the loop is not a variable declaration (`let`/`const`/`var`)")?;
    let (lhs_dec, init) = after_kw.split_once('=').context("the accumulator has no initial value")?;
    let acc = match lhs_dec.split_once(':') {
        Some((n, _)) => n.trim().to_string(),
        None => lhs_dec.trim().to_string(),
    };
    anyhow::ensure!(
        !acc.is_empty() && acc.chars().all(is_ident),
        "`{lhs_dec}` is not a single variable"
    );
    let init = init.trim();

    for word in ["continue", "return", "throw", "yield"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }
    anyhow::ensure!(!body.contains("await "), "the loop's body can await");

    let shape = if whole_word_count(body, "break") == 1 {
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let rest_trim = rest.trim_start();
                let paren_open = rest_trim.find('(').context("the `if` has no `(`")?;
                let paren_close = crate::parameter_object::matching_bracket(rest_trim, paren_open)
                    .context("the `if` condition is not closed")?;
                let cond_str = rest_trim[paren_open + 1..paren_close].trim().to_string();
                let after_paren = rest_trim[paren_close + 1..].trim_start();
                let brace = after_paren.find('{').context("the `if` has no body")?;
                let inner_open = body.len() - after_paren.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (cond_str, body[inner_open + 1..inner_close].trim())
            }
            None => anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express"),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        let parts: Vec<&str> = stmt_trimmed.split([';', '\n']).map(str::trim).filter(|s| !s.is_empty()).collect();
        anyhow::ensure!(
            parts.len() == 2 && parts[1] == "break",
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        let assign = parts[0];
        let (assign_lhs, assign_rhs) = assign.split_once('=').context("expected assignment before break")?;
        anyhow::ensure!(
            assign_lhs.trim() == acc,
            "assignment target is not the accumulator"
        );
        let rhs = assign_rhs.trim();
        if init == "null" || init == "undefined" {
            Shape::Find { cond, value: rhs.to_string() }
        } else if init == "false" && rhs == "true" {
            Shape::Any { cond }
        } else if init == "true" && rhs == "false" {
            let cond_norm = if let Some(inner) = cond.strip_prefix('!') {
                inner.trim().to_string()
            } else {
                format!("!({cond})")
            };
            Shape::All { cond: cond_norm }
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        }
    } else {
        anyhow::ensure!(
            whole_word_count(body, "break") == 0,
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let rest_trim = rest.trim_start();
                let paren_open = rest_trim.find('(').context("the `if` has no `(`")?;
                let paren_close = crate::parameter_object::matching_bracket(rest_trim, paren_open)
                    .context("the `if` condition is not closed")?;
                let cond_str = rest_trim[paren_open + 1..paren_close].trim().to_string();
                let after_paren = rest_trim[paren_close + 1..].trim_start();
                let brace = after_paren.find('{').context("the `if` has no body")?;
                let inner_open = body.len() - after_paren.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (Some(cond_str), body[inner_open + 1..inner_close].trim())
            }
            None => (None, body),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        if let Some(rest) = stmt_trimmed.strip_prefix(&acc) {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix("+=") {
                let v = val.trim();
                anyhow::ensure!(is_zero(init), "`{acc}` starts at `{init}`, not zero, so a sum would lose it");
                if let Some(c) = cond {
                    if v == "1" {
                        Shape::Count { cond: c }
                    } else {
                        Shape::Sum { cond: Some(c), value: v.to_string() }
                    }
                } else {
                    Shape::Sum { cond: None, value: v.to_string() }
                }
            } else if rest == "++" {
                anyhow::ensure!(is_zero(init), "`{acc}` starts at `{init}`, not zero, so a count would lose it");
                let c = cond.unwrap_or_else(|| "true".to_string());
                Shape::Count { cond: c }
            } else if let Some(args) = rest.strip_prefix(".push(") {
                let v = args.strip_suffix(')').context("malformed push call")?;
                anyhow::ensure!(
                    init == "[]" || init == "new Array()" || init == "Array()",
                    "`{acc}` does not start empty (`{init}`)"
                );
                Shape::Collect { cond, value: v.trim().to_string() }
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …;` or `{acc}.push(…);`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …;` or `{acc}.push(…);`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } if value == pattern => {
            format!("const {acc} = {source}.reduce((acc, {pattern}) => acc + {pattern}, 0);")
        }
        Shape::Sum { cond: None, value } => {
            format!("const {acc} = {source}.reduce((acc, {pattern}) => acc + ({value}), 0);")
        }
        Shape::Sum { cond: Some(c), value } => {
            format!("const {acc} = {source}.filter({pattern} => {c}).reduce((acc, {pattern}) => acc + ({value}), 0);")
        }
        Shape::Count { cond } => {
            format!("const {acc} = {source}.filter({pattern} => {cond}).length;")
        }
        Shape::Collect { cond: None, value } if value == pattern => {
            format!("const {acc} = {source}.map({pattern} => {pattern});")
        }
        Shape::Collect { cond: None, value } => {
            format!("const {acc} = {source}.map({pattern} => {value});")
        }
        Shape::Collect { cond: Some(c), value } => {
            format!("const {acc} = {source}.filter({pattern} => {c}).map({pattern} => {value});")
        }
        Shape::Find { cond, value } if value == pattern => {
            format!("const {acc} = {source}.find({pattern} => {cond}) ?? null;")
        }
        Shape::Find { cond, value } => {
            format!(
                "const {acc} = (() => {{ let matched = false; const found = {source}.find({pattern} => {{ const yes = {cond}; if (yes) matched = true; return yes; }}); return matched ? [found].map({pattern} => {value})[0] ?? null : null; }})();"
            )
        }
        Shape::Any { cond } => {
            format!("const {acc} = {source}.some({pattern} => {cond});")
        }
        Shape::All { cond } => {
            format!("const {acc} = {source}.every({pattern} => {cond});")
        }
    };

    let indent: String = text[dec_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let replacement = format!("{indent}{statement}");

    Ok(PolyglotLoop {
        start: dec_start,
        end: close_brace + 1,
        indent,
        replacement,
        statement,
    })
}

/// Recognises a Python loop replacement.
pub fn recognise_python(text: &str, at: usize) -> Result<PolyglotLoop> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);

    let for_at = if let Some(idx) = text[line_start..line_end].find("for ") {
        line_start + idx
    } else if let Some(idx) = text[at..].find("for ") {
        at + idx
    } else if let Some(idx) = text[..at].rfind("for ") {
        idx
    } else {
        anyhow::bail!("no `for` loop found at or near this position");
    };

    let for_line_start = text[..for_at].rfind('\n').map_or(0, |i| i + 1);
    let for_line_end = text[for_at..].find('\n').map_or(text.len(), |i| for_at + i);
    let for_line = text[for_line_start..for_line_end].trim();

    let for_indent: String = text[for_line_start..for_at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();

    let for_header = for_line.strip_prefix("for ").context("expected `for `")?;
    let for_header = for_header.strip_suffix(':').context("expected `:` at end of for line")?.trim();
    let (pattern, source) = for_header.split_once(" in ").context("expected `in` in for loop")?;
    let pattern = pattern.trim().to_string();
    let source = source.trim().to_string();

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let (lhs_dec, init) = dec_line.split_once('=').context("the accumulator has no initial value")?;
    let acc = match lhs_dec.split_once(':') {
        Some((n, _)) => n.trim().to_string(),
        None => lhs_dec.trim().to_string(),
    };
    anyhow::ensure!(
        !acc.is_empty() && acc.chars().all(is_ident),
        "`{lhs_dec}` is not a single variable"
    );
    let init = init.trim();

    let rest = &text[for_line_end..];
    let mut body_end = for_line_end;
    let mut body_lines = Vec::new();
    let mut current_offset = for_line_end;

    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            current_offset += line.len();
            continue;
        }
        let line_indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t') .collect();
        if line_indent.len() > for_indent.len() && line.starts_with(&for_indent) {
            body_lines.push(line.trim());
            current_offset += line.len();
            body_end = current_offset;
        } else {
            break;
        }
    }

    anyhow::ensure!(!body_lines.is_empty(), "the loop has no body");
    let body = body_lines.join("\n");

    for word in ["continue", "return", "raise", "yield"] {
        anyhow::ensure!(
            whole_word_count(&body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }
    anyhow::ensure!(!body.contains("await "), "the loop's body can await");

    let shape = if whole_word_count(&body, "break") == 1 {
        anyhow::ensure!(
            whole_word_count(&body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, rhs) = if let Some(rest) = body.strip_prefix("if ") {
            let (cond_part, action_part) = if let Some((c, a)) = rest.split_once(':') {
                (c.trim().to_string(), a.trim())
            } else {
                anyhow::bail!("malformed if statement in python loop");
            };
            let action_lines: Vec<&str> = action_part.split(['\n', ';']).map(str::trim).filter(|s| !s.is_empty()).collect();
            anyhow::ensure!(action_lines.len() == 2 && action_lines[1] == "break", "expected assign and break");
            let (assign_lhs, assign_rhs) = action_lines[0].split_once('=').context("expected assignment")?;
            anyhow::ensure!(assign_lhs.trim() == acc, "assignment target is not the accumulator");
            (cond_part, assign_rhs.trim())
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        };

        if init == "None" {
            Shape::Find { cond, value: rhs.to_string() }
        } else if init == "False" && rhs == "True" {
            Shape::Any { cond }
        } else if init == "True" && rhs == "False" {
            let cond_norm = if let Some(inner) = cond.strip_prefix("not ") {
                inner.trim().to_string()
            } else {
                format!("not ({cond})")
            };
            Shape::All { cond: cond_norm }
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        }
    } else {
        anyhow::ensure!(
            whole_word_count(&body, "break") == 0,
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(&body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = if let Some(rest) = body.strip_prefix("if ") {
            let (c, a) = rest.split_once(':').context("expected `:` in if statement")?;
            (Some(c.trim().to_string()), a.trim())
        } else {
            (None, body.as_str())
        };

        let stmt_trimmed = stmt.trim();
        if let Some(rest) = stmt_trimmed.strip_prefix(&acc) {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix("+=") {
                let v = val.trim();
                anyhow::ensure!(is_zero(init), "`{acc}` starts at `{init}`, not zero, so a sum would lose it");
                if let Some(c) = cond {
                    if v == "1" {
                        Shape::Count { cond: c }
                    } else {
                        Shape::Sum { cond: Some(c), value: v.to_string() }
                    }
                } else {
                    Shape::Sum { cond: None, value: v.to_string() }
                }
            } else if let Some(args) = rest.strip_prefix(".append(") {
                let v = args.strip_suffix(')').context("malformed append call")?;
                anyhow::ensure!(init == "[]" || init == "list()", "`{acc}` does not start empty (`{init}`)");
                Shape::Collect { cond, value: v.trim().to_string() }
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …` or `{acc}.append(…)`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …` or `{acc}.append(…)`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } if value == pattern => {
            format!("{acc} = sum({source})")
        }
        Shape::Sum { cond: None, value } => {
            format!("{acc} = sum({value} for {pattern} in {source})")
        }
        Shape::Sum { cond: Some(c), value } => {
            format!("{acc} = sum({value} for {pattern} in {source} if {c})")
        }
        Shape::Count { cond } => {
            format!("{acc} = sum(1 for {pattern} in {source} if {cond})")
        }
        Shape::Collect { cond: None, value } => {
            format!("{acc} = [{value} for {pattern} in {source}]")
        }
        Shape::Collect { cond: Some(c), value } => {
            format!("{acc} = [{value} for {pattern} in {source} if {c}]")
        }
        Shape::Find { cond, value } => {
            format!("{acc} = next(({value} for {pattern} in {source} if {cond}), None)")
        }
        Shape::Any { cond } => {
            format!("{acc} = any({cond} for {pattern} in {source})")
        }
        Shape::All { cond } => {
            format!("{acc} = all({cond} for {pattern} in {source})")
        }
    };

    let indent: String = text[dec_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let replacement = format!("{indent}{statement}");

    Ok(PolyglotLoop {
        start: dec_start,
        end: body_end,
        indent,
        replacement,
        statement,
    })
}

/// Recognises a Swift loop replacement.
pub fn recognise_swift(text: &str, at: usize) -> Result<PolyglotLoop> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);

    let for_at = if let Some(idx) = text[line_start..line_end].find("for ") {
        line_start + idx
    } else if let Some(idx) = text[at..].find("for ") {
        at + idx
    } else if let Some(idx) = text[..at].rfind("for ") {
        idx
    } else {
        anyhow::bail!("no `for` loop found at or near this position");
    };

    let for_line_start = text[..for_at].rfind('\n').map_or(0, |i| i + 1);
    let open_brace = at_depth_zero(text, for_at + 4, "{").context("the loop has no body `{`")?;
    let header = text[for_at + 4..open_brace].trim();
    let (pattern, source) = header.split_once(" in ").context("expected `in` in for loop")?;
    let pattern = pattern.trim().to_string();
    let source = source.trim().to_string();

    let close_brace = crate::parameter_object::matching_bracket(text, open_brace)
        .context("the loop's body is not closed")?;
    let body = text[open_brace + 1..close_brace].trim();

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let dec_trimmed = dec_line.trim_end_matches(';').trim();
    let after_var = dec_trimmed.strip_prefix("var ").context("the statement above the loop is not `var <acc> = …`")?;
    let (lhs_dec, init) = after_var.split_once('=').context("the accumulator has no initial value")?;
    let acc = match lhs_dec.split_once(':') {
        Some((n, _)) => n.trim().to_string(),
        None => lhs_dec.trim().to_string(),
    };
    anyhow::ensure!(
        !acc.is_empty() && acc.chars().all(is_ident),
        "`{lhs_dec}` is not a single variable"
    );
    let init = init.trim();

    for word in ["continue", "return", "throw"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }
    anyhow::ensure!(!body.contains("await "), "the loop's body can await");

    let shape = if whole_word_count(body, "break") == 1 {
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = rest.find('{').context("the `if` has no body")?;
                let cond_str = rest[..brace].trim().to_string();
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (cond_str, body[inner_open + 1..inner_close].trim())
            }
            None => anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express"),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        let parts: Vec<&str> = stmt_trimmed.split([';', '\n']).map(str::trim).filter(|s| !s.is_empty()).collect();
        anyhow::ensure!(
            parts.len() == 2 && parts[1] == "break",
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        let (assign_lhs, assign_rhs) = parts[0].split_once('=').context("expected assignment before break")?;
        anyhow::ensure!(assign_lhs.trim() == acc, "assignment target is not the accumulator");
        let rhs = assign_rhs.trim();
        if init == "nil" {
            Shape::Find { cond, value: rhs.to_string() }
        } else if init == "false" && rhs == "true" {
            Shape::Any { cond }
        } else if init == "true" && rhs == "false" {
            let cond_norm = if let Some(inner) = cond.strip_prefix('!') {
                inner.trim().to_string()
            } else {
                format!("!({cond})")
            };
            Shape::All { cond: cond_norm }
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        }
    } else {
        anyhow::ensure!(
            whole_word_count(body, "break") == 0,
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = rest.find('{').context("the `if` has no body")?;
                let cond_str = rest[..brace].trim().to_string();
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (Some(cond_str), body[inner_open + 1..inner_close].trim())
            }
            None => (None, body),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        if let Some(rest) = stmt_trimmed.strip_prefix(&acc) {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix("+=") {
                let v = val.trim();
                anyhow::ensure!(is_zero(init), "`{acc}` starts at `{init}`, not zero, so a sum would lose it");
                if let Some(c) = cond {
                    if v == "1" {
                        Shape::Count { cond: c }
                    } else {
                        Shape::Sum { cond: Some(c), value: v.to_string() }
                    }
                } else {
                    Shape::Sum { cond: None, value: v.to_string() }
                }
            } else if let Some(args) = rest.strip_prefix(".append(") {
                let v = args.strip_suffix(')').context("malformed append call")?;
                anyhow::ensure!(
                    init == "[]" || init.ends_with("()") || init.ends_with("[]"),
                    "`{acc}` does not start empty (`{init}`)"
                );
                Shape::Collect { cond, value: v.trim().to_string() }
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …` or `{acc}.append(…)`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …` or `{acc}.append(…)`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } if value == pattern => {
            format!("let {acc} = {source}.reduce(0, +)")
        }
        Shape::Sum { cond: None, value } => {
            format!("let {acc} = {source}.reduce(0) {{ _acc, {pattern} in _acc + ({value}) }}")
        }
        Shape::Sum { cond: Some(c), value } => {
            format!("let {acc} = {source}.filter {{ {pattern} in {c} }}.reduce(0) {{ _acc, {pattern} in _acc + ({value}) }}")
        }
        Shape::Count { cond } => {
            format!("let {acc} = {source}.filter {{ {pattern} in {cond} }}.count")
        }
        Shape::Collect { cond: None, value } if value == pattern => {
            format!("let {acc} = {source}.map {{ {pattern} }}")
        }
        Shape::Collect { cond: None, value } => {
            format!("let {acc} = {source}.map {{ {pattern} in {value} }}")
        }
        Shape::Collect { cond: Some(c), value } => {
            format!("let {acc} = {source}.filter {{ {pattern} in {c} }}.map {{ {pattern} in {value} }}")
        }
        Shape::Find { cond, value } if value == pattern => {
            format!("let {acc} = {source}.first(where: {{ {pattern} in {cond} }})")
        }
        Shape::Find { cond, value } => {
            format!("let {acc} = {source}.first(where: {{ {pattern} in {cond} }}).map {{ {pattern} in {value} }}")
        }
        Shape::Any { cond } => {
            format!("let {acc} = {source}.contains(where: {{ {pattern} in {cond} }})")
        }
        Shape::All { cond } => {
            format!("let {acc} = {source}.allSatisfy {{ {pattern} in {cond} }}")
        }
    };

    let indent: String = text[dec_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let replacement = format!("{indent}{statement}");

    Ok(PolyglotLoop {
        start: dec_start,
        end: close_brace + 1,
        indent,
        replacement,
        statement,
    })
}

/// Recognises a C++ loop replacement.
pub fn recognise_cpp(text: &str, at: usize) -> Result<PolyglotLoop> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);

    let for_at = if let Some(idx) = text[line_start..line_end].find("for ") {
        line_start + idx
    } else if let Some(idx) = text[at..].find("for ") {
        at + idx
    } else if let Some(idx) = text[..at].rfind("for ") {
        idx
    } else {
        anyhow::bail!("no `for` loop found at or near this position");
    };

    let for_line_start = text[..for_at].rfind('\n').map_or(0, |i| i + 1);
    let open_paren = at_depth_zero(text, for_at + 4, "(").context("the `for` has no `(`")?;
    let close_paren = crate::parameter_object::matching_bracket(text, open_paren)
        .context("the `for` header `(...)` is not closed")?;
    let header = text[open_paren + 1..close_paren].trim();
    let (decl, source) = header.split_once(':').context("expected `:` in range-for loop")?;
    let source = source.trim().to_string();
    let pattern = decl
        .rsplit(|c: char| !is_ident(c))
        .find(|s| !s.is_empty())
        .context("could not extract loop variable")?
        .to_string();

    let open_brace = at_depth_zero(text, close_paren + 1, "{").context("the loop has no body `{`")?;
    let close_brace = crate::parameter_object::matching_bracket(text, open_brace)
        .context("the loop's body is not closed")?;
    let body = text[open_brace + 1..close_brace].trim();

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let dec_trimmed = dec_line.trim_end_matches(';').trim();
    let (lhs_dec, init) = dec_trimmed.split_once('=').context("the accumulator has no initial value")?;
    let acc = lhs_dec
        .rsplit(|c: char| !is_ident(c))
        .find(|s| !s.is_empty())
        .context("could not extract accumulator variable")?
        .to_string();
    let accumulator_decl = lhs_dec.trim().to_string();
    let init = init.trim();
    anyhow::ensure!(
        !source.starts_with('{'),
        "a braced range initializer cannot be safely stored for a single-evaluation iterator conversion"
    );
    let mut range_name = "__prod_code_range".to_string();
    let mut suffix = 0usize;
    while text.contains(&range_name) {
        suffix += 1;
        range_name = format!("__prod_code_range_{suffix}");
    }
    let raw_acc_type = accumulator_decl
        .strip_suffix(&acc)
        .unwrap_or_default()
        .trim();
    let acc_type = raw_acc_type
        .strip_prefix("const ")
        .or_else(|| raw_acc_type.strip_prefix("volatile "))
        .unwrap_or(raw_acc_type)
        .trim();
    anyhow::ensure!(
        !acc_type.contains('&'),
        "a reference accumulator cannot be represented safely by std::accumulate"
    );
    let sum_initial = if acc_type.is_empty() || acc_type == "auto" || acc_type == "decltype(auto)" {
        init.to_string()
    } else {
        format!("static_cast<{acc_type}>({init})")
    };

    for word in ["continue", "return", "throw"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }

    let shape = if whole_word_count(body, "break") == 1 {
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let rest_trim = rest.trim_start();
                let paren_open = rest_trim.find('(').context("the `if` has no `(`")?;
                let paren_close = crate::parameter_object::matching_bracket(rest_trim, paren_open)
                    .context("the `if` condition is not closed")?;
                let cond_str = rest_trim[paren_open + 1..paren_close].trim().to_string();
                let after_paren = rest_trim[paren_close + 1..].trim_start();
                let brace = after_paren.find('{').context("the `if` has no body")?;
                let inner_open = body.len() - after_paren.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (cond_str, body[inner_open + 1..inner_close].trim())
            }
            None => anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express"),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        let parts: Vec<&str> = stmt_trimmed.split([';', '\n']).map(str::trim).filter(|s| !s.is_empty()).collect();
        anyhow::ensure!(
            parts.len() == 2 && parts[1] == "break",
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        let (assign_lhs, assign_rhs) = parts[0].split_once('=').context("expected assignment before break")?;
        anyhow::ensure!(assign_lhs.trim() == acc, "assignment target is not the accumulator");
        let rhs = assign_rhs.trim();
        if init == "false" && rhs == "true" {
            Shape::Any { cond }
        } else if init == "true" && rhs == "false" {
            let cond_norm = if let Some(inner) = cond.strip_prefix('!') {
                inner.trim().to_string()
            } else {
                format!("!({cond})")
            };
            Shape::All { cond: cond_norm }
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        }
    } else {
        anyhow::ensure!(
            whole_word_count(body, "break") == 0,
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let rest_trim = rest.trim_start();
                let paren_open = rest_trim.find('(').context("the `if` has no `(`")?;
                let paren_close = crate::parameter_object::matching_bracket(rest_trim, paren_open)
                    .context("the `if` condition is not closed")?;
                let cond_str = rest_trim[paren_open + 1..paren_close].trim().to_string();
                let after_paren = rest_trim[paren_close + 1..].trim_start();
                let brace = after_paren.find('{').context("the `if` has no body")?;
                let inner_open = body.len() - after_paren.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (Some(cond_str), body[inner_open + 1..inner_close].trim())
            }
            None => (None, body),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        if let Some(rest) = stmt_trimmed.strip_prefix(&acc) {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix("+=") {
                let v = val.trim();
                anyhow::ensure!(is_zero(init), "`{acc}` starts at `{init}`, not zero, so a sum would lose it");
                if let Some(c) = cond {
                    if v == "1" {
                        Shape::Count { cond: c }
                    } else {
                        Shape::Sum { cond: Some(c), value: v.to_string() }
                    }
                } else {
                    Shape::Sum { cond: None, value: v.to_string() }
                }
            } else if rest == "++" {
                anyhow::ensure!(is_zero(init), "`{acc}` starts at `{init}`, not zero, so a count would lose it");
                let c = cond.unwrap_or_else(|| "true".to_string());
                Shape::Count { cond: c }
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …;`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …;`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } if value == pattern => {
            format!("{accumulator_decl} = std::accumulate({range_name}.begin(), {range_name}.end(), {sum_initial});")
        }
        Shape::Sum { cond: None, value } => {
            format!("{accumulator_decl} = std::accumulate({range_name}.begin(), {range_name}.end(), {sum_initial}, [](auto _acc, const auto& {pattern}) {{ return _acc + ({value}); }});")
        }
        Shape::Sum { cond: Some(c), value } => {
            format!("{accumulator_decl} = std::accumulate({range_name}.begin(), {range_name}.end(), {sum_initial}, [](auto _acc, const auto& {pattern}) {{ return ({c}) ? _acc + ({value}) : _acc; }});")
        }
        Shape::Count { cond } => {
            format!("const auto {acc} = std::count_if({range_name}.begin(), {range_name}.end(), [](const auto& {pattern}) {{ return {cond}; }});")
        }
        Shape::Any { cond } => {
            format!("const bool {acc} = std::any_of({range_name}.begin(), {range_name}.end(), [](const auto& {pattern}) {{ return {cond}; }});")
        }
        Shape::All { cond } => {
            format!("const bool {acc} = std::all_of({range_name}.begin(), {range_name}.end(), [](const auto& {pattern}) {{ return {cond}; }});")
        }
        _ => anyhow::bail!("unsupported shape for C++"),
    };

    let indent: String = text[dec_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let replacement = format!("{indent}auto&& {range_name} = ({source});\n{indent}{statement}");

    Ok(PolyglotLoop {
        start: dec_start,
        end: close_brace + 1,
        indent,
        replacement,
        statement,
    })
}

/// Recognises a Go loop replacement.
pub fn recognise_go(text: &str, at: usize) -> Result<PolyglotLoop> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);

    let for_at = if let Some(idx) = text[line_start..line_end].find("for ") {
        line_start + idx
    } else if let Some(idx) = text[at..].find("for ") {
        at + idx
    } else if let Some(idx) = text[..at].rfind("for ") {
        idx
    } else {
        anyhow::bail!("no `for` loop found at or near this position");
    };

    let for_line_start = text[..for_at].rfind('\n').map_or(0, |i| i + 1);
    let open_brace = at_depth_zero(text, for_at + 4, "{").context("the loop has no body `{`")?;
    let header = text[for_at + 4..open_brace].trim();
    let (lhs, source) = header.split_once(" range ").context("expected `range` in for loop")?;
    let source = source.trim().to_string();
    let range_vars: Vec<&str> = lhs.split(',').map(str::trim).collect();
    anyhow::ensure!(
        (1..=2).contains(&range_vars.len()) && !range_vars[0].is_empty(),
        "Go range header must name one or two variables"
    );
    let pattern = if range_vars.len() == 2 {
        range_vars[1].to_string()
    } else {
        range_vars[0].to_string()
    };
    let pattern = pattern.strip_suffix(":=").unwrap_or(&pattern).trim().to_string();

    let close_brace = crate::parameter_object::matching_bracket(text, open_brace)
        .context("the loop's body is not closed")?;
    let body = text[open_brace + 1..close_brace].trim();
    if range_vars.len() == 2 && range_vars[0] != "_" {
        anyhow::ensure!(
            whole_word_count(body, range_vars[0]) == 0,
            "the loop body uses range index/key `{}`, which the iterator conversion cannot preserve",
            range_vars[0]
        );
    }
    let range_binding = if range_vars.len() == 2 {
        format!("_, {pattern}")
    } else {
        pattern.clone()
    };

    let before_loop = text[..for_line_start].trim_end_matches(['\n', ' ', '\t']);
    let dec_start = before_loop.rfind('\n').map_or(0, |i| i + 1);
    let dec_line = before_loop[dec_start..].trim();
    let dec_trimmed = dec_line.trim_end_matches(';').trim();
    let (acc, init) = if let Some((a, i)) = dec_trimmed.split_once(":=") {
        (a.trim().to_string(), i.trim())
    } else if let Some(rest) = dec_trimmed.strip_prefix("var ") {
        if let Some((lhs, i)) = rest.split_once('=') {
            let a = lhs.split_whitespace().next().unwrap_or(lhs).trim();
            (a.to_string(), i.trim())
        } else {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            anyhow::ensure!(!parts.is_empty(), "malformed var declaration");
            (parts[0].to_string(), "0")
        }
    } else {
        anyhow::bail!("the statement above the loop is not a variable declaration");
    };
    anyhow::ensure!(
        !acc.is_empty() && acc.chars().all(is_ident),
        "`{acc}` is not a single variable"
    );

    for word in ["continue", "return", "panic"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }

    let shape = if whole_word_count(body, "break") == 1 {
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = rest.find('{').context("the `if` has no body")?;
                let cond_str = rest[..brace].trim().to_string();
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (cond_str, body[inner_open + 1..inner_close].trim())
            }
            None => anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express"),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        let parts: Vec<&str> = stmt_trimmed.split([';', '\n']).map(str::trim).filter(|s| !s.is_empty()).collect();
        anyhow::ensure!(
            parts.len() == 2 && parts[1] == "break",
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        let (assign_lhs, assign_rhs) = parts[0].split_once('=').context("expected assignment before break")?;
        anyhow::ensure!(assign_lhs.trim() == acc, "assignment target is not the accumulator");
        let rhs = assign_rhs.trim();
        if init == "false" && rhs == "true" {
            Shape::Any { cond }
        } else if init == "true" && rhs == "false" {
            let cond_norm = if let Some(inner) = cond.strip_prefix('!') {
                inner.trim().to_string()
            } else {
                format!("!({cond})")
            };
            Shape::All { cond: cond_norm }
        } else {
            anyhow::bail!("the loop's body has `break`, which an iterator chain cannot express");
        }
    } else {
        anyhow::ensure!(
            whole_word_count(body, "break") == 0,
            "the loop's body has `break`, which an iterator chain cannot express"
        );
        anyhow::ensure!(
            whole_word_count(body, &acc) == 1,
            "the loop's body uses `{acc}` for more than the one accumulating statement"
        );
        let (cond, stmt) = match body.strip_prefix("if ") {
            Some(rest) => {
                let brace = rest.find('{').context("the `if` has no body")?;
                let cond_str = rest[..brace].trim().to_string();
                let inner_open = body.len() - rest.len() + brace;
                let inner_close = crate::parameter_object::matching_bracket(body, inner_open)
                    .context("the `if` is not closed")?;
                anyhow::ensure!(
                    body[inner_close + 1..].trim().is_empty(),
                    "the `if` has an `else` or is followed by more statements"
                );
                (Some(cond_str), body[inner_open + 1..inner_close].trim())
            }
            None => (None, body),
        };
        let stmt_trimmed = stmt.trim_end_matches(';').trim();
        if let Some(rest) = stmt_trimmed.strip_prefix(&acc) {
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix("+=") {
                let v = val.trim();
                anyhow::ensure!(is_zero(init), "`{acc}` starts at `{init}`, not zero, so a sum would lose it");
                if let Some(c) = cond {
                    if v == "1" {
                        Shape::Count { cond: c }
                    } else {
                        Shape::Sum { cond: Some(c), value: v.to_string() }
                    }
                } else {
                    Shape::Sum { cond: None, value: v.to_string() }
                }
            } else if rest == "++" {
                anyhow::ensure!(is_zero(init), "`{acc}` starts at `{init}`, not zero, so a count would lose it");
                let c = cond.unwrap_or_else(|| "true".to_string());
                Shape::Count { cond: c }
            } else if let Some(args) = rest.strip_prefix("=") {
                let args = args.trim();
                if let Some(app) = args.strip_prefix("append(") {
                    let v = app.strip_suffix(')').context("malformed append call")?;
                    let (target, item) = v.split_once(',').context("expected slice, item in append")?;
                    anyhow::ensure!(target.trim() == acc, "append target is not accumulator");
                    Shape::Collect { cond, value: item.trim().to_string() }
                } else {
                    anyhow::bail!("unsupported assignment in Go loop");
                }
            } else {
                anyhow::bail!("the loop's body is not `{acc} += …` or `{acc} = append(…)`");
            }
        } else {
            anyhow::bail!("the loop's body is not `{acc} += …` or `{acc} = append(…)`");
        }
    };

    let statement = match shape {
        Shape::Sum { cond: None, value } => {
            format!("{acc} := func() int {{ s := 0; for {range_binding} := range {source} {{ s += {value} }}; return s }}()")
        }
        Shape::Sum { cond: Some(c), value } => {
            format!("{acc} := func() int {{ s := 0; for {range_binding} := range {source} {{ if {c} {{ s += {value} }} }}; return s }}()")
        }
        Shape::Count { cond } => {
            format!("{acc} := func() int {{ c := 0; for {range_binding} := range {source} {{ if {cond} {{ c++ }} }}; return c }}()")
        }
        Shape::Collect { cond: None, value } => {
            format!("{acc} := func() []interface{{}} {{ res := make([]interface{{}}, 0); for {range_binding} := range {source} {{ res = append(res, {value}) }}; return res }}()")
        }
        Shape::Collect { cond: Some(c), value } => {
            format!("{acc} := func() []interface{{}} {{ res := make([]interface{{}}, 0); for {range_binding} := range {source} {{ if {c} {{ res = append(res, {value}) }} }}; return res }}()")
        }
        Shape::Any { cond } => {
            format!("{acc} := func() bool {{ for {range_binding} := range {source} {{ if {cond} {{ return true }} }}; return false }}()")
        }
        Shape::All { cond } => {
            format!("{acc} := func() bool {{ for {range_binding} := range {source} {{ if !({cond}) {{ return false }} }}; return true }}()")
        }
        _ => anyhow::bail!("unsupported shape for Go"),
    };

    let indent: String = text[dec_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let replacement = format!("{indent}{statement}");

    Ok(PolyglotLoop {
        start: dec_start,
        end: close_brace + 1,
        indent,
        replacement,
        statement,
    })
}

/// Finds the offset of the target loop given either line/col or symbol name.
pub fn find_loop_offset(
    text: &str,
    _lang: Language,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<usize> {
    if let Some(l) = line {
        let c = col.unwrap_or(1);
        if let Some(offset) = crate::signature::offset_of(text, l, c) {
            return Ok(offset);
        }
    }
    let sym = symbol.context("missing `line` or `symbol` identifying the loop")?;
    // 1. Look for function declaration containing `sym`
    for pattern in [
        format!("fn {sym}"),
        format!("def {sym}"),
        format!("func {sym}"),
        format!("function {sym}"),
        format!("{sym}("),
    ] {
        if let Some(for_idx) = text.find(&pattern).and_then(|idx| text[idx..].find("for ").map(|f| idx + f)) {
            return Ok(for_idx);
        }
    }
    // 2. Look for accumulator or loop variable `sym`
    for pattern in [
        format!("let mut {sym}"),
        format!("let {sym}"),
        format!("var {sym}"),
        format!("const {sym}"),
        format!("{sym} ="),
        format!("{sym} :="),
    ] {
        if let Some(for_idx) = text.find(&pattern).and_then(|idx| text[idx..].find("for ").map(|f| idx + f)) {
            return Ok(for_idx);
        }
    }
    anyhow::bail!("could not find loop for symbol `{sym}`")
}

/// Polyglot entry point for converting an accumulating loop into an iterator chain or functional expression.
#[allow(clippy::too_many_arguments)]
pub async fn loop_to_iterator_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    apply: bool,
    force: bool,
) -> Result<Rewritten> {
    let lang = crate::parameter_object::Language::of(file)
        .with_context(|| format!("unsupported language for {}", file.display()))?;

    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read {}", file.display()))?;

    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string();

    if lang == Language::Rust {
        let (l, c) = match (line, col) {
            (Some(l), Some(c)) => (l, c),
            (Some(l), None) => (l, 1),
            _ => {
                let offset = find_loop_offset(&text, lang, symbol, line, col)?;
                crate::signature::line_col_at(&text, offset)
                    .with_context(|| format!("cannot find line/col for loop in {}", file.display()))?
            }
        };
        return loop_to_iterator(remote, root, file, l, c, apply, force).await;
    }
    if lang == Language::Java {
        anyhow::bail!("loop_to_iterator does not support Java yet");
    }

    let offset = find_loop_offset(&text, lang, symbol, line, col)?;

    let polyglot_loop = match lang {
        Language::TypeScript | Language::JavaScript => recognise_ts(&text, offset)?,
        Language::Python => recognise_python(&text, offset)?,
        Language::Swift => recognise_swift(&text, offset)?,
        Language::Cpp | Language::C => recognise_cpp(&text, offset)?,
        Language::Go => recognise_go(&text, offset)?,
        Language::Rust | Language::Java => unreachable!(),
    };

    let mut new_text = text.clone();
    new_text.replace_range(polyglot_loop.start..polyglot_loop.end, &polyglot_loop.replacement);

    let reports = crate::diagnostics::validate_texts(
        remote,
        root,
        &[(file.to_path_buf(), new_text.clone())],
        &[],
    )
    .await?;
    let errors: Vec<&crate::diagnostics::DocDiagnostic> = reports
        .iter()
        .flat_map(|r| r.items.iter())
        .filter(|d| d.severity == "error")
        .collect();
    let diagnostics: Vec<String> = errors
        .iter()
        .map(|d| {
            format!(
                "{}{} ({rel}:{}:{})",
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
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let files: BTreeMap<PathBuf, String> =
            std::iter::once((file.to_path_buf(), new_text.clone())).collect();
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }

    Ok(Rewritten {
        statement: polyglot_loop.statement,
        root: root.to_path_buf(),
        file: rel,
        new_text,
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOPS: &str = "pub fn total(prices: &[u64]) -> u64 {\n    let mut sum = 0;\n    for p in prices {\n        sum += p * 2;\n    }\n    sum\n}\n\npub fn evens(xs: &[i32]) -> usize {\n    let mut n = 0;\n    for x in xs {\n        if x % 2 == 0 {\n            n += 1;\n        }\n    }\n    n\n}\n\npub fn names(users: &[(u32, String)]) -> Vec<String> {\n    let mut out = Vec::new();\n    for (id, name) in users {\n        if *id > 10 {\n            out.push(name.clone());\n        }\n    }\n    out\n}\n";

    fn at(line: &str) -> AccumulatorLoop {
        recognise(LOOPS, LOOPS.find(line).unwrap()).unwrap()
    }

    #[test]
    fn a_sum_a_count_and_a_collect_are_recognised() {
        let sum = at("for p in prices");
        assert_eq!(
            chain(&sum, "u64", Some("&[u64]"), false),
            "    let sum: u64 = prices.iter().map(|p| p * 2).sum();"
        );
        assert_eq!(
            &LOOPS[sum.start..sum.end],
            "    let mut sum = 0;\n    for p in prices {\n        sum += p * 2;\n    }"
        );
        let count = at("for x in xs");
        assert_eq!(
            chain(&count, "usize", None, false),
            "    let n: usize = xs.into_iter().filter(|&x| x % 2 == 0).count();"
        );
        let names = at("for (id, name)");
        assert_eq!(
            chain(&names, "Vec<_>", None, true),
            "    let mut out: Vec<_> = users.into_iter().filter_map(|(id, name)| if *id > 10 { Some(name.clone()) } else { None }).collect();"
        );
    }

    #[test]
    fn rust_general_loop_conversion_find_any_all() {
        let find_src = "pub fn find_user(users: &[u32]) -> Option<&u32> {\n    let mut found = None;\n    for u in users {\n        if *u > 10 {\n            found = Some(u);\n            break;\n        }\n    }\n    found\n}\n";
        let l = recognise(find_src, find_src.find("for u in").unwrap()).unwrap();
        assert_eq!(
            chain(&l, "Option<_>", None, false),
            "    let found: Option<_> = users.into_iter().find(|&u| *u > 10);"
        );

        let find_map_src = "pub fn find_user(users: &[u32]) -> Option<u32> {\n    let mut found = None;\n    for u in users {\n        if *u > 10 {\n            found = Some(*u);\n            break;\n        }\n    }\n    found\n}\n";
        let l2 = recognise(find_map_src, find_map_src.find("for u in").unwrap()).unwrap();
        assert_eq!(
            chain(&l2, "Option<_>", None, false),
            "    let found: Option<_> = users.into_iter().find_map(|u| if *u > 10 { Some(*u) } else { None });"
        );

        let any_src = "pub fn has_admin(users: &[bool]) -> bool {\n    let mut has_any = false;\n    for u in users {\n        if *u {\n            has_any = true;\n            break;\n        }\n    }\n    has_any\n}\n";
        let l = recognise(any_src, any_src.find("for u in").unwrap()).unwrap();
        assert_eq!(
            chain(&l, "bool", None, false),
            "    let has_any: bool = users.into_iter().any(|&u| *u);"
        );

        let all_src = "pub fn all_active(users: &[bool]) -> bool {\n    let mut all_match = true;\n    for u in users {\n        if !*u {\n            all_match = false;\n            break;\n        }\n    }\n    all_match\n}\n";
        let l = recognise(all_src, all_src.find("for u in").unwrap()).unwrap();
        assert_eq!(
            chain(&l, "bool", None, false),
            "    let all_match: bool = users.into_iter().all(|&u| !(!*u));"
        );
    }

    #[test]
    fn a_loop_that_does_more_than_accumulate_is_refused() {
        for (from, to, anchor, why) in [
            (
                "        sum += p * 2;\n",
                "        sum += p * 2;\n        if sum > 9 {\n            break;\n        }\n",
                "for p in",
                "`break`",
            ),
            (
                "        sum += p * 2;\n",
                "        sum += p * sum;\n",
                "for p in",
                "more than the one",
            ),
            (
                "    let mut sum = 0;",
                "    let mut sum = 5;",
                "for p in",
                "not zero",
            ),
            (
                "    let mut out = Vec::new();",
                "    let mut out = vec![1];",
                "for (id",
                "does not start empty",
            ),
            (
                "        sum += p * 2;\n",
                "        sum -= p;\n",
                "for p in",
                "is not `sum += …;`",
            ),
        ] {
            let text = LOOPS.replace(from, to);
            let err = recognise(&text, text.find(anchor).unwrap()).unwrap_err();
            assert!(format!("{err:#}").contains(why), "{why}: {err:#}");
        }
    }

    #[test]
    fn the_source_is_iterated_as_the_loop_did() {
        assert_eq!(iterator_of("prices", None), "prices.into_iter()");
        assert_eq!(
            iterator_of("prices", Some("Vec<u64>")),
            "prices.into_iter()"
        );
        assert_eq!(iterator_of("prices", Some("&[u64]")), "prices.iter()");
        assert_eq!(iterator_of("xs", Some("&mut Vec<u8>")), "xs.iter_mut()");
        assert_eq!(iterator_of("&v", None), "v.iter()");
        assert_eq!(
            iterator_of("&mut self.items", None),
            "self.items.iter_mut()"
        );
        assert_eq!(iterator_of("0..n", None), "(0..n)");
        assert_eq!(iterator_of("m.values()", None), "m.values().into_iter()");
        assert_eq!(
            binding_type("```rust\nprices: &[u64]\n```"),
            Some("&[u64]".into())
        );
        assert_eq!(
            binding_type("```rust\nlet mut sum: u64\n```\n---\nno Drop"),
            Some("u64".into())
        );
        assert_eq!(binding_type("```rust\nfn f()\n```"), None);
        assert!(is_zero("0") && is_zero("0.0") && is_zero("0u64") && is_zero("0_i32"));
        assert!(!is_zero("1") && !is_zero("x") && !is_zero("0x10"));
    }

    #[test]
    fn polyglot_ts_recognised() {
        let src = "function total(prices: number[]): number {\n    let sum = 0;\n    for (const p of prices) {\n        sum += p * 2;\n    }\n    return sum;\n}";
        let poly = recognise_ts(src, src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    const sum = prices.reduce((acc, p) => acc + (p * 2), 0);");

        let cnt_src = "function evens(xs: number[]): number {\n    let count = 0;\n    for (const x of xs) {\n        if (x % 2 === 0) {\n            count += 1;\n        }\n    }\n    return count;\n}";
        let poly = recognise_ts(cnt_src, cnt_src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    const count = xs.filter(x => x % 2 === 0).length;");

        let find_src = "function findItem(items: string[]): string | null {\n    let found = null;\n    for (const item of items) {\n        if (item.length > 3) {\n            found = item;\n            break;\n        }\n    }\n    return found;\n}";
        let poly = recognise_ts(find_src, find_src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    const found = items.find(item => item.length > 3) ?? null;");

        let projected_find_src = "function findPrice(prices: number[]): number | null {\n    let found = null;\n    for (const p of prices) {\n        if (p > 0) {\n            found = p * 2;\n            break;\n        }\n    }\n    return found;\n}";
        let poly = recognise_ts(projected_find_src, projected_find_src.find("for ").unwrap()).unwrap();
        assert!(poly.replacement.contains("prices.find(p =>"));
        assert!(!poly.replacement.contains(".filter("));
    }

    #[test]
    fn polyglot_python_recognised() {
        let src = "def total(prices):\n    total = 0\n    for p in prices:\n        total += p * 2\n    return total";
        let poly = recognise_python(src, src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    total = sum(p * 2 for p in prices)");

        let cnt_src = "def evens(xs):\n    count = 0\n    for x in xs:\n        if x % 2 == 0:\n            count += 1\n    return count";
        let poly = recognise_python(cnt_src, cnt_src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    count = sum(1 for x in xs if x % 2 == 0)");

        let collect_src = "def names(users):\n    out = []\n    for u in users:\n        if u.age > 10:\n            out.append(u.name)\n    return out";
        let poly = recognise_python(collect_src, collect_src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    out = [u.name for u in users if u.age > 10]");
    }

    #[test]
    fn polyglot_swift_recognised() {
        let src = "func total(prices: [Int]) -> Int {\n    var sum = 0\n    for p in prices {\n        sum += p * 2\n    }\n    return sum\n}";
        let poly = recognise_swift(src, src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    let sum = prices.reduce(0) { _acc, p in _acc + (p * 2) }");

        let cnt_src = "func evens(xs: [Int]) -> Int {\n    var count = 0\n    for x in xs {\n        if x % 2 == 0 {\n            count += 1\n        }\n    }\n    return count\n}";
        let poly = recognise_swift(cnt_src, cnt_src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    let count = xs.filter { x in x % 2 == 0 }.count");
    }

    #[test]
    fn swift_conversions_bind_the_loop_element_in_each_closure() {
        let collect = "func doubled(prices: [Int]) -> [Int] {\n    var result: [Int] = []\n    for p in prices {\n        result.append(p * 2)\n    }\n    return result\n}";
        let poly = recognise_swift(collect, collect.find("for ").unwrap()).unwrap();
        assert!(poly.replacement.contains("map { p in p * 2 }"));

        let find = "func findPrice(prices: [Int]) -> Int? {\n    var found: Int? = nil\n    for p in prices {\n        if p > 0 {\n            found = p * 2\n            break\n        }\n    }\n    return found\n}";
        let poly = recognise_swift(find, find.find("for ").unwrap()).unwrap();
        assert!(poly.replacement.contains("first(where: { p in p > 0 }).map { p in p * 2 }"));

        let any = "func hasPrice(prices: [Int]) -> Bool {\n    var found = false\n    for p in prices {\n        if p > 0 {\n            found = true\n            break\n        }\n    }\n    return found\n}";
        let poly = recognise_swift(any, any.find("for ").unwrap()).unwrap();
        assert!(poly.replacement.contains("contains(where: { p in p > 0 })"));

        let all = "func allPrices(prices: [Int]) -> Bool {\n    var valid = true\n    for p in prices {\n        if p <= 0 {\n            valid = false\n            break\n        }\n    }\n    return valid\n}";
        let poly = recognise_swift(all, all.find("for ").unwrap()).unwrap();
        assert!(poly.replacement.contains("allSatisfy { p in !(p <= 0) }"));
    }

    #[test]
    fn polyglot_cpp_recognised() {
        let src = "int total(const std::vector<int>& prices) {\n    int sum = 0;\n    for (const auto& p : prices) {\n        sum += p * 2;\n    }\n    return sum;\n}";
        let poly = recognise_cpp(src, src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    auto&& __prod_code_range = (prices);\n    int sum = std::accumulate(__prod_code_range.begin(), __prod_code_range.end(), static_cast<int>(0), [](auto _acc, const auto& p) { return _acc + (p * 2); });");

        let any_src = "bool has_even(const std::vector<int>& xs) {\n    bool has_any = false;\n    for (const auto& x : xs) {\n        if (x % 2 == 0) {\n            has_any = true;\n            break;\n        }\n    }\n    return has_any;\n}";
        let poly = recognise_cpp(any_src, any_src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    auto&& __prod_code_range = (xs);\n    const bool has_any = std::any_of(__prod_code_range.begin(), __prod_code_range.end(), [](const auto& x) { return x % 2 == 0; });");
    }

    #[test]
    fn polyglot_go_recognised() {
        let src = "func total(prices []int) int {\n    sum := 0\n    for _, p := range prices {\n        sum += p * 2\n    }\n    return sum\n}";
        let poly = recognise_go(src, src.find("for ").unwrap()).unwrap();
        assert_eq!(poly.replacement, "    sum := func() int { s := 0; for _, p := range prices { s += p * 2 }; return s }()");
    }

    #[test]
    fn cpp_conversion_evaluates_range_once_and_preserves_accumulator_type() {
        let src = "long long total = 0;\nfor (const auto& value : make_values()) {\n    total += value;\n}";
        let poly = recognise_cpp(src, src.find("for ").unwrap()).unwrap();

        assert_eq!(poly.replacement.matches("make_values()").count(), 1);
        assert!(poly.replacement.contains("static_cast<long long>(0)"));
    }

    #[test]
    fn go_single_range_variable_remains_the_index() {
        let src = "func total(values []int) int {\n    sum := 0\n    for i := range values {\n        sum += i\n    }\n    return sum\n}";
        let poly = recognise_go(src, src.find("for ").unwrap()).unwrap();

        assert!(poly.replacement.contains("for i := range values"));
        assert!(!poly.replacement.contains("for _, i := range values"));
    }
}
