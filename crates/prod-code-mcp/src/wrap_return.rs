//! Wrapping what a function returns in `Option` or `Result`, with every caller.
//!
//! rust-analyzer's `wrap_return_type_in_option` / `wrap_return_type_in_result` rewrite the
//! signature and every value the function returns, and touch no caller: after it, every call site
//! is a type error. This does the other half. A caller that itself returns an `Option` (or a
//! `Result`) gets `?` after the call; any other caller cannot, and is reported with its line,
//! because turning a `None` or an error into something else there is a decision, not a rewrite.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Which wrapper the return type gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Wrapper {
    Option,
    Result,
}

impl Wrapper {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "option" | "Option" => Ok(Self::Option),
            "result" | "Result" => Ok(Self::Result),
            other => anyhow::bail!("`{other}` is not a wrapper: use `option` or `result`"),
        }
    }

    fn assist_id(self) -> &'static str {
        match self {
            Self::Option => "wrap_return_type_in_option",
            Self::Result => "wrap_return_type_in_result",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Option => "Option",
            Self::Result => "Result",
        }
    }
}

/// What the wrapping did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WrappedReturn {
    pub function: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub was: String,
    pub now: String,
    /// Call sites that got `?`.
    pub propagated: usize,
    /// Call sites whose caller cannot propagate, with the reason.
    pub blocked: Vec<String>,
    pub unmatched: Vec<String>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl WrappedReturn {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- returned: `{}`\n- now returns: `{}`\n- {} call site(s) propagate with `?`\n\n",
            self.function, self.file, self.was, self.now, self.propagated
        );
        let mut body = String::new();
        let mut changed_lines = 0usize;
        for (path, new_text) in &self.rewritten {
            let old_text = std::fs::read_to_string(path).unwrap_or_default();
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
                "\n{} call site(s) cannot propagate: the calling function does not return {}. \
                 Each needs a decision — unwrap, match, or wrap that caller too:\n",
                self.blocked.len(),
                if self.now.starts_with("Option") {
                    "an `Option`"
                } else {
                    "a `Result`"
                }
            ));
            for b in &self.blocked {
                out.push_str(&format!("  {b}\n"));
            }
        }
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot rewritten ({} reference(s) this could not read as a call):\n",
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

/// The return type a function header declares between its parameter list's `)` at `close` and
/// its body's `{`, or `None` for a function that returns `()` implicitly.
pub fn declared_return(text: &str, close: usize) -> Option<(usize, usize)> {
    let body = text[close..].find(['{', ';']).map(|i| close + i)?;
    let header = &text[close + 1..body];
    let arrow = header.find("->")?;
    let start = close + 1 + arrow + 2;
    let mut end = body;
    if let Some(w) = text[start..body].find(" where") {
        end = start + w;
    }
    let lead = text[start..end].len() - text[start..end].trim_start().len();
    let trail = text[start..end].len() - text[start..end].trim_end().len();
    Some((start + lead, end - trail))
}

