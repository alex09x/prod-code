//! Introducing a variable for every occurrence of an expression: `(w + 1)` three times in a
//! function becomes `let w1 = w + 1;` once, and `w1` three times.
//!
//! rust-analyzer's `extract_variable` replaces the one selection. Replacing all of them is only
//! the same program when evaluating the expression once is the same as evaluating it at each
//! place, so the expression may not call anything (a call can do something, or return something
//! different each time), and no name it reads may be assigned, rebound or have a method called on
//! it between the first occurrence and the last. An expression that can panic is bound earlier
//! only when one occurrence runs every time the binding does, so a division is never moved out
//! of the `if` that guarded it. Anything else is refused, and the result is type-checked.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Introduced {
    pub name: String,
    pub expression: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub occurrences: usize,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Introduced {
    pub fn render(&self) -> String {
        let old_text = std::fs::read_to_string(self.root.join(&self.file)).unwrap_or_default();
        let old_text = if self.applied {
            crate::refactor::text_before_apply(&self.root.join(&self.file))
        } else {
            old_text
        };
        let new_text = self
            .rewritten
            .first()
            .map(|(_, t)| t.as_str())
            .unwrap_or("");
        let diff = similar::TextDiff::from_lines(old_text.as_str(), new_text)
            .unified_diff()
            .context_radius(1)
            .header(&format!("a/{}", self.file), &format!("b/{}", self.file))
            .to_string();
        let mut out = format!(
            "`let {} = {};` ({})\n\n- {} occurrence(s) now read `{}`\n\n{diff}",
            self.name, self.expression, self.file, self.occurrences, self.name
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

/// The identifiers an expression reads, and why it may not be evaluated once for every place,
/// when it may not: a call (`f(x)`, `x.len()`), a macro, `?` or `.await`.
pub fn reads_and_effects(expr: &str) -> (Vec<String>, Option<String>) {
    let mut names = Vec::new();
    let mut effect = None;
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    // The word after `as` is a type.
    let mut after_as = false;
    while i < chars.len() {
        let c = chars[i];
        if is_ident(c) && (i == 0 || !is_ident(chars[i - 1])) {
            let start = i;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let next = chars[i..].iter().find(|c| !c.is_whitespace()).copied();
            let after_dot = start > 0 && chars[start - 1] == '.';
            let is_type = std::mem::replace(&mut after_as, word == "as");
            if next == Some('(') {
                effect.get_or_insert(format!("it calls `{word}`"));
            } else if next == Some('!') && chars.get(i + 1) != Some(&'=') {
                effect.get_or_insert(format!("it expands the macro `{word}!`"));
            } else if !after_dot
                && !is_type
                && !word.chars().next().is_some_and(|c| c.is_ascii_digit())
                && !matches!(word.as_str(), "as" | "true" | "false")
                && !word.chars().next().is_some_and(char::is_uppercase)
            {
                names.push(word);
            }
            continue;
        }
        if c == '?' {
            effect.get_or_insert("it propagates an error with `?`".to_string());
        }
        i += 1;
    }
    if expr.contains(".await") {
        effect.get_or_insert("it awaits".to_string());
    }
    names.sort();
    names.dedup();
    (names, effect)
}

/// Every place `expr` occurs in `text[from..to]` as whole tokens, as byte offsets.
pub fn occurrences(text: &str, from: usize, to: usize, expr: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let region = &text[from..to];
    let mut at = 0;
    while let Some(i) = region[at..].find(expr) {
        let start = from + at + i;
        let end = start + expr.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let first = expr.chars().next().unwrap_or(' ');
        let last = expr.chars().next_back().unwrap_or(' ');
        // `x(w + 1)` is a call's argument list, not `(w + 1)`: a name there would glue to it.
        let clean_start = !((is_ident(first) || first == '(') && before.is_some_and(is_ident));
        let clean_end = !((is_ident(last) || last == ')') && after.is_some_and(is_ident));
        if clean_start && clean_end {
            out.push(start);
        }
        at += i + expr.len().max(1);
    }
    out
}

/// Whether `name` is assigned (`name =`, `name +=`), mutably borrowed (`&mut name`) or bound
/// again (`let name`) in `text`.
pub fn changes(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(i) = text[at..].find(name) {
        let start = at + i;
        let end = start + name.len();
        at = end;
        let whole = (start == 0 || !is_ident(bytes[start - 1] as char))
            && (end >= bytes.len() || !is_ident(bytes[end] as char));
        if !whole {
            continue;
        }
        let before = text[..start].trim_end();
        let mut after = text[end..].trim_start();
        // `name.field = ...` changes `name` too, and `name.method()` may: it can take `&mut self`.
        let mut segments = 0;
        while let Some(rest) = after.strip_prefix('.') {
            let field = rest.len() - rest.trim_start_matches(is_ident).len();
            if field == 0 {
                break;
            }
            segments += 1;
            after = rest[field..].trim_start();
        }
        let called = segments > 0 && after.starts_with('(');
        let assigned = (after.starts_with('=') && !after.starts_with("=="))
            || ["+=", "-=", "*=", "/=", "%=", "|=", "&=", "^=", "<<=", ">>="]
                .iter()
                .any(|op| after.starts_with(op));
        let borrowed = before.ends_with("&mut");
        let bound = before.ends_with("let") || before.ends_with("let mut");
        if assigned || borrowed || bound || called {
            return true;
        }
    }
    false
}

/// Whether evaluating `expr` can panic: a division or remainder, an index, or arithmetic that
/// overflows in a debug build.
pub fn can_panic(expr: &str) -> bool {
    expr.contains(['/', '%', '[', '+', '-', '*']) || expr.contains("<<")
}

/// Whether the occurrence at `at` is evaluated whenever the statement at `anchor` runs: it sits
/// at the anchor's own block level, nothing between them can leave early (`return`, `break`,
/// `continue`, `?`, a panicking macro), and nothing earlier in its own statement short-circuits
/// or defers it (`&&`, `||`, a closure).
pub fn surely_evaluated(text: &str, anchor: usize, statement: usize, at: usize) -> bool {
    let between = &text[anchor..at];
    let mut depth = 0i32;
    for c in between.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    let leaves = ["return", "break", "continue"].iter().any(|w| {
        between.match_indices(w).any(|(i, _)| {
            !between[..i].chars().next_back().is_some_and(is_ident)
                && !between[i + w.len()..].chars().next().is_some_and(is_ident)
        })
    }) || between.contains('?')
        || [
            "panic!",
            "unreachable!",
            "todo!",
            "unimplemented!",
            "assert",
        ]
        .iter()
        .any(|m| between.contains(m));
    let own = &text[statement.min(at)..at];
    depth == 0 && !leaves && !own.contains('|') && !own.contains("&&")
}

/// The span to replace for the occurrence `start..end`: with the parentheses around it when it
/// is written `(expr)` on its own, so `a + (s.x + 1)` becomes `a + x1`. The parentheses of a
/// call, an index or a method stay.
pub fn parenthesised(text: &str, start: usize, end: usize) -> std::ops::Range<usize> {
    let open = text[..start].trim_end();
    let close = text[end..].trim_start();
    if open.ends_with('(') && close.starts_with(')') {
        let before = open[..open.len() - 1].trim_end().chars().next_back();
        if !before.is_some_and(|c| is_ident(c) || matches!(c, ')' | ']' | '>' | '!')) {
            return open.len() - 1..text.len() - close.len() + 1;
        }
    }
    start..end
}

/// The bodies, as brace offsets, of the loops (`loop`, `while`, `for`) whose keyword lies in
/// `text[from..to]`.
pub fn loops_after(text: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for keyword in ["loop", "while", "for"] {
        let mut at = from;
        while let Some(i) = text[at..to].find(keyword) {
            let start = at + i;
            let end = start + keyword.len();
            at = end;
            let whole = !text[..start].chars().next_back().is_some_and(is_ident)
                && !text[end..].chars().next().is_some_and(is_ident);
            if !whole {
                continue;
            }
            if let Some(open) = text[end..].find('{').map(|i| end + i)
                && let Some(close) = crate::parameter_object::matching_bracket(text, open)
            {
                out.push((open, close));
            }
        }
    }
    out
}

/// The opening brace of the innermost block, from the function body `body_open` inward, that
/// holds both `first` and `last`.
pub fn innermost_block(text: &str, body_open: usize, first: usize, last: usize) -> usize {
    text[body_open..first]
        .match_indices('{')
        .map(|(i, _)| body_open + i)
        .rev()
        .find(|open| {
            crate::parameter_object::matching_bracket(text, *open).is_some_and(|close| close > last)
        })
        .unwrap_or(body_open)
}

/// Where the statement of the block `block_open` that holds `at` begins: after the last `;`, or
/// the last `}` that closes a statement (not one followed by `else`), at the block's own depth.
pub fn statement_start(text: &str, block_open: usize, at: usize) -> usize {
    let mut depth = 0i32;
    let mut start = block_open + 1;
    for (i, c) in text[block_open + 1..at].char_indices() {
        let i = block_open + 1 + i;
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' => depth -= 1,
            '}' => {
                depth -= 1;
                if depth == 0 && !text[i + 1..].trim_start().starts_with("else") {
                    start = i + 1;
                }
            }
            ';' if depth == 0 => start = i + 1,
            _ => {}
        }
    }
    start + (text[start..].len() - text[start..].trim_start().len())
}

/// The braces of the innermost function body that holds `at`: the nearest `fn ` before it whose
/// body, the first `{` after its parameter list, closes after it.
pub fn enclosing_body(text: &str, at: usize) -> Option<(usize, usize)> {
    let mut search = at;
    while let Some(fn_at) = text[..search].rfind("fn ") {
        search = fn_at;
        if fn_at > 0 && text[..fn_at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let name_at = fn_at + 3;
        let Some((_, _, close)) = crate::signature::param_span(text, name_at) else {
            continue;
        };
        let Some(open) = text[close..].find(['{', ';']).map(|i| close + i) else {
            continue;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        if let Some(end) = crate::parameter_object::matching_bracket(text, open)
            && open < at
            && at < end
        {
            return Some((open, end));
        }
    }
    None
}

/// Introduces `name` for the expression selected at `line`:`col` .. `end_line`:`end_col` of
/// `file`, replacing every occurrence of it in the enclosing function.
#[allow(clippy::too_many_arguments)]
pub async fn introduce_variable(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    (line, col): (u32, u32),
    (end_line, end_col): (u32, u32),
    name: &str,
    apply: bool,
    force: bool,
) -> Result<Introduced> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(is_ident),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let start =
        crate::signature::offset_of(&text, line, col).context("the start is not in the file")?;
    let end = crate::signature::offset_of(&text, end_line, end_col)
        .context("the end is not in the file")?;
    anyhow::ensure!(start < end, "the selection is empty");
    let expr = text[start..end].trim().to_string();
    anyhow::ensure!(!expr.contains(';'), "the selection is not one expression");
    let (names, effect) = reads_and_effects(&expr);
    if let Some(effect) = effect {
        anyhow::bail!(
            "`{expr}` cannot be evaluated once for every place it occurs: {effect}. Use \
             `code_assist` with `extract_variable` for this one occurrence"
        );
    }

    // The innermost function whose body holds the selection.
    let (body_open, body_close) =
        enclosing_body(&text, start).context("the selection is not inside a function")?;
    let found = occurrences(&text, body_open, body_close, &expr);
    let first = *found
        .first()
        .context("the selection is not in the function")?;
    let last = *found.last().unwrap_or(&first);
    let anchor = statement_start(&text, innermost_block(&text, body_open, first, last), first);
    // A loop that starts after the binding and holds a later occurrence runs that occurrence
    // again, after whatever the rest of its body changed.
    let mut watched = vec![(anchor, last + expr.len())];
    watched.extend(
        loops_after(&text, anchor, last)
            .into_iter()
            .filter(|(open, close)| found.iter().any(|at| open < at && at < close)),
    );
    for n in &names {
        anyhow::ensure!(
            !watched
                .iter()
                .any(|(from, to)| changes(&text[*from..*to], n)),
            "`{n}` changes between the first occurrence of `{expr}` and the last, so one value \
             cannot stand for all of them"
        );
    }
    // Binding it earlier evaluates it where the code may not have: a division under
    // `if b != 0` would divide by zero. That is the same program only if one occurrence
    // runs whenever the binding does.
    if can_panic(&expr) {
        let anywhere = found.iter().any(|at| {
            let statement =
                statement_start(&text, innermost_block(&text, body_open, *at, *at), *at);
            surely_evaluated(&text, anchor, statement, *at)
        });
        anyhow::ensure!(
            anywhere,
            "`{expr}` can panic, and no occurrence of it runs every time the binding would \
             (each is under a condition, or after a way out). Use `code_assist` with \
             `extract_variable` for one occurrence"
        );
    }

    // `let name = expr;` above the statement that holds the first occurrence, in the innermost
    // block that holds them all, at its indentation.
    let stmt_line_start = text[..anchor].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[stmt_line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let bare = expr
        .strip_prefix('(')
        .and_then(|e| e.strip_suffix(')'))
        .filter(|inner| {
            crate::parameter_object::matching_bracket(&expr, 0) == Some(expr.len() - 1)
                && !inner.is_empty()
        })
        .unwrap_or(&expr);
    let mut new_text = text.clone();
    for at in found.iter().rev() {
        new_text.replace_range(parenthesised(&text, *at, *at + expr.len()), name);
    }
    new_text.insert_str(stmt_line_start, &format!("{indent}let {name} = {bare};\n"));

    let rewritten: BTreeMap<PathBuf, String> =
        std::iter::once((file.to_path_buf(), new_text.clone())).collect();
    let reports = crate::diagnostics::validate_texts(
        remote,
        root,
        &[(file.to_path_buf(), new_text.clone())],
        &[],
    )
    .await?;
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
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }
    Ok(Introduced {
        name: name.to_string(),
        expression: bare.to_string(),
        root: root.to_path_buf(),
        file: file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned(),
        occurrences: found.len(),
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
    fn an_expression_that_calls_or_awaits_is_not_one_value() {
        assert_eq!(reads_and_effects("(w + 1)"), (vec!["w".to_string()], None));
        assert_eq!(
            reads_and_effects("a.b * LIMIT as u64").0,
            vec!["a".to_string()]
        );
        for with_effect in ["f(x)", "v.len()", "format!(\"x\")", "r?", "fut.await"] {
            assert!(reads_and_effects(with_effect).1.is_some(), "{with_effect}");
        }
    }

    #[test]
    fn occurrences_are_whole_and_a_changed_name_is_seen() {
        let t = "let a = (w + 1) * 2; let b = (w + 1) + 3; let c = x(w + 1);";
        assert_eq!(occurrences(t, 0, t.len(), "(w + 1)").len(), 2);
        let f = "fn outer(w: u32) -> u32 {\n    fn inner() {}\n    w + 1\n}\n";
        let (open, close) = enclosing_body(f, f.find("w + 1").unwrap()).unwrap();
        assert_eq!((&f[open..open + 1], &f[close..close + 1]), ("{", "}"));
        assert!(f[open..close].contains("fn inner"));
        assert!(enclosing_body(f, 3).is_none());
        assert_eq!(occurrences("aw + 1 + w + 1", 0, 14, "w + 1").len(), 1);
        assert!(changes("w += 1;", "w"));
        assert!(changes("let w = 3;", "w"));
        assert!(changes("f(&mut w);", "w"));
        assert!(changes("w.inner.len = 2;", "w"));
        assert!(!changes("let n = w.len == 2;", "w"));
        assert!(!changes("let x = w == 1;", "w"));
        assert!(!changes("let ww = 1; www = 2;", "w"));
    }

    #[test]
    fn the_binding_goes_before_the_statement_in_the_block_that_holds_them_all() {
        let f = "fn f(w: u32, h: u32) -> u32 {\n    let x = 1;\n    let a = if h > 2 {\n        (w + 1) * h\n    } else {\n        0\n    };\n    a + (w + 1)\n}\n";
        let found = occurrences(f, 0, f.len(), "(w + 1)");
        let body = f.find('{').unwrap();
        assert_eq!(innermost_block(f, body, found[0], found[1]), body);
        let at = statement_start(f, body, found[0]);
        assert!(f[at..].starts_with("let a = if"), "{}", &f[at..]);
        let g = "fn g() { if c { let y = 2; foo(w + 1, w + 1) } }";
        let found = occurrences(g, 0, g.len(), "w + 1");
        let inner = innermost_block(g, g.find('{').unwrap(), found[0], found[1]);
        assert_eq!(&g[inner - 2..inner], "c ");
        assert!(g[statement_start(g, inner, found[0])..].starts_with("foo("));
        // The `}` before `else` does not end the statement.
        let e = "fn e() { let a = if c { 1 } else { w + 1 }; a + (w + 1) }";
        let at = statement_start(e, e.find('{').unwrap(), e.find("w + 1").unwrap());
        assert!(e[at..].starts_with("let a"), "{}", &e[at..]);
    }

    #[test]
    fn a_method_call_on_a_name_may_change_it() {
        assert!(changes("s.bump();", "s"));
        assert!(changes("s.inner.push(1);", "s"));
        assert!(!changes("let y = s.x;", "s"));
    }

    #[test]
    fn a_guarded_division_is_not_surely_evaluated() {
        assert!(can_panic("a / b") && can_panic("v[i]") && can_panic("w + 1"));
        assert!(!can_panic("s.x") && !can_panic("flag"));
        let r = "fn r() {\n    let mut r = 0;\n    if b != 0 {\n        r += a / b;\n    }\n    r + a / b\n}";
        let found = occurrences(r, 0, r.len(), "a / b");
        let body = r.find('{').unwrap();
        let anchor = statement_start(r, body, found[0]);
        assert!(r[anchor..].starts_with("if b != 0"));
        // Inside the `if`: not every time. After it, at the block's level: every time.
        let own = |at: usize| statement_start(r, innermost_block(r, body, at, at), at);
        assert!(!surely_evaluated(r, anchor, own(found[0]), found[0]));
        assert!(surely_evaluated(r, anchor, own(found[1]), found[1]));
        // A way out before it, or a short circuit in its own statement, and it is not.
        let early = "fn e() {\n    if c { return 0; }\n    a / b\n}";
        let at = early.find("a / b").unwrap();
        assert!(!surely_evaluated(early, early.find("if").unwrap(), at, at));
        let short = "fn s() {\n    let z = b != 0 && a / b > 1;\n}";
        let at = short.find("a / b").unwrap();
        let st = short.find("let z").unwrap();
        assert!(!surely_evaluated(short, st, st, at));
    }

    #[test]
    fn an_occurrence_in_parentheses_of_its_own_loses_them() {
        let t = "a + (s.x + 1) + f(s.x + 1) + v[(s.x + 1)]";
        let spans: Vec<_> = occurrences(t, 0, t.len(), "s.x + 1")
            .into_iter()
            .map(|at| &t[parenthesised(t, at, at + 7)])
            .collect();
        assert_eq!(spans, ["(s.x + 1)", "s.x + 1", "(s.x + 1)"]);
    }

    #[test]
    fn a_loop_after_the_first_occurrence_is_watched_whole() {
        let t = "let a = w + 1; while go { f(w + 1); w += 1; } for_each(x); loops;";
        let found = occurrences(t, 0, t.len(), "w + 1");
        let loops = loops_after(t, found[0], found[1]);
        assert_eq!(loops.len(), 1);
        let (open, close) = loops[0];
        assert!(t[open..=close].contains("w += 1"));
        assert!(changes(&t[open..close], "w"));
        assert!(!changes(&t[found[0]..found[1] + 5], "w"));
    }
}
