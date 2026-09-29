//! Wrapping what a function returns in `Option`, `Result`, `Promise`, or `Pointer`, with callers.
//!
//! rust-analyzer's `wrap_return_type_in_option` / `wrap_return_type_in_result` rewrite the
//! signature and every value the function returns for Rust, touching no caller. Here, for Rust:
//! a caller that itself returns an `Option` (or a `Result`) gets `?` after the call; any other
//! caller cannot, and is reported with its line, because turning a `None` or an error into
//! something else there is a decision, not a rewrite.
//!
//! Across TypeScript/JavaScript, Python, C++, Swift, and Go:
//! - TypeScript/JavaScript: supports `promise` (`Promise<T>`, adding `async` to declaration,
//!   rewriting callers to `await call(...)`), `option`/`nullable` (`T | null`), `result` (`Result<T, E>`).
//! - Python: supports `option`/`optional` (`Optional[T]`), `result` (`Result[T, E]`, wrapping return values in `Ok(...)`).
//! - C++: supports `option`/`optional` (`std::optional<T>`), `result`/`expected` (`std::expected<T, E>`).
//! - Swift: supports `option`/`optional` (`T?`), `result` (`Result<T, Error>`, wrapping return values in `.success(...)`).
//! - Go: supports `result`/`error` (`(T, error)` with `return expr, nil`), `pointer`/`option` (`*T`).
//!
//! Refuses when the function already returns the target wrapper, or when callers cannot
//! propagate without `force`.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use crate::parameter_object::Language;

/// Which wrapper the return type gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wrapper {
    Option,
    Result,
    Promise,
    Pointer,
    Custom(String),
}

impl serde::Serialize for Wrapper {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Option => serializer.serialize_str("option"),
            Self::Result => serializer.serialize_str("result"),
            Self::Promise => serializer.serialize_str("promise"),
            Self::Pointer => serializer.serialize_str("pointer"),
            Self::Custom(name) => serializer.serialize_str(name),
        }
    }
}

