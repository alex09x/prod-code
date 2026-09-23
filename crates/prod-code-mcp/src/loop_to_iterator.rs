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

/// What the loop builds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Shape {
    /// `acc += X`, maybe under `if C`.
    Sum { cond: Option<String>, value: String },
    /// `if C { acc += 1 }` into a `usize`.
    Count { cond: String },
    /// `acc.push(X)`, maybe under `if C`.
    Collect { cond: Option<String>, value: String },
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
    let number: String = init
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '_')
        .collect();
    let suffix = &init[number.len()..];
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

    for word in ["break", "continue", "return"] {
        anyhow::ensure!(
            whole_word_count(body, word) == 0,
            "the loop's body has `{word}`, which an iterator chain cannot express"
        );
    }
    anyhow::ensure!(
        !body.contains('?') && !body.contains(".await"),
        "the loop's body can leave early (`?`) or await"
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
    let shape = if pushes {
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
    let (line, col) = crate::signature::line_col_at(text, at);
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let hover = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line - 1, "character": col - 1 },
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
}
