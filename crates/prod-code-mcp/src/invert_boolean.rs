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
                 keeps its old meaning under its new name, so each needs a look):\n",
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
        if c == b'"' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            i = j + 1;
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
                || line.contains(" fn ");
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
        i += 1;
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
            b';' | b'}' => return i,
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
    let (nl, nc) = crate::signature::line_col_at(&text, start);
    for (path, l, c) in crate::signature::references(remote, root, file, nl, nc)
        .await
        .unwrap_or_default()
    {
        let body = texts
            .entry(path.clone())
            .or_insert_with(|| std::fs::read_to_string(&path).unwrap_or_default())
            .clone();
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
