//! Inlining a parameter: when every call passes the same constant for it, the value moves into
//! the body as a `let`, and the parameter leaves the declaration and every call.
//!
//! `fn clamp(x: u32, max: u32)` called as `clamp(v, LIMIT)` everywhere becomes `fn clamp(x: u32)`
//! with `let max: u32 = LIMIT;` at the top of its body, and the calls become `clamp(v)`. The
//! value must mean the same thing in the body as at the call: a literal, a constant, a path. A
//! lowercase name may be a local of the caller and is refused, and so is a set of calls that do
//! not agree on the value.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the inlining did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InlinedParameter {
    pub function: String,
    pub parameter: String,
    /// The value every call passed, now bound at the top of the body.
    pub value: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub rewritten_calls: usize,
    /// References that are not a call with this parameter's argument: the function used as a
    /// value, or a call this could not read. They block the write.
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl InlinedParameter {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` of `{}` ({})\n\n- every call passes `{}`: it is bound at the top of the body\n- \
             {} call(s) lose the argument\n\n",
            self.parameter, self.function, self.file, self.value, self.rewritten_calls
        );
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
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot a call with this argument ({}); nothing is written while any remains:\n",
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

/// Whether an argument means the same in the callee's body as at the call: a literal, a
/// constant (`ALL_CAPS`), a type or unit value (`Mode`, `None`), or a path of those
/// (`Mode::Fast`, `crate::limits::MAX`). A lowercase name may be a local of the caller; a call or
/// an expression may depend on one.
pub fn is_caller_independent(arg: &str) -> bool {
    let a = arg.trim();
    let literal = a.strip_prefix('-').unwrap_or(a);
    if literal.starts_with(|c: char| c.is_ascii_digit())
        && literal.chars().all(|c| is_ident(c) || c == '.')
    {
        return true;
    }
    if a == "true" || a == "false" {
        return true;
    }
    if (a.starts_with('"') || a.starts_with("b\"") || a.starts_with('\'')) && !a.contains('{') {
        return true;
    }
    let segments: Vec<&str> = a.split("::").collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || !s.chars().all(is_ident))
    {
        return false;
    }
    let last = segments.last().copied().unwrap_or("");
    let first_char_upper = last.chars().next().is_some_and(char::is_uppercase);
    let all_caps = last
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    first_char_upper || all_caps
}

/// Inlines the parameter at `line`:`col` of `file`.
#[allow(clippy::too_many_arguments)]
pub async fn inline_parameter(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    apply: bool,
    force: bool,
) -> Result<InlinedParameter> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let (fn_at, param, _) = crate::signature::parameter_at(&text, line, col)
        .context("the position is not on a parameter of a function declaration")?;
    let (function, open, close) =
        crate::signature::param_span(&text, fn_at).context("the function has no parameter list")?;
    let (receiver, declared) = crate::signature::parse_declared(&text[open..close]);
    let index = declared
        .iter()
        .position(|d| d.name == param)
        .context("the parameter is not in the list")?;
    let raw = declared[index].raw.clone();
    anyhow::ensure!(
        raw.contains(':'),
        "`{param}` has no type written; a `let` for it needs one"
    );
    let body_open = text[close..]
        .find('{')
        .map(|i| close + i)
        .with_context(|| format!("`{function}` has no body"))?;
    let body_close = crate::parameter_object::matching_bracket(&text, body_open)
        .context("the function's body does not close")?;

    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.clone());
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut values: Vec<(String, String)> = Vec::new();
    let (fl, fc) = crate::signature::line_col_at(&text, fn_at);
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    for (path, l, c) in crate::signature::references(remote, root, file, fl, fc)
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
        if !body[at..].starts_with(function.as_str())
            || body[at + function.len()..].starts_with(is_ident)
        {
            unmatched.push(format!(
                "{site} (the analyzer places `{function}` here, but the file says otherwise)"
            ));
            continue;
        }
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        if same_file && body_open < at && at < body_close {
            unmatched.push(format!(
                "{site} (a call inside `{function}` itself passes its own `{param}`)"
            ));
            continue;
        }
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + function.len())
        else {
            unmatched.push(format!(
                "{site} (the function used as a value: it would change type)"
            ));
            continue;
        };
        let args = crate::parameter_object::split_args(&body[args_start..args_end]);
        let method_syntax = body[..at].trim_end().ends_with('.');
        let arg_index = if receiver.is_some() && !method_syntax {
            index + 1
        } else {
            index
        };
        let Some(arg) = args.get(arg_index) else {
            unmatched.push(format!("{site} (the call has no argument for `{param}`)"));
            continue;
        };
        values.push((site, arg.trim().to_string()));
        let remaining: Vec<&str> = args
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != arg_index)
            .map(|(_, a)| a.trim())
            .collect();
        edits.entry(path.clone()).or_default().push((
            args_start,
            args_end - args_start,
            remaining.join(", "),
        ));
    }

    let first = values.first().map(|(_, v)| v.clone()).with_context(|| {
        format!("no call passes a value for `{param}`, so there is none to inline")
    })?;
    let differing: Vec<String> = values
        .iter()
        .filter(|(_, v)| *v != first)
        .map(|(site, v)| format!("{site} passes `{v}`"))
        .collect();
    anyhow::ensure!(
        differing.is_empty(),
        "the calls do not agree on `{param}`: {} of {} pass `{first}`, and\n  {}",
        values.len() - differing.len(),
        values.len(),
        differing.join("\n  ")
    );
    anyhow::ensure!(
        is_caller_independent(&first),
        "every call passes `{first}` for `{param}`, but it may name something of the caller's \
         (a local, or an expression over one); only a literal, a constant or a path is inlined"
    );

    // The declaration: without the parameter, and the value bound at the top of the body.
    let mut kept: Vec<String> = receiver.into_iter().collect();
    kept.extend(
        declared
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, d)| d.raw.clone()),
    );
    let first_line_indent = text[body_open + 1..]
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(4);
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((open, close - open, kept.join(", ")));
    own.push((
        body_open + 1,
        0,
        format!("\n{}let {raw} = {first};", " ".repeat(first_line_indent)),
    ));

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }
    let rewritten_calls = values.len();

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
            unmatched.is_empty() || force,
            "{} reference(s) to `{function}` are not a call passing `{param}`; nothing was \
             written:\n  {}",
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

    Ok(InlinedParameter {
        function,
        parameter: param,
        value: first,
        root: root.to_path_buf(),
        file: display(root, file),
        rewritten_calls,
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
    fn only_a_value_that_means_the_same_in_the_body_is_inlined() {
        for ok in [
            "10",
            "-3",
            "2.5",
            "1_000u64",
            "true",
            "\"x\"",
            "b\"raw\"",
            "'c'",
            "LIMIT",
            "Mode::Fast",
            "crate::limits::MAX",
            "None",
            "Default",
        ] {
            assert!(is_caller_independent(ok), "{ok}");
        }
        for no in [
            "limit",
            "v + 1",
            "f()",
            "self.max",
            "&x",
            "LIMIT + 1",
            "format!(\"{x}\")",
            "\"{x}\"",
            "Mode::from(x)",
        ] {
            assert!(!is_caller_independent(no), "{no}");
        }
    }

    #[test]
    fn the_report_names_the_value_and_the_calls() {
        let done = InlinedParameter {
            function: "clamp".into(),
            parameter: "max".into(),
            value: "LIMIT".into(),
            root: "/nonexistent".into(),
            file: "src/lib.rs".into(),
            rewritten_calls: 2,
            unmatched: vec![
                "src/lib.rs:9:5 (the function used as a value: it would change type)".into(),
            ],
            rewritten: vec![],
            diagnostics: vec![],
            applied: false,
        };
        let text = done.render(1000);
        assert!(text.contains("every call passes `LIMIT`"), "{text}");
        assert!(text.contains("2 call(s) lose the argument"), "{text}");
        assert!(
            text.contains("nothing is written while any remains"),
            "{text}"
        );
        assert!(text.contains("nothing was written"), "{text}");
    }
}
