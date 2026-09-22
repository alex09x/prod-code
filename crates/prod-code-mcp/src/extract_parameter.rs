//! Promoting an expression inside a function into a parameter of it.
//!
//! The expression leaves the body and becomes the argument every existing call site passes, so
//! the behaviour of every current caller is unchanged and the next one can choose. What makes
//! this different from bundling parameters is where it can go wrong: the expression may name
//! something that exists inside the function and nowhere else, and the report says so rather
//! than writing a call site that cannot compile.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the extraction did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtractedParameter {
    /// The function the parameter was added to.
    pub symbol: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub name: String,
    pub ty: String,
    /// The expression that left the body, as it was written.
    pub expression: String,
    /// How many places in the body now read the parameter.
    pub replaced: usize,
    pub call_sites: usize,
    pub rewritten: Vec<(String, String)>,
    pub unmatched: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl ExtractedParameter {
    /// The report: what moved out of the body, and whether the result compiles.
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- new parameter: `{}: {}`\n- from the body: `{}`\n- {} place(s) in the \
             body now read it, {} call site(s) pass it\n\n",
            self.symbol,
            self.file,
            self.name,
            self.ty,
            self.expression,
            self.replaced,
            self.call_sites
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
        if !self.unmatched.is_empty() {
            out.push_str(&format!(
                "\nnot given the argument ({} reference(s) that are not a call with this \
                 arity — a function pointer, a macro, or a call already changed):\n",
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
            // The expression travelled to places where its names may not exist. That is the
            // usual cause and it is not worth making the reader work it out.
            out.push_str(
                "\nthe expression is now written at every call site: if it names a local, a \
                 parameter or anything private to the function it came from, it cannot be \
                 spelled there. Extract something the callers can see.\n",
            );
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

/// The type in a hover answer, when it is one this can read.
///
/// rust-analyzer writes `let x: u32` for a binding and a bare path for a type, and neither
/// shape is reliable for an arbitrary expression — so a hover that does not parse is a reason
/// to ask the caller for the type rather than to guess at it.
pub fn type_from_hover(hover: &str) -> Option<String> {
    for line in hover.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("let ")
            && let Some((_, ty)) = rest.split_once(':')
        {
            let ty = ty.trim().trim_end_matches(&[',', ';'][..]).trim();
            if !ty.is_empty() {
                return Some(ty.to_string());
            }
        }
    }
    None
}

/// The smallest *function* containing `line`, and its line span.
///
/// Not the smallest declaration: `textDocument/documentSymbol` reports local bindings too, so
/// the innermost thing containing an expression is usually the `let` it is part of. Only a
/// function or a method can take a parameter, so only those are candidates.
pub fn enclosing_function(symbols: &serde_json::Value, line: u32) -> Option<(String, u32, u32)> {
    /// LSP `SymbolKind`: a free function, and a method on a type.
    const FUNCTION: u64 = 12;
    const METHOD: u64 = 6;

    fn walk(nodes: &[serde_json::Value], line: u32, best: &mut Option<(String, u32, u32)>) {
        for node in nodes {
            let range = node
                .get("range")
                .or_else(|| node.get("location").and_then(|l| l.get("range")));
            let kind = node.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            if let Some(range) = range
                && (kind == FUNCTION || kind == METHOD)
                && let (Some(s), Some(e)) = (
                    range.pointer("/start/line").and_then(|l| l.as_u64()),
                    range.pointer("/end/line").and_then(|l| l.as_u64()),
                )
            {
                let (s, e) = (s as u32 + 1, e as u32 + 1);
                if s <= line && line <= e && best.as_ref().is_none_or(|(_, bs, be)| e - s < be - bs)
                {
                    let name = node
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string();
                    *best = Some((name, s, e));
                }
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                walk(children, line, best);
            }
        }
    }
    let mut best = None;
    walk(symbols.as_array().map(|a| a.as_slice())?, line, &mut best);
    best
}

/// The parameter list with `param` added at the end.
pub fn with_parameter(list: &str, param: &str) -> String {
    let trimmed = list.trim();
    if trimmed.is_empty() {
        return param.to_string();
    }
    // A trailing comma means the list is written one per line; keep that shape, and keep
    // whatever whitespace sits between the last parameter and the closing parenthesis.
    if trimmed.ends_with(',') {
        let head = list.trim_end_matches(|c: char| c.is_whitespace());
        let tail = &list[head.len()..];
        let indent: String = head
            .lines()
            .next_back()
            .unwrap_or("")
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        return format!("{head}\n{indent}{param},{tail}");
    }
    format!("{trimmed}, {param}")
}

/// The argument list with `argument` added at the end.
pub fn with_argument(args: &str, argument: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return argument.to_string();
    }
    format!("{trimmed}, {argument}")
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Promotes the expression selected in `file` into a parameter of the function that contains it.
#[allow(clippy::too_many_arguments)]
pub async fn extract(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    start: (u32, u32),
    end: (u32, u32),
    name: &str,
    ty: Option<&str>,
    replace_all: bool,
    apply: bool,
    force: bool,
) -> Result<ExtractedParameter> {
    anyhow::ensure!(
        !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let from = crate::signature::offset_of(&text, start.0, start.1)
        .context("the selection does not start inside the file")?;
    let to = crate::signature::offset_of(&text, end.0, end.1)
        .context("the selection does not end inside the file")?;
    anyhow::ensure!(to > from, "the selection is empty");
    let expression = text[from..to].trim().to_string();
    anyhow::ensure!(!expression.is_empty(), "the selection is only whitespace");

    // The function the selection is inside, and its parameter list.
    let symbols = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": url::Url::from_file_path(file)
            .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?.to_string() } }),
    )
    .await?;
    let (callee, fn_start, fn_end) =
        enclosing_function(&symbols, start.0).context("the selection is not inside a function")?;
    let fn_offset = {
        let lines: Vec<&str> = text.lines().collect();
        let head = lines
            .get(fn_start as usize - 1)
            .context("the declaration's first line is not in the file")?;
        let at = head
            .find(&format!("fn {callee}"))
            .map(|i| i + 3)
            .with_context(|| format!("`{callee}` is not a function"))?;
        crate::signature::offset_of(&text, fn_start, at as u32 + 1)
            .context("the declaration is not where the analyzer put it")?
    };
    let (_, open, close) = crate::signature::param_span(&text, fn_offset)
        .with_context(|| format!("`{callee}` has no parameter list"))?;
    anyhow::ensure!(
        from > close,
        "the selection is in the signature, not in the body"
    );

    // The type: the caller's, or the one hover gives when it gives a shape this can read.
    let ty = match ty {
        Some(ty) => ty.to_string(),
        None => {
            let hover = crate::tools::execute_lsp_query(
                remote,
                root,
                file,
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": url::Url::from_file_path(file)
                        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?.to_string() },
                    "position": { "line": start.0.saturating_sub(1), "character": start.1.saturating_sub(1) },
                }),
            )
            .await
            .ok()
            .and_then(|h| {
                h.pointer("/contents/value")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default();
            type_from_hover(&hover).context(
                "the analyzer does not give a type for this selection in a shape this can read; \
                 pass the type explicitly",
            )?
        }
    };

    // Every edit against the file as it is, applied from the last offset backwards.
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let body_range = close..crate::signature::offset_of(&text, fn_end, 1).unwrap_or(text.len());
    let mut replaced = 0usize;
    if replace_all {
        let mut at = body_range.start;
        while let Some(i) = text[at..body_range.end.min(text.len())].find(&expression) {
            let hit = at + i;
            edits.entry(file.to_path_buf()).or_default().push((
                hit,
                expression.len(),
                name.to_string(),
            ));
            replaced += 1;
            at = hit + expression.len();
        }
    } else {
        edits
            .entry(file.to_path_buf())
            .or_default()
            .push((from, to - from, name.to_string()));
        replaced = 1;
    }
    edits.entry(file.to_path_buf()).or_default().push((
        open,
        close - open,
        with_parameter(&text[open..close], &format!("{name}: {ty}")),
    ));

    // Every call site passes what the body used to say.
    let (fn_line, fn_col) = crate::signature::line_col_at(&text, fn_offset);
    let mut unmatched = Vec::new();
    let mut call_sites = 0usize;
    for (path, rl, rc) in crate::signature::references(remote, root, file, fn_line, fn_col)
        .await
        .unwrap_or_default()
    {
        let body = if path == *file {
            text.clone()
        } else {
            std::fs::read_to_string(&path).unwrap_or_default()
        };
        let Some(at) = crate::signature::offset_of(&body, rl, rc) else {
            continue;
        };
        let Some((args_start, args_end)) =
            crate::parameter_object::call_args_span(&body, at + callee.len())
        else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, &path)));
            continue;
        };
        edits.entry(path).or_default().push((
            args_start,
            args_end - args_start,
            with_argument(&body[args_start..args_end], &expression),
        ));
        call_sites += 1;
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = if path == *file {
            text.clone()
        } else {
            std::fs::read_to_string(&path).unwrap_or_default()
        };
        file_edits.sort_by_key(|(at, _, _)| *at);
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
            "the change does not compile ({} error(s)); nothing was written. Extract something \
             the callers can see, or pass `force: true`:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(ExtractedParameter {
        symbol: callee,
        root: root.to_path_buf(),
        file: display(root, file),
        name: name.to_string(),
        ty,
        expression,
        replaced,
        call_sites,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        diagnostics,
        applied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_enclosing_function_is_not_the_let_the_expression_sits_in() {
        // What the analyzer really answers: the function, and the local inside it.
        let symbols = serde_json::json!([
            { "name": "move_item", "kind": 12,
              "range": { "start": { "line": 577 }, "end": { "line": 700 } },
              "children": [
                { "name": "removed", "kind": 13,
                  "range": { "start": { "line": 628 }, "end": { "line": 628 } } }
              ] }
        ]);
        assert_eq!(
            enclosing_function(&symbols, 629),
            Some(("move_item".to_string(), 578, 701)),
            "a local binding cannot take a parameter, so it is not a candidate"
        );
        assert_eq!(enclosing_function(&symbols, 900), None);
    }

    #[test]
    fn a_hover_that_names_a_binding_gives_its_type_and_anything_else_gives_none() {
        assert_eq!(
            type_from_hover("```rust\nlet decl_end: u32\n```").as_deref(),
            Some("u32")
        );
        assert_eq!(
            type_from_hover("```rust\nlet name: BTreeMap<String, u8>\n```").as_deref(),
            Some("BTreeMap<String, u8>")
        );
        assert_eq!(type_from_hover("```rust\ncore::str\n```"), None);
        assert_eq!(type_from_hover(""), None);
    }

    #[test]
    fn a_parameter_is_added_at_the_end_and_keeps_the_lists_shape() {
        assert_eq!(with_parameter("", "limit: usize"), "limit: usize");
        assert_eq!(
            with_parameter("a: u8, b: u8", "limit: usize"),
            "a: u8, b: u8, limit: usize"
        );
        // One per line, trailing comma: the new one keeps that shape and the indentation.
        assert_eq!(
            with_parameter("\n    a: u8,\n    b: u8,\n", "limit: usize"),
            "\n    a: u8,\n    b: u8,\n    limit: usize,\n"
        );
    }

    fn report(
        unmatched: Vec<String>,
        diagnostics: Vec<String>,
        applied: bool,
    ) -> ExtractedParameter {
        ExtractedParameter {
            symbol: "render".into(),
            root: PathBuf::from("/root"),
            file: "src/lib.rs".into(),
            name: "width_limit".into(),
            ty: "usize".into(),
            expression: "80".into(),
            replaced: 1,
            call_sites: 2,
            rewritten: vec![("/root/src/lib.rs".into(), "pub fn render() {}\n".into())],
            unmatched,
            diagnostics,
            applied,
        }
    }

    #[test]
    fn the_report_says_what_was_left_out_and_what_the_analyzer_thought() {
        let clean = report(Vec::new(), Vec::new(), false).render(4000);
        assert!(
            clean.contains("new parameter: `width_limit: usize`"),
            "{clean}"
        );
        assert!(clean.contains("2 call site(s) pass it"), "{clean}");
        assert!(
            clean.contains("the analyzer accepts the result: 0 errors"),
            "{clean}"
        );
        assert!(clean.contains("nothing was written"), "{clean}");

        let missed = report(vec!["src/other.rs:9:5".into()], Vec::new(), false).render(4000);
        assert!(
            missed.contains("not given the argument (1 reference"),
            "{missed}"
        );
        assert!(missed.contains("src/other.rs:9:5"), "{missed}");

        // A rejected result explains the usual cause rather than leaving a raw diagnostic.
        let broken = report(
            Vec::new(),
            vec!["cannot find value `n` [E0425] (src/lib.rs:8:17)".into()],
            false,
        )
        .render(4000);
        assert!(
            broken.contains("the analyzer rejects the result"),
            "{broken}"
        );
        assert!(broken.contains("if it names a local"), "{broken}");

        let written = report(Vec::new(), Vec::new(), true).render(4000);
        assert!(written.contains("[applied to 1 file(s)]"), "{written}");
        assert!(!written.contains("nothing was written"), "{written}");
    }

    #[test]
    fn a_diff_longer_than_the_budget_is_cut_and_says_so() {
        let cut = report(Vec::new(), Vec::new(), false).render(10);
        assert!(cut.contains("… diff truncated"), "{cut}");
    }

    #[test]
    fn an_argument_is_added_at_the_end_of_whatever_was_there() {
        assert_eq!(with_argument("", "64"), "64");
        assert_eq!(with_argument("a, b", "64"), "a, b, 64");
        assert_eq!(with_argument("  a  ", "64"), "a, 64");
    }
}