/// The innermost function whose body contains `at`, and the return type its header declares
/// (`()` when it declares none).
pub fn enclosing_return_type(text: &str, at: usize) -> Option<String> {
    let mut search = at;
    while let Some(fn_at) = text[..search].rfind("fn ") {
        search = fn_at;
        if text[..fn_at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let Some((_, _, close)) = crate::signature::param_span(text, fn_at + 3) else {
            continue;
        };
        let Some(open) = text[close..].find(['{', ';']).map(|i| close + i) else {
            continue;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        let Some(end) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        if open < at && at < end {
            return Some(
                declared_return(text, close)
                    .map(|(s, e)| text[s..e].to_string())
                    .unwrap_or_else(|| "()".to_string()),
            );
        }
    }
    None
}

/// Whether a function returning `ty` can apply `?` to a value wrapped in `wrapper`.
pub fn propagates(ty: &str, wrapper: Wrapper) -> bool {
    let head = ty.trim().split('<').next().unwrap_or("").trim();
    let last = head.rsplit("::").next().unwrap_or(head);
    match wrapper {
        Wrapper::Option => last == "Option",
        Wrapper::Result => last == "Result",
    }
}

/// Wraps the return type of the function declared at `line`:`col` of `file` and makes every caller
/// that can propagate do so with `?`.
#[allow(clippy::too_many_arguments)]
pub async fn wrap(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    wrapper: Wrapper,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    // The name the position is in, however far into it.
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..start].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, _, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    let (ret_start, ret_end) = declared_return(&text, close).with_context(|| {
        format!("`{name}` returns `()` implicitly; declare `-> ()` first if that is what is meant")
    })?;
    let was = text[ret_start..ret_end].to_string();
    anyhow::ensure!(
        !propagates(&was, wrapper),
        "`{name}` already returns a `{}`",
        wrapper.name()
    );
    let error =
        match wrapper {
            Wrapper::Result => Some(error.map(str::trim).filter(|e| !e.is_empty()).context(
                "pass `error`: the type a `Result` fails with, such as `anyhow::Error`",
            )?),
            Wrapper::Option => None,
        };

    // rust-analyzer's half: the signature and every returned value.
    let (rl, rc) = crate::signature::line_col_at(&text, ret_start);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let edit = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "prodCode/applyAssist",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": rl - 1, "character": rc - 1 },
                "end": { "line": rl - 1, "character": rc - 1 }
            },
            "id": wrapper.assist_id(),
        }),
    )
    .await
    .with_context(|| format!("rust-analyzer does not wrap the return type of `{name}` here"))?;
    let (planned, _) = crate::refactor::planned_texts(root, &edit)?;
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut new_decl = planned
        .into_iter()
        .find(|(p, _)| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == canonical)
        .map(|(_, t)| t)
        .context("the assist did not rewrite the declaring file")?;
    // `Result<T, _>` gets the error type it was given.
    let now = match error {
        Some(error) => {
            let sig_at = new_decl.find(&format!("fn {name}")).unwrap_or(0);
            let body_at = new_decl[sig_at..]
                .find('{')
                .map_or(new_decl.len(), |i| sig_at + i);
            let hole = new_decl[sig_at..body_at]
                .rfind(", _>")
                .map(|i| sig_at + i)
                .context("the wrapped signature has no `_` error type to fill in")?;
            new_decl.replace_range(hole..hole + ", _>".len(), &format!(", {error}>"));
            format!("Result<{was}, {error}>")
        }
        None => format!("Option<{was}>"),
    };

    // Where the declaring file did not change, a position means the same thing before and after.
    let prefix = text
        .bytes()
        .zip(new_decl.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = text
        .bytes()
        .rev()
        .zip(new_decl.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(text.len() - prefix);
    let delta = new_decl.len() as isize - text.len() as isize;

    // The function's own span in the file as it is: a call inside it is a recursive call, whose
    // text the assist itself rewrote.
    let own_end = text[close..]
        .find('{')
        .and_then(|i| crate::parameter_object::matching_bracket(&text, close + i))
        .unwrap_or(close);
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut edits: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    let mut propagated = 0usize;
    let mut blocked = Vec::new();
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
        if !body[at..].starts_with(name.as_str()) {
            unmatched.push(format!(
                "{site} (the analyzer places `{name}` here, but the file says otherwise)"
            ));
            continue;
        }
        let Some((_, args_end)) = crate::parameter_object::call_args_span(&body, at + name.len())
        else {
            unmatched.push(format!("{site} (not a call: a function used as a value)"));
            continue;
        };
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        if same_file && start < at && at < own_end {
            unmatched.push(format!(
                "{site} (a call inside `{name}` itself: add `?` there by hand)"
            ));
            continue;
        }
        let caller = enclosing_return_type(&body, at).unwrap_or_else(|| "()".to_string());
        if !propagates(&caller, wrapper) {
            let line_text = body[body[..at].rfind('\n').map_or(0, |i| i + 1)..]
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            blocked.push(format!(
                "{site} the caller returns `{caller}`: `{line_text}`"
            ));
            continue;
        }
        let insert_at = args_end + 1;
        // Elsewhere, and before the part the assist rewrote, a position is unchanged.
        let mapped = if !same_file || insert_at <= prefix {
            insert_at
        } else if insert_at >= text.len() - suffix {
            (insert_at as isize + delta) as usize
        } else {
            unmatched.push(format!(
                "{site} (a call inside `{name}` itself: add `?` there by hand)"
            ));
            continue;
        };
        edits.entry(path.clone()).or_default().push(mapped);
        propagated += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), new_decl);
    for (path, mut spots) in edits {
        let same_file = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()) == canonical;
        let key = if same_file {
            file.to_path_buf()
        } else {
            path.clone()
        };
        let mut body = rewritten
            .get(&key)
            .cloned()
            .unwrap_or_else(|| texts.get(&path).cloned().unwrap_or_default());
        spots.sort_unstable();
        for spot in spots.into_iter().rev() {
            body.insert(spot, '?');
        }
        rewritten.insert(key, body);
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
            blocked.is_empty() || force,
            "{} call site(s) cannot propagate with `?`; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
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

    Ok(WrappedReturn {
        function: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now,
        propagated,
        blocked,
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
    fn a_wrapper_is_named_either_way_and_nothing_else() {
        assert_eq!(Wrapper::parse("option").unwrap(), Wrapper::Option);
        assert_eq!(Wrapper::parse("Result").unwrap(), Wrapper::Result);
        assert!(Wrapper::parse("either").is_err());
        assert_eq!(Wrapper::Option.assist_id(), "wrap_return_type_in_option");
        assert_eq!(Wrapper::Result.assist_id(), "wrap_return_type_in_result");
    }

    #[test]
    fn the_declared_return_type_is_found_between_the_arrow_and_the_body() {
        let text = "pub fn plain(a: u32) -> Vec<u32> where u32: Copy {\n    vec![a]\n}\n";
        let close = text.find(')').unwrap();
        let (s, e) = declared_return(text, close).unwrap();
        assert_eq!(&text[s..e], "Vec<u32>");
        let unit = "fn f() {}\n";
        assert!(declared_return(unit, unit.find(')').unwrap()).is_none());
    }

    #[test]
    fn the_caller_is_the_innermost_function_around_the_call() {
        let text = "fn outer() -> Option<u32> {\n    fn inner() -> u32 {\n        plain()\n    }\n    Some(plain()?)\n}\nfn unit() {\n    plain();\n}\n";
        let first = text.find("plain()").unwrap();
        assert_eq!(enclosing_return_type(text, first).as_deref(), Some("u32"));
        let second = text[first + 1..].find("plain()").unwrap() + first + 1;
        assert_eq!(
            enclosing_return_type(text, second).as_deref(),
            Some("Option<u32>")
        );
        let third = text.rfind("plain()").unwrap();
        assert_eq!(enclosing_return_type(text, third).as_deref(), Some("()"));
        assert_eq!(enclosing_return_type("plain()", 0), None);
    }

    #[test]
    fn only_a_matching_wrapper_can_propagate() {
        assert!(propagates("Option<u32>", Wrapper::Option));
        assert!(propagates("std::option::Option<u32>", Wrapper::Option));
        assert!(propagates("anyhow::Result<()>", Wrapper::Result));
        assert!(propagates("Result<u32, String>", Wrapper::Result));
        assert!(!propagates("Result<u32, String>", Wrapper::Option));
        assert!(!propagates("u32", Wrapper::Result));
        assert!(!propagates("()", Wrapper::Option));
    }

    fn report() -> WrappedReturn {
        WrappedReturn {
            function: "plain".into(),
            root: "/root".into(),
            file: "src/lib.rs".into(),
            was: "u32".into(),
            now: "Result<u32, String>".into(),
            propagated: 2,
            blocked: vec!["src/app.rs:4:5 the caller returns `u32`: `plain()`".into()],
            unmatched: vec![
                "src/lib.rs:9:5 (a call inside `plain` itself: add `?` there by hand)".into(),
            ],
            rewritten: vec![("/root/src/lib.rs".into(), "fn main() {}\n".into())],
            diagnostics: vec!["mismatched types (src/app.rs:4:5)".into()],
            applied: false,
        }
    }

    #[test]
    fn the_report_names_what_propagates_and_what_needs_a_decision() {
        let text = report().render(10_000);
        assert!(
            text.contains("returned: `u32`") && text.contains("now returns: `Result<u32, String>`"),
            "{text}"
        );
        assert!(text.contains("2 call site(s) propagate"), "{text}");
        assert!(text.contains("does not return a `Result`"), "{text}");
        assert!(text.contains("a call inside `plain` itself"), "{text}");
        assert!(text.contains("the analyzer rejects the result"), "{text}");
        assert!(text.contains("nothing was written"), "{text}");
        let mut done = report();
        done.blocked.clear();
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
    }
}