impl Wrapper {
    pub fn parse(text: &str) -> Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            anyhow::bail!("wrapper cannot be empty: use `option`, `result`, `promise`, `pointer`, or a custom envelope type name");
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "option" | "optional" | "nullable" => Ok(Self::Option),
            "result" | "expected" | "error" => Ok(Self::Result),
            "promise" | "future" | "async" => Ok(Self::Promise),
            "pointer" | "ptr" => Ok(Self::Pointer),
            _ => Ok(Self::Custom(trimmed.to_string())),
        }
    }

    pub fn assist_id(&self) -> &'static str {
        match self {
            Self::Option => "wrap_return_type_in_option",
            Self::Result => "wrap_return_type_in_result",
            _ => "wrap_return_type_in_option",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Option => "Option",
            Self::Result => "Result",
            Self::Promise => "Promise",
            Self::Pointer => "Pointer",
            Self::Custom(name) => name.as_str(),
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
    /// Call sites that got `?` (or `await`, etc.).
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
        let prop_note = if self.now.starts_with("Promise") {
            format!("{} call site(s) propagate with `await`", self.propagated)
        } else if self.now.starts_with("Option") || self.now.starts_with("Result") {
            format!("{} call site(s) propagate with `?`", self.propagated)
        } else {
            format!("{} call site(s) propagate", self.propagated)
        };
        let mut out = format!(
            "`{}` ({})\n\n- returned: `{}`\n- now returns: `{}`\n- {}\n\n",
            self.function, self.file, self.was, self.now, prop_note
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
        if !self.blocked.is_empty() {
            let target_desc = if self.now.starts_with("Option") {
                "an `Option`"
            } else if self.now.starts_with("Result") {
                "a `Result`"
            } else if self.now.starts_with("Promise") {
                "a `Promise`"
            } else {
                &self.now
            };
            out.push_str(&format!(
                "\n{} call site(s) cannot propagate: the calling function does not return {target_desc}. \
                 Each needs a decision — unwrap, match, or wrap that caller too:\n",
                self.blocked.len()
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

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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
/// Whether a function returning `ty` can apply `?` (or propagate) a value wrapped in `wrapper`.
pub fn propagates(ty: &str, wrapper: &Wrapper) -> bool {
    let head = ty.trim().split('<').next().unwrap_or("").trim();
    let head = head.split('[').next().unwrap_or(head).trim();
    let last = head.rsplit("::").next().unwrap_or(head);
    let last = last.rsplit('.').next().unwrap_or(last).trim();
    match wrapper {
        Wrapper::Option => {
            last == "Option"
                || last == "Optional"
                || ty.trim().ends_with('?')
                || ty.trim().contains("| null")
                || ty.trim().contains("| None")
                || ty.trim().starts_with('*')
        }
        Wrapper::Result => last == "Result" || last == "expected" || last == "error",
        Wrapper::Promise => last == "Promise" || last == "Future",
        Wrapper::Pointer => ty.trim().starts_with('*'),
        Wrapper::Custom(custom_name) => {
            let custom_head = custom_name.trim().split('<').next().unwrap_or(custom_name).trim();
            let custom_head = custom_head.split('[').next().unwrap_or(custom_head).trim();
            let custom_last = custom_head.rsplit("::").next().unwrap_or(custom_head);
            let custom_last = custom_last.rsplit('.').next().unwrap_or(custom_last).trim();
            last == custom_last || ty.contains(custom_last)
        }
    }
}

/// Formats a constructor or factory call for a wrapped return expression.
fn format_constructor_call(
    constructor: Option<&str>,
    default_base: &str,
    expr: &str,
    lang: Language,
    was: &str,
) -> String {
    let expr = expr.trim();
    if let Some(ctor) = constructor {
        let ctor = ctor.trim();
        if ctor.contains("{expr}") {
            return ctor.replace("{expr}", expr);
        }
        if ctor.contains("{}") {
            return ctor.replace("{}", expr);
        }
        if expr.is_empty() {
            return format!("{ctor}()");
        }
        return format!("{ctor}({expr})");
    }

    match lang {
        Language::Rust => {
            if expr.is_empty() {
                format!("{default_base}::new()")
            } else {
                format!("{default_base}::new({expr})")
            }
        }
        Language::TypeScript | Language::JavaScript => {
            if expr.is_empty() {
                format!("new {default_base}()")
            } else {
                format!("new {default_base}({expr})")
            }
        }
        Language::Python => {
            if expr.is_empty() {
                format!("{default_base}()")
            } else {
                format!("{default_base}({expr})")
            }
        }
        Language::Cpp | Language::C => {
            if expr.is_empty() {
                format!("{default_base}()")
            } else if !was.is_empty() && was != "void" {
                format!("{default_base}<{was}>({expr})")
            } else {
                format!("{default_base}({expr})")
            }
        }
        Language::Swift => {
            if expr.is_empty() {
                format!("{default_base}()")
            } else {
                format!("{default_base}({expr})")
            }
        }
        Language::Go => {
            let clean_base = default_base.trim_start_matches('*');
            if expr.is_empty() {
                format!("&{clean_base}{{}}")
            } else {
                format!("&{clean_base}{{Data: {expr}}}")
            }
        }
    }
}

/// Rewrites explicit return statements and the trailing expression in a Rust function body.
fn rewrite_rust_body(
    body: &str,
    constructor: Option<&str>,
    envelope_base: &str,
    was: &str,
) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'r' if body[i..].starts_with("return") => {
                let before = if i > 0 { bytes[i - 1] as char } else { ' ' };
                let after = if i + 6 < bytes.len() { bytes[i + 6] as char } else { ' ' };
                if !is_ident(before) && !is_ident(after) {
                    let end_stmt = body[i..].find(';').map_or(body.len(), |e| i + e);
                    let ret_stmt = &body[i..end_stmt];
                    let expr = ret_stmt.strip_prefix("return").unwrap().trim();
                    let wrapped = format_constructor_call(constructor, envelope_base, expr, Language::Rust, was);
                    edits.push((i, end_stmt - i, format!("return {wrapped}")));
                    i = end_stmt;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Check if the body has a tail expression (not terminated by semicolon)
    let trimmed_body = body.trim_end();
    if !trimmed_body.is_empty() && !trimmed_body.ends_with(';') {
        let mut last_boundary = 0;
        let mut d = 0;
        let end_idx = trimmed_body.len();
        let mut in_str = false;
        let mut in_line_comment = false;
        let mut in_block_comment = false;
        let mut escape = false;

        for (idx, &b) in bytes.iter().enumerate().take(end_idx) {
            if in_line_comment {
                if b == b'\n' {
                    in_line_comment = false;
                }
                continue;
            }
            if in_block_comment {
                if b == b'/' && idx > 0 && bytes[idx - 1] == b'*' {
                    in_block_comment = false;
                }
                continue;
            }
            if in_str {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    in_str = false;
                }
                continue;
            }
            if b == b'"' {
                in_str = true;
                continue;
            }
            if b == b'/' && idx + 1 < end_idx {
                if bytes[idx + 1] == b'/' {
                    in_line_comment = true;
                    continue;
                } else if bytes[idx + 1] == b'*' {
                    in_block_comment = true;
                    continue;
                }
            }
            match b {
                b'{' => d += 1,
                b'}' => {
                    d -= 1;
                    if d == 0 && idx + 1 < end_idx {
                        last_boundary = idx + 1;
                    }
                }
                b';' if d == 0 => {
                    last_boundary = idx + 1;
                }
                _ => {}
            }
        }
        let tail_slice = &body[last_boundary..end_idx];
        let tail_trimmed = tail_slice.trim();
        if !tail_trimmed.is_empty() && !tail_trimmed.starts_with("return") {
            let lead_ws = tail_slice.len() - tail_slice.trim_start().len();
            let tail_start = last_boundary + lead_ws;
            let tail_end = tail_start + tail_trimmed.len();
            let wrapped = format_constructor_call(constructor, envelope_base, tail_trimmed, Language::Rust, was);
            edits.push((tail_start, tail_end - tail_start, wrapped));
        }
    }

    let mut out = body.to_string();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (start, len, repl) in edits {
        out.replace_range(start..start + len, &repl);
    }
    out
}

/// Wraps the return type of the function declared at `line`:`col` (or `symbol`) of `file` for Rust.
#[allow(clippy::too_many_arguments)]
pub async fn wrap_rust(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: u32,
    col: u32,
    wrapper: Wrapper,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    wrap_rust_ext(
        remote,
        root,
        file,
        symbol,
        line,
        col,
        wrapper,
        None,
        error,
        apply,
        force,
    )
    .await
}

/// Wraps the return type of a Rust function with optional custom constructor.
#[allow(clippy::too_many_arguments)]
pub async fn wrap_rust_ext(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: u32,
    col: u32,
    wrapper: Wrapper,
    constructor: Option<&str>,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = if line > 0 && col > 0 {
        crate::signature::offset_of(&text, line, col)
            .context("the position is not inside the file")?
    } else if let Some(sym) = symbol {
        let needle = format!("fn {sym}");
        let pos = text.find(&needle).with_context(|| {
            format!("function `{sym}` not found in {}", file.display())
        })?;
        pos + 3
    } else {
        anyhow::bail!("provide either line and character or symbol");
    };
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
        !propagates(&was, &wrapper),
        "`{name}` already returns a `{}`",
        wrapper.name()
    );
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());

    let (new_decl, now) = match &wrapper {
        Wrapper::Custom(custom_name) => {
            let base_name = custom_name.split(['<', '[']).next().unwrap_or(custom_name).trim();
            let base_name = base_name.rsplit("::").next().unwrap_or(base_name).trim();
            let now = if custom_name.contains('<') {
                custom_name.replace("<T>", &format!("<{was}>")).replace("<>", &format!("<{was}>"))
            } else {
                format!("{custom_name}<{was}>")
            };
            let body_open = text[close..]
                .find('{')
                .map(|i| close + i)
                .context("function declaration has no body")?;
            let body_close = crate::parameter_object::matching_bracket(&text, body_open)
                .context("unmatched bracket in function body")?;
            let body_text = &text[body_open + 1..body_close];
            let rewritten_body = rewrite_rust_body(body_text, constructor, base_name, &was);
            let mut new_text = text.clone();
            new_text.replace_range(body_open + 1..body_close, &rewritten_body);
            new_text.replace_range(ret_start..ret_end, &now);
            (new_text, now)
        }
        Wrapper::Option | Wrapper::Result => {
            let error = match wrapper {
                Wrapper::Result => Some(error.map(str::trim).filter(|e| !e.is_empty()).context(
                    "pass `error`: the type a `Result` fails with, such as `anyhow::Error`",
                )?),
                _ => None,
            };

            let (rl, rc) = crate::signature::position_at(&text, ret_start)?;
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
            let mut decl_text = planned
                .into_iter()
                .find(|(p, _)| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == canonical)
                .map(|(_, t)| t)
                .context("the assist did not rewrite the declaring file")?;
            let now = match error {
                Some(error) => {
                    let sig_at = decl_text.find(&format!("fn {name}")).unwrap_or(0);
                    let body_at = decl_text[sig_at..]
                        .find('{')
                        .map_or(decl_text.len(), |i| sig_at + i);
                    let hole = decl_text[sig_at..body_at]
                        .rfind(", _>")
                        .map(|i| sig_at + i)
                        .context("the wrapped signature has no `_` error type to fill in")?;
                    decl_text.replace_range(hole..hole + ", _>".len(), &format!(", {error}>"));
                    format!("Result<{was}, {error}>")
                }
                None => format!("Option<{was}>"),
            };
            (decl_text, now)
        }
        _ => anyhow::bail!("Rust wrap_return supports `option`, `result`, or a custom envelope type"),
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
    let (nl, nc) = crate::signature::position_at(&text, start)?;
    let mut refs = crate::signature::references(remote, root, file, nl, nc)
        .await
        .unwrap_or_default();

    if refs.is_empty() {
        for entry in ignore::WalkBuilder::new(root).build().flatten() {
            let p = entry.path();
            if p.is_file()
                && p.extension().is_some_and(|ext| ext == "rs")
                && let Ok(content) = std::fs::read_to_string(p)
                && content.contains(&name)
            {
                for (idx, _) in content.match_indices(&name) {
                    if idx > 0 && is_ident(content[..idx].chars().next_back().unwrap()) {
                        continue;
                    }
                    if content[idx + name.len()..].starts_with(is_ident) {
                        continue;
                    }
                    if p == file && idx >= start && idx <= close {
                        continue;
                    }
                    if let Ok((l, c)) = crate::signature::position_at(&content, idx) {
                        refs.push((p.to_path_buf(), l, c));
                    }
                }
            }
        }
    }

    for (path, l, c) in refs {
        let body = crate::refactor::referenced_text(&mut texts, &path)?.clone();
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
        if crate::inline_parameter::is_in_comment(&body, at, Language::Rust) {
            continue;
        }
        if crate::inline_parameter::is_import_or_export_context(&body, at, Language::Rust) {
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
                "{site} (a call inside `{name}` itself: add wrapper there by hand)"
            ));
            continue;
        }
        let caller = enclosing_return_type(&body, at).unwrap_or_else(|| "()".to_string());
        if !propagates(&caller, &wrapper) {
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
        if matches!(wrapper, Wrapper::Option | Wrapper::Result) {
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
        }
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
            "{} call site(s) cannot propagate; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not rewritten; nothing was written:\n  {}",
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

/// Function declaration info for non-Rust languages.
#[derive(Debug, Clone)]
pub struct PolyglotFuncDecl {
    pub name: String,
    pub decl_start: usize,
    pub name_start: usize,
    pub close_paren: usize,
    pub body_open: usize,
    pub body_close: usize,
    pub was: String,
    pub ret_span: Option<(usize, usize)>,
    pub is_async: bool,
    pub is_arrow: bool,
    pub has_return_type: bool,
}

/// Finds the declaration info for a function in non-Rust languages.
pub fn find_polyglot_decl(
    text: &str,
    lang: Language,
    symbol: Option<&str>,
    line: Option<u32>,
) -> Result<PolyglotFuncDecl> {
    let clean_name = symbol
        .map(|f| {
            f.rsplit_once("::")
                .map(|(_, m)| m)
                .or_else(|| f.rsplit_once('.').map(|(_, m)| m))
                .unwrap_or(f)
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
        .context("could not determine function name to wrap return value for")?;

    for (name_idx, _) in text.match_indices(&clean_name) {
        if name_idx > 0 && text[..name_idx].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let after_name = &text[name_idx + clean_name.len()..];
        if after_name.chars().next().is_some_and(is_ident) {
            continue;
        }

        let line_start = text[..name_idx].rfind('\n').map_or(0, |p| p + 1);
        let before_on_line = text[line_start..name_idx].trim_start();
        if crate::inline_parameter::is_in_comment(text, name_idx, lang) {
            continue;
        }

        let line_trimmed = before_on_line.trim_start();
        if line_trimmed.starts_with("import ")
            || line_trimmed.starts_with("import{")
            || line_trimmed.starts_with("from ")
            || line_trimmed.starts_with("export {")
            || line_trimmed.starts_with("export{")
            || line_trimmed.starts_with("export *")
            || line_trimmed.starts_with("use ")
            || line_trimmed.starts_with("#include")
        {
            continue;
        }

        // Multi-line import or export statement context
        let search_start = name_idx.saturating_sub(500);
        let before_sub = &text[search_start..name_idx];
        if let Some(imp_pos) = before_sub.rfind("import ").or_else(|| before_sub.rfind("export {")) {
            let between = &before_sub[imp_pos..];
            if between.contains('{') && !between.contains('}') {
                continue;
            }
        }

        // Skip call/expression contexts
        let before_trimmed = before_on_line.trim_end();
        if before_trimmed.ends_with('=')
            || before_trimmed.ends_with('+')
            || before_trimmed.ends_with('-')
            || before_trimmed.ends_with('*')
            || before_trimmed.ends_with('/')
            || before_trimmed.ends_with(',')
            || before_trimmed.ends_with('(')
            || before_trimmed.ends_with(':')
            || before_trimmed.ends_with("return")
            || before_trimmed.ends_with("throw")
            || before_trimmed.ends_with("await")
        {
            continue;
        }

        // Check if followed by `(` (or generic `<...>(`)
        let trimmed_after = after_name.trim_start();
        let open_paren = if trimmed_after.starts_with('(') {
            name_idx + clean_name.len() + (after_name.len() - trimmed_after.len())
        } else if trimmed_after.starts_with('<') {
            if let Some(end_gen) = trimmed_after.find('>') {
                let rest_after_gen = trimmed_after[end_gen + 1..].trim_start();
                if rest_after_gen.starts_with('(') {
                    name_idx + clean_name.len() + (after_name.len() - rest_after_gen.len())
                } else {
                    continue;
                }
            } else {
                continue;
            }
        } else if (lang == Language::TypeScript || lang == Language::JavaScript)
            && (before_on_line.starts_with("const ") || before_on_line.starts_with("let ") || before_on_line.starts_with("var "))
        {
            // Arrow function e.g. `const fn = (...) =>`
            let Some(eq_pos) = after_name.find('=') else { continue };
            let after_eq = after_name[eq_pos + 1..].trim_start();
            let after_async = after_eq.strip_prefix("async ").unwrap_or(after_eq).trim_start();
            if after_async.starts_with('(') {
                name_idx + clean_name.len() + (after_name.len() - after_async.len())
            } else {
                continue;
            }
        } else {
            continue;
        };

        let Some(close_paren) = crate::parameter_object::matching_bracket(text, open_paren) else {
            continue;
        };

        let decl_start = line_start;
        let is_async = text[decl_start..open_paren].contains("async");

        if lang == Language::Python {
            let Some(colon_rel) = text[close_paren..].find(':') else { continue };
            let colon = close_paren + colon_rel;
            let between = text[close_paren + 1..colon].trim();
            let (was, ret_span, has_return_type) = if let Some(arr_pos) = between.find("->") {
                let r = between[arr_pos + 2..].trim();
                let abs_s = close_paren + 1 + (text[close_paren + 1..colon].find("->").unwrap() + 2);
                let abs_start = abs_s + (text[abs_s..colon].len() - text[abs_s..colon].trim_start().len());
                let abs_end = colon - (text[abs_s..colon].len() - text[abs_s..colon].trim_end().len());
                (r.to_string(), Some((abs_start, abs_end)), true)
            } else {
                ("None".to_string(), None, false)
            };
            let body_open = colon;
            let body_close = crate::inline_parameter::find_python_body_close(text, decl_start, colon);
            return Ok(PolyglotFuncDecl {
                name: clean_name,
                decl_start,
                name_start: name_idx,
                close_paren,
                body_open,
                body_close,
                was,
                ret_span,
                is_async,
                is_arrow: false,
                has_return_type,
            });
        }

        // C-style braces
        let Some(open_brace_rel) = text[close_paren..].find('{') else { continue };
        let open_brace = close_paren + open_brace_rel;
        let Some(body_close) = crate::parameter_object::matching_bracket(text, open_brace) else { continue };

        let header_slice = &text[close_paren + 1..open_brace];
        if header_slice.contains(';') {
            continue;
        }
        let is_arrow = header_slice.contains("=>");

        let (was, ret_span, has_return_type) = match lang {
            Language::TypeScript | Language::JavaScript => {
                let end_header = if is_arrow {
                    close_paren + 1 + header_slice.find("=>").unwrap()
                } else {
                    open_brace
                };
                if let Some(c_rel) = text[close_paren + 1..end_header].find(':') {
                    let c_pos = close_paren + 1 + c_rel;
                    let ret_raw = text[c_pos + 1..end_header].trim();
                    let s_start = c_pos + 1 + (text[c_pos + 1..end_header].len() - text[c_pos + 1..end_header].trim_start().len());
                    let s_end = end_header - (text[c_pos + 1..end_header].len() - text[c_pos + 1..end_header].trim_end().len());
                    (ret_raw.to_string(), Some((s_start, s_end)), true)
                } else {
                    let def_ret = if lang == Language::TypeScript { "void" } else { "" };
                    (def_ret.to_string(), None, false)
                }
            }
            Language::Swift => {
                if let Some(arr_pos) = header_slice.find("->") {
                    let ret_raw = header_slice[arr_pos + 2..].trim();
                    let s_pos = close_paren + 1 + arr_pos + 2;
                    let s_start = s_pos + (text[s_pos..open_brace].len() - text[s_pos..open_brace].trim_start().len());
                    let s_end = open_brace - (text[s_pos..open_brace].len() - text[s_pos..open_brace].trim_end().len());
                    (ret_raw.to_string(), Some((s_start, s_end)), true)
                } else {
                    ("Void".to_string(), None, false)
                }
            }
            Language::Go => {
                let ret_raw = header_slice.trim();
                if !ret_raw.is_empty() {
                    let s_pos = close_paren + 1;
                    let s_start = s_pos + (header_slice.len() - header_slice.trim_start().len());
                    let s_end = open_brace - (header_slice.len() - header_slice.trim_end().len());
                    (ret_raw.to_string(), Some((s_start, s_end)), true)
                } else {
                    (String::new(), None, false)
                }
            }
            Language::Cpp | Language::C => {
                let before_name = text[decl_start..name_idx].trim();
                let words: Vec<&str> = before_name.split_whitespace().collect();
                let ret_raw = words.join(" ");
                let s_start = decl_start + (text[decl_start..name_idx].len() - before_name.len());
                let s_end = name_idx - (text[decl_start..name_idx].len() - text[decl_start..name_idx].trim_end().len());
                (ret_raw, Some((s_start, s_end)), true)
            }
            Language::Python | Language::Rust => unreachable!(),
        };

        return Ok(PolyglotFuncDecl {
            name: clean_name,
            decl_start,
            name_start: name_idx,
            close_paren,
            body_open: open_brace,
            body_close,
            was,
            ret_span,
            is_async,
            is_arrow,
            has_return_type,
        });
    }

    anyhow::bail!("function declaration `{clean_name}` not found")
}

/// Rewrites return statements in function body.
fn rewrite_body_returns(
    body: &str,
    lang: Language,
    wrapper: &Wrapper,
    constructor: Option<&str>,
    was: &str,
) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'r' if body[i..].starts_with("return") => {
                let before = if i > 0 { body.as_bytes()[i - 1] as char } else { ' ' };
                let after = if i + 6 < bytes.len() { body.as_bytes()[i + 6] as char } else { ' ' };
                if !is_ident(before) && !is_ident(after) {
                    let end_stmt = body[i..].find([';', '\n']).map_or(body.len(), |e| i + e);
                    let ret_stmt = &body[i..end_stmt];
                    let expr = ret_stmt.strip_prefix("return").unwrap().trim();
                    let semi = if ret_stmt.ends_with(';') { ";" } else { "" };
                    let expr_clean = expr.strip_suffix(';').unwrap_or(expr).trim();
                    match wrapper {
                        Wrapper::Custom(custom_name) => {
                            let base = custom_name.split(['<', '[']).next().unwrap_or(custom_name).trim();
                            let base = base.rsplit("::").next().unwrap_or(base);
                            let base = base.rsplit('.').next().unwrap_or(base).trim();
                            let wrapped = format_constructor_call(constructor, base, expr_clean, lang, was);
                            edits.push((i, end_stmt - i, format!("return {wrapped}{semi}")));
                        }
                        Wrapper::Result => match lang {
                            Language::Go => {
                                if expr_clean.is_empty() {
                                    edits.push((i, end_stmt - i, format!("return nil{semi}")));
                                } else {
                                    edits.push((i, end_stmt - i, format!("return {expr_clean}, nil{semi}")));
                                }
                            }
                            Language::Swift => {
                                if expr_clean.is_empty() {
                                    edits.push((i, end_stmt - i, format!("return .success(()){semi}")));
                                } else {
                                    edits.push((i, end_stmt - i, format!("return .success({expr_clean}){semi}")));
                                }
                            }
                            Language::Python => {
                                if expr_clean.is_empty() {
                                    edits.push((i, end_stmt - i, format!("return Ok(None){semi}")));
                                } else {
                                    edits.push((i, end_stmt - i, format!("return Ok({expr_clean}){semi}")));
                                }
                            }
                            Language::TypeScript | Language::JavaScript => {
                                if expr_clean.is_empty() {
                                    edits.push((i, end_stmt - i, format!("return {{ ok: true, value: undefined }}{semi}")));
                                } else {
                                    edits.push((i, end_stmt - i, format!("return {{ ok: true, value: {expr_clean} }}{semi}")));
                                }
                            }
                            _ => {}
                        },
                        Wrapper::Pointer | Wrapper::Option
                            if lang == Language::Go
                                && !expr_clean.starts_with('&')
                                && !expr_clean.is_empty()
                                && expr_clean != "nil" =>
                        {
                            edits.push((i, end_stmt - i, format!("return &{expr_clean}{semi}")));
                        }
                        _ => {}
                    }
                    i = end_stmt;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }

    let mut out = body.to_string();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (start, len, repl) in edits {
        out.replace_range(start..start + len, &repl);
    }
    out
}

/// Restructures the declaring file: signature and return expressions in the body.
pub fn restructure_declaring_file(
    text: &str,
    lang: Language,
    decl: &PolyglotFuncDecl,
    wrapper: &Wrapper,
    constructor: Option<&str>,
    error: Option<&str>,
) -> Result<(String, String)> {
    let was = &decl.was;
    let mut out = text.to_string();

    let now = match lang {
        Language::TypeScript | Language::JavaScript => {
            let is_ts = lang == Language::TypeScript;
            match wrapper {
                Wrapper::Promise => {
                    let now = if was.is_empty() || was == "void" {
                        if is_ts { "Promise<void>".to_string() } else { "Promise".to_string() }
                    } else {
                        format!("Promise<{was}>")
                    };
                    if is_ts {
                        if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                            out.replace_range(s..e, &format!("Promise<{was}>"));
                        } else {
                            out.insert_str(decl.close_paren + 1, ": Promise<void>");
                        }
                    }
                    if !decl.is_async {
                        let header_part = &out[decl.decl_start..decl.name_start];
                        if let Some(pos) = header_part.rfind("function ") {
                            out.insert_str(decl.decl_start + pos, "async ");
                        } else if decl.is_arrow {
                            let open_p = out[decl.decl_start..].find('(').unwrap();
                            out.insert_str(decl.decl_start + open_p, "async ");
                        } else {
                            // Method
                            out.insert_str(decl.name_start, "async ");
                        }
                    }
                    now
                }
                Wrapper::Option => {
                    let now = if is_ts {
                        format!("{was} | null")
                    } else {
                        "Option".to_string()
                    };
                    if is_ts {
                        if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                            out.replace_range(s..e, &format!("{was} | null"));
                        } else {
                            out.insert_str(decl.close_paren + 1, ": void | null");
                        }
                    }
                    now
                }
                Wrapper::Result => {
                    let err_ty = error.unwrap_or("Error");
                    let now = if is_ts {
                        format!("Result<{was}, {err_ty}>")
                    } else {
                        format!("Result<{err_ty}>")
                    };
                    if is_ts {
                        if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                            out.replace_range(s..e, &now);
                        } else {
                            out.insert_str(decl.close_paren + 1, &format!(": Result<void, {err_ty}>"));
                        }
                    }
                    now
                }
                Wrapper::Custom(custom_name) => {
                    let base = custom_name.split('<').next().unwrap_or(custom_name).trim();
                    let now = if custom_name.contains('<') {
                        custom_name.replace("<T>", &format!("<{was}>")).replace("<>", &format!("<{was}>"))
                    } else if was.is_empty() || was == "void" {
                        if is_ts { format!("{base}<void>") } else { base.to_string() }
                    } else {
                        if is_ts { format!("{base}<{was}>") } else { base.to_string() }
                    };
                    if is_ts {
                        if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                            out.replace_range(s..e, &now);
                        } else {
                            out.insert_str(decl.close_paren + 1, &format!(": {now}"));
                        }
                    }
                    now
                }
                Wrapper::Pointer => anyhow::bail!("Pointer wrapper is not supported for TypeScript/JavaScript"),
            }
        }
        Language::Python => {
            match wrapper {
                Wrapper::Option => {
                    let now = if was.is_empty() || was == "None" {
                        "Optional[Any]".to_string()
                    } else {
                        format!("Optional[{was}]")
                    };
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, &format!(" -> {now}"));
                    }
                    now
                }
                Wrapper::Result => {
                    let err_ty = error.unwrap_or("Exception");
                    let now = if was.is_empty() || was == "None" {
                        format!("Result[Any, {err_ty}]")
                    } else {
                        format!("Result[{was}, {err_ty}]")
                    };
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, &format!(" -> {now}"));
                    }
                    now
                }
                Wrapper::Custom(custom_name) => {
                    let base = custom_name.split('[').next().unwrap_or(custom_name).trim();
                    let now = if custom_name.contains('[') {
                        custom_name.replace("[T]", &format!("[{was}]")).replace("[]", &format!("[{was}]"))
                    } else if was.is_empty() || was == "None" {
                        format!("{base}[Any]")
                    } else {
                        format!("{base}[{was}]")
                    };
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, &format!(" -> {now}"));
                    }
                    now
                }
                Wrapper::Promise | Wrapper::Pointer => anyhow::bail!("{wrapper:?} wrapper is not supported for Python"),
            }
        }
        Language::Cpp | Language::C => {
            match wrapper {
                Wrapper::Option => {
                    let now = format!("std::optional<{was}>");
                    if let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    }
                    now
                }
                Wrapper::Result => {
                    let err_ty = error.unwrap_or("std::string");
                    let now = format!("std::expected<{was}, {err_ty}>");
                    if let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    }
                    now
                }
                Wrapper::Custom(custom_name) => {
                    let base = custom_name.split('<').next().unwrap_or(custom_name).trim();
                    let now = if custom_name.contains('<') {
                        custom_name.replace("<T>", &format!("<{was}>")).replace("<>", &format!("<{was}>"))
                    } else if was.is_empty() || was == "void" {
                        base.to_string()
                    } else {
                        format!("{base}<{was}>")
                    };
                    if let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    }
                    now
                }
                Wrapper::Promise | Wrapper::Pointer => anyhow::bail!("{wrapper:?} wrapper is not supported for C++"),
            }
        }
        Language::Swift => {
            match wrapper {
                Wrapper::Option => {
                    let now = format!("{was}?");
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, &format!(" -> {now} "));
                    }
                    now
                }
                Wrapper::Result => {
                    let err_ty = error.unwrap_or("Error");
                    let now = format!("Result<{was}, {err_ty}>");
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, &format!(" -> {now} "));
                    }
                    now
                }
                Wrapper::Custom(custom_name) => {
                    let base = custom_name.split('<').next().unwrap_or(custom_name).trim();
                    let now = if custom_name.contains('<') {
                        custom_name.replace("<T>", &format!("<{was}>")).replace("<>", &format!("<{was}>"))
                    } else if was.is_empty() || was == "Void" {
                        format!("{base}<Void>")
                    } else {
                        format!("{base}<{was}>")
                    };
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, &format!(" -> {now} "));
                    }
                    now
                }
                Wrapper::Promise | Wrapper::Pointer => anyhow::bail!("{wrapper:?} wrapper is not supported for Swift"),
            }
        }
        Language::Go => {
            match wrapper {
                Wrapper::Result => {
                    let now = if was.is_empty() {
                        "error".to_string()
                    } else if was.starts_with('(') && was.ends_with(')') {
                        let inner = was[1..was.len() - 1].trim();
                        format!("({inner}, error)")
                    } else {
                        format!("({was}, error)")
                    };
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, " error ");
                    }
                    now
                }
                Wrapper::Pointer | Wrapper::Option => {
                    let now = format!("*{was}");
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, &format!(" *{was} "));
                    }
                    now
                }
                Wrapper::Custom(custom_name) => {
                    let base = custom_name.split('[').next().unwrap_or(custom_name).trim();
                    let now = if custom_name.contains('[') {
                        custom_name.replace("[T]", &format!("[{was}]")).replace("[]", &format!("[{was}]"))
                    } else if custom_name.starts_with('*') {
                        custom_name.to_string()
                    } else if was.is_empty() {
                        base.to_string()
                    } else {
                        format!("{base}[{was}]")
                    };
                    if decl.has_return_type && let Some((s, e)) = decl.ret_span {
                        out.replace_range(s..e, &now);
                    } else {
                        out.insert_str(decl.body_open, &format!(" {now} "));
                    }
                    now
                }
                Wrapper::Promise => anyhow::bail!("Promise wrapper is not supported for Go"),
            }
        }
        Language::Rust => unreachable!(),
    };

    // Body rewrite: re-find body open and close in `out`
    let new_body_open = out[decl.name_start..].find(if lang == Language::Python { ':' } else { '{' })
        .map(|i| decl.name_start + i)
        .context("cannot find body open")?;
    let new_body_close = if lang == Language::Python {
        crate::inline_parameter::find_python_body_close(&out, decl.decl_start, new_body_open)
    } else {
        crate::parameter_object::matching_bracket(&out, new_body_open).context("unclosed body")?
    };

    let body_text = &out[new_body_open + 1..new_body_close];
    let rewritten_body = rewrite_body_returns(body_text, lang, wrapper, constructor, was);
    out.replace_range(new_body_open + 1..new_body_close, &rewritten_body);

    Ok((out, now))
}

/// Identifies return type and async status of the innermost enclosing function.
pub fn enclosing_polyglot_info(content: &str, at: usize, lang: Language) -> Option<(String, bool)> {
    if lang == Language::Python {
        let lines: Vec<&str> = content[..at].lines().collect();
        let target_line = lines.last()?;
        let target_indent = target_line.len() - target_line.trim_start().len();
        for line in lines.iter().rev().skip(1) {
            let trimmed = line.trim();
            if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
                let indent = line.len() - line.trim_start().len();
                if indent < target_indent {
                    let is_async = trimmed.starts_with("async def ");
                    let ret = if let Some(arr) = trimmed.find("->") {
                        trimmed[arr + 2..trimmed.rfind(':').unwrap_or(trimmed.len())].trim().to_string()
                    } else {
                        "None".to_string()
                    };
                    return Some((ret, is_async));
                }
            }
        }
        return None;
    }

    // C-like bracket languages
    let mut search = at;
    while let Some(open_rel) = content[..search].rfind('{') {
        search = open_rel;
        let Some(close_b) = crate::parameter_object::matching_bracket(content, open_rel) else {
            continue;
        };
        if open_rel < at && at < close_b {
            let line_start = content[..open_rel].rfind('\n').map_or(0, |p| p + 1);
            let header = &content[line_start..open_rel];
            let is_async = header.contains("async");

            let ret_type = match lang {
                Language::TypeScript | Language::JavaScript => {
                    let end_h = if let Some(arr) = header.find("=>") { arr } else { header.len() };
                    if let Some(colon) = header[..end_h].rfind(':') {
                        header[colon + 1..end_h].trim().to_string()
                    } else {
                        String::new()
                    }
                }
                Language::Swift => {
                    if let Some(arr) = header.find("->") {
                        header[arr + 2..].trim().to_string()
                    } else {
                        "Void".to_string()
                    }
                }
                Language::Go => {
                    if let Some(func_pos) = header.find("func ") {
                        let after_func = &header[func_pos + 5..];
                        let trimmed = after_func.trim_start();
                        let offset = func_pos + 5 + (after_func.len() - trimmed.len());
                        if trimmed.starts_with('(') {
                            // Receiver present: func (r Recv) Name(params) RetType
                            if let Some(recv_close) = crate::parameter_object::matching_bracket(header, offset) {
                                if let Some(param_open_rel) = header[recv_close + 1..].find('(') {
                                    let param_open = recv_close + 1 + param_open_rel;
                                    if let Some(param_close) = crate::parameter_object::matching_bracket(header, param_open) {
                                        header[param_close + 1..].trim().to_string()
                                    } else {
                                        String::new()
                                    }
                                } else {
                                    String::new()
                                }
                            } else {
                                String::new()
                            }
                        } else if let Some(param_open_rel) = trimmed.find('(') {
                            let param_open = offset + param_open_rel;
                            if let Some(param_close) = crate::parameter_object::matching_bracket(header, param_open) {
                                header[param_close + 1..].trim().to_string()
                            } else {
                                String::new()
                            }
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    }
                }
                Language::Cpp | Language::C => {
                    if let Some(paren_open) = header.find('(') {
                        let before_paren = header[..paren_open].trim();
                        let words: Vec<&str> = before_paren.split_whitespace().collect();
                        if words.len() >= 2 {
                            words[..words.len() - 1].join(" ")
                        } else {
                            words.join(" ")
                        }
                    } else {
                        String::new()
                    }
                }
                _ => String::new(),
            };
            return Some((ret_type, is_async));
        }
    }
    None
}

/// Unified wrap_return refactoring across Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.
#[allow(clippy::too_many_arguments)]
pub async fn wrap_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    wrapper: Wrapper,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    wrap_polyglot_ext(
        remote,
        root,
        file,
        symbol,
        line,
        col,
        wrapper,
        None,
        error,
        apply,
        force,
    )
    .await
}

/// Unified wrap_return refactoring with optional custom constructor across Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.
#[allow(clippy::too_many_arguments)]
pub async fn wrap_polyglot_ext(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    wrapper: Wrapper,
    constructor: Option<&str>,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    let lang = Language::of(file).with_context(|| format!("unsupported language for {}", file.display()))?;
    if lang == Language::Rust {
        return wrap_rust_ext(
            remote,
            root,
            file,
            symbol,
            line.unwrap_or(0),
            col.unwrap_or(0),
            wrapper,
            constructor,
            error,
            apply,
            force,
        )
        .await;
    }

    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read {}", file.display()))?;

    let decl = find_polyglot_decl(&text, lang, symbol, line)?;
    let name = decl.name.clone();
    let was = decl.was.clone();

    // Check already wrapped
    match &wrapper {
        Wrapper::Promise => {
            anyhow::ensure!(
                !was.contains("Promise<") && (!decl.is_async || !was.is_empty()),
                "`{name}` already returns a `Promise`"
            );
        }
        Wrapper::Option => {
            anyhow::ensure!(
                !was.contains("Optional[")
                    && !was.contains("std::optional")
                    && !was.contains("optional")
                    && !was.ends_with('?')
                    && !was.contains("| null")
                    && !was.contains("| None")
                    && !was.starts_with('*')
                    && !was.starts_with("Option<"),
                "`{name}` already returns an `Option`"
            );
        }
        Wrapper::Result => {
            anyhow::ensure!(
                !was.contains("Result<")
                    && !was.contains("Result[")
                    && !was.contains("std::expected")
                    && !was.contains("expected")
                    && !was.contains("error")
                    && !was.contains("{ ok:"),
                "`{name}` already returns a `Result`"
            );
        }
        Wrapper::Pointer => {
            anyhow::ensure!(
                !was.starts_with('*'),
                "`{name}` already returns a `Pointer`"
            );
        }
        Wrapper::Custom(custom_name) => {
            let base = custom_name.split(['<', '[']).next().unwrap_or(custom_name).trim();
            let base = base.rsplit("::").next().unwrap_or(base);
            let base = base.rsplit('.').next().unwrap_or(base).trim();
            anyhow::ensure!(
                !was.contains(base),
                "`{name}` already returns a `{base}`"
            );
        }
    }

    let (new_decl_file, now) = restructure_declaring_file(&text, lang, &decl, &wrapper, constructor, error)?;

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), new_decl_file.clone());

    let mut propagated = 0usize;
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();

    let canonical_file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());

    // Traverse workspace files for callers
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if !path.is_file() || !crate::inline_parameter::language_matches(lang, path) {
            continue;
        }
        let Ok(other_content) = std::fs::read_to_string(path) else {
            continue;
        };
        if !other_content.contains(&name) {
            continue;
        }

        let is_decl_file = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()) == canonical_file;
        let rel_path = display(root, path);

        let mut file_edits: Vec<(usize, usize, String)> = Vec::new();

        for (at, _) in other_content.match_indices(&name) {
            if at > 0 {
                let prev = other_content[..at].chars().next_back().unwrap();
                if is_ident(prev) {
                    continue;
                }
            }
            let after = &other_content[at + name.len()..];
            if after.starts_with(is_ident) {
                continue;
            }

            let line_num = other_content[..at].lines().count();
            let col_num = at - other_content[..at].rfind('\n').map_or(0, |p| p + 1) + 1;
            let site = format!("{rel_path}:{line_num}:{col_num}");

            // In declaring file, skip the declaration itself
            if is_decl_file && at >= decl.decl_start && at <= decl.close_paren {
                continue;
            }

            // Self-call inside function's own body
            if is_decl_file && at > decl.body_open && at < decl.body_close {
                unmatched.push(format!("{site} (a call inside `{name}` itself)"));
                continue;
            }

            if crate::inline_parameter::is_in_comment(&other_content, at, lang) {
                continue;
            }
            if crate::inline_parameter::is_import_or_export_context(&other_content, at, lang) {
                continue;
            }

            // C++ prototype in header
            let proto_close_paren = other_content[at + name.len()..]
                .find('(')
                .and_then(|open| crate::parameter_object::matching_bracket(&other_content, at + name.len() + open));
            if matches!(lang, Language::Cpp | Language::C)
                && let Some(cp) = proto_close_paren
                && crate::inline_parameter::is_c_cpp_prototype(&other_content, at, cp)
            {
                if let Some(ret_start) = other_content[..at].rfind(&was) {
                    file_edits.push((ret_start, was.len(), now.clone()));
                }
                continue;
            }

            let Some((_args_start, args_end)) = crate::parameter_object::call_args_span(&other_content, at + name.len()) else {
                unmatched.push(format!("{site} `{name}` used as a value"));
                continue;
            };

            // Call site found!
            let caller_info = enclosing_polyglot_info(&other_content, at, lang);
            let (caller_ret, caller_is_async) = caller_info.unwrap_or_else(|| (String::new(), false));

            match &wrapper {
                Wrapper::Promise => {
                    if caller_is_async {
                        let before_call = other_content[..at].trim_end();
                        if before_call.ends_with("await") {
                            propagated += 1;
                        } else {
                            let after_call = other_content[args_end + 1..].trim_start();
                            if after_call.starts_with('.') {
                                file_edits.push((at, 0, "(await ".to_string()));
                                file_edits.push((args_end + 1, 0, ")".to_string()));
                            } else {
                                file_edits.push((at, 0, "await ".to_string()));
                            }
                            propagated += 1;
                        }
                    } else {
                        let line_text = other_content[other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        blocked.push(format!("{site} the caller is not async and cannot await `{name}`: `{line_text}`"));
                    }
                }
                Wrapper::Option => {
                    let can_propagate = match lang {
                        Language::TypeScript | Language::JavaScript => caller_ret.contains("| null") || caller_ret.contains("Option<"),
                        Language::Python => caller_ret.contains("Optional[") || caller_ret.contains("| None"),
                        Language::Cpp | Language::C => caller_ret.contains("optional"),
                        Language::Swift => caller_ret.ends_with('?') || caller_ret.contains("Optional<"),
                        Language::Go => caller_ret.starts_with('*'),
                        Language::Rust => unreachable!(),
                    };
                    if can_propagate {
                        propagated += 1;
                    } else {
                        let line_text = other_content[other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let desc = if caller_ret.is_empty() { "none" } else { &caller_ret };
                        blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
                    }
                }
                Wrapper::Result => {
                    let can_propagate = match lang {
                        Language::TypeScript | Language::JavaScript => caller_ret.contains("Result<"),
                        Language::Python => caller_ret.contains("Result["),
                        Language::Cpp | Language::C => caller_ret.contains("expected") || caller_ret.contains("Result"),
                        Language::Swift => caller_ret.contains("Result<"),
                        Language::Go => caller_ret.contains("error"),
                        Language::Rust => unreachable!(),
                    };
                    if can_propagate {
                        propagated += 1;
                    } else {
                        let line_text = other_content[other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let desc = if caller_ret.is_empty() { "none" } else { &caller_ret };
                        blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
                    }
                }
                Wrapper::Pointer => {
                    let can_propagate = match lang {
                        Language::Go => caller_ret.starts_with('*'),
                        _ => caller_ret.contains('*'),
                    };
                    if can_propagate {
                        propagated += 1;
                    } else {
                        let line_text = other_content[other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let desc = if caller_ret.is_empty() { "none" } else { &caller_ret };
                        blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
                    }
                }
                Wrapper::Custom(custom_name) => {
                    let base = custom_name.split(['<', '[']).next().unwrap_or(custom_name).trim();
                    let base = base.rsplit("::").next().unwrap_or(base);
                    let base = base.rsplit('.').next().unwrap_or(base).trim();
                    let can_propagate = caller_ret.contains(base);
                    if can_propagate {
                        propagated += 1;
                    } else {
                        let line_text = other_content[other_content[..at].rfind('\n').map_or(0, |i| i + 1)..]
                            .lines()
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let desc = if caller_ret.is_empty() { "none" } else { &caller_ret };
                        blocked.push(format!("{site} the caller returns `{desc}`: `{line_text}`"));
                    }
                }
            }
        }

        if !file_edits.is_empty() {
            let mut body = if is_decl_file {
                new_decl_file.clone()
            } else {
                other_content
            };
            file_edits.sort_by_key(|(at, len, _)| (*at, *len != 0));
            for (at, len, replacement) in file_edits.into_iter().rev() {
                body.replace_range(at..at + len, &replacement);
            }
            rewritten.insert(path.to_path_buf(), body);
        }
    }

    rewritten.retain(|p, t| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true));

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
            "{} call site(s) cannot propagate; nothing was written:\n  {}",
            blocked.len(),
            blocked.join("\n  ")
        );
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not rewritten; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
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

/// Backward compatibility wrapper.
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
    wrap_polyglot_ext(
        remote,
        root,
        file,
        None,
        Some(line),
        Some(col),
        wrapper,
        None,
        error,
        apply,
        force,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapper_is_named_either_way_and_nothing_else() {
        assert_eq!(Wrapper::parse("option").unwrap(), Wrapper::Option);
        assert_eq!(Wrapper::parse("Result").unwrap(), Wrapper::Result);
        assert_eq!(Wrapper::parse("promise").unwrap(), Wrapper::Promise);
        assert_eq!(Wrapper::parse("pointer").unwrap(), Wrapper::Pointer);
        assert_eq!(Wrapper::parse("Response").unwrap(), Wrapper::Custom("Response".into()));
        assert!(Wrapper::parse("").is_err());
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
        assert!(propagates("Option<u32>", &Wrapper::Option));
        assert!(propagates("std::option::Option<u32>", &Wrapper::Option));
        assert!(propagates("anyhow::Result<()>", &Wrapper::Result));
        assert!(propagates("Result<u32, String>", &Wrapper::Result));
        assert!(propagates("Response<u32>", &Wrapper::Custom("Response".into())));
        assert!(propagates("my_mod::Response<u32>", &Wrapper::Custom("Response".into())));
        assert!(!propagates("Result<u32, String>", &Wrapper::Option));
        assert!(!propagates("u32", &Wrapper::Result));
        assert!(!propagates("u32", &Wrapper::Custom("Response".into())));
        assert!(!propagates("()", &Wrapper::Option));
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
