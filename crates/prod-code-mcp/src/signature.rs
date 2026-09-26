//! Change a function's parameter list, with every call site (roadmap 7.1.1).
//!
//! `code_rename` changes what a function is called; nothing changed what it *takes*.
//! rust-analyzer has no change-signature assist (at a function declaration it offers
//! `inline_into_callers` and two type-alias generators), so an agent had to edit the
//! declaration by hand and then rewrite every argument list by hand — or hand-write an SSR
//! rule, inventing a placeholder per argument and getting the arity right from memory.
//!
//! Here the arity and the types come from the declaration, the rule is built from them, and
//! the rule is resolved in the declaring file's own scope so that call sites match however
//! they are spelled. Three things then make the answer honest rather than merely plausible:
//!
//! - the rewrite is **reconciled** against the analyzer's reference list, so a call site that
//!   was not rewritten is named instead of silently missing from a count;
//! - a parameter that the body still uses is not dropped without saying where it is used;
//! - the whole change — declaration and call sites together — is type-checked in an overlay
//!   before anything is written, which is what catches a reorder of two different types;
//! - a change that type-checks can still change what the program does: arguments are evaluated
//!   left to right and parameters are dropped in reverse order of declaration, so a reorder of
//!   `f(mark(a), mark(b))` or of two owned values, or a removed parameter whose argument does
//!   something, is refused with the call site, `force` or not (#442). What looks inert is not
//!   taken on its spelling: a field read `a.n` can call a user `Deref`, an argument for a
//!   reference parameter can be converted by one, and `u32` or `Option<u32>` can name a type of
//!   the crate's own with a `Drop`. The analyzer is asked what each parameter's type names, and
//!   what it does not confirm counts as able to run code.
//!
//! A reference list that cannot be had is an error, never an empty list: an empty one reads as
//! "no callers" and "nothing to reconcile".

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// One entry of the requested parameter list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Param {
    /// Keep the parameter declared under this name, in this position.
    Keep(String),
    /// Add a parameter, passing `value` at every call site.
    Add {
        name: String,
        ty: String,
        value: String,
    },
}

/// A reference to the symbol: file, line, column, all 1-based.
type Reference = (PathBuf, u32, u32);

/// A parameter as the declaration writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declared {
    /// The whole parameter, `name: Type` with any `mut` or pattern kept verbatim.
    pub(crate) raw: String,
    /// What the parameter is called, for matching against the request.
    pub(crate) name: String,
}

/// What a change did, or would do.
#[derive(Debug)]
pub struct SignatureChange {
    pub symbol: String,
    /// The workspace root, so that the report can show relative paths.
    pub root: PathBuf,
    /// The declaring file, relative to the workspace root.
    pub file: String,
    pub old_signature: String,
    pub new_signature: String,
    /// The structural rule the call-site rewrite ran, empty when the order did not change.
    pub rule: String,
    /// Every file this changes, as (relative path, whole new content).
    pub rewritten: Vec<(String, String)>,
    /// References the analyzer knows about that the rewrite did not touch.
    pub unmatched: Vec<String>,
    /// Lines the rewrite changed that are not references — an over-match, or a call the
    /// analyzer did not list.
    pub unexpected: Vec<String>,
    /// Errors the analyzer reports for the changed files, checked together.
    pub diagnostics: Vec<String>,
    pub applied: bool,
    /// The return type before and after, when the request changed it (`()` for none).
    pub returns: Option<(String, String)>,
    /// The visibility before and after, when the request changed it (`private` for none).
    pub visibility: Option<(String, String)>,
    /// Whether it was `async` and is now, when the request changed it.
    pub asyncness: Option<(bool, bool)>,
    /// Calls that now `.await` from a function that is not `async`: each blocks the write
    /// unless `force`.
    pub not_async: Vec<String>,
}

/// What a signature change does besides the parameters: the return type and the visibility.
#[derive(Debug, Clone, Default)]
pub struct Modifiers {
    /// The return type the function should have; `()` removes it.
    pub returns: Option<String>,
    /// `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`, or `private` to remove it.
    pub visibility: Option<String>,
    /// Whether the function should be `async`; every call gains or loses its `.await`.
    pub asyncness: Option<bool>,
}

impl SignatureChange {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: ({})\n- now: ({})\n",
            self.symbol, self.file, self.old_signature, self.new_signature
        );
        if let Some((was, now)) = &self.returns {
            out.push_str(&format!("- returns: `{was}` → `{now}`\n"));
        }
        if let Some((was, now)) = &self.visibility {
            out.push_str(&format!("- visibility: `{was}` → `{now}`\n"));
        }
        if let Some((was, now)) = self.asyncness {
            let word = |a: bool| if a { "async" } else { "not async" };
            out.push_str(&format!(
                "- {} → {}: every call {} `.await`\n",
                word(was),
                word(now),
                if now { "gains" } else { "loses" }
            ));
        }
        for site in &self.not_async {
            out.push_str(&format!(
                "- {site}: awaits from a function that is not `async`\n"
            ));
        }
        if !self.rule.is_empty() {
            out.push_str(&format!("- call sites: `{}`\n", self.rule));
        }
        out.push('\n');
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
                "\nnot rewritten ({} reference(s) the rule did not match — a call through a \
                 function pointer, a macro, or a spelling structural search cannot see):\n",
                self.unmatched.len()
            ));
            for r in &self.unmatched {
                out.push_str(&format!("  {r}\n"));
            }
        }
        if !self.unexpected.is_empty() {
            out.push_str(&format!(
                "\nchanged without being a known reference ({}), check these by hand:\n",
                self.unexpected.len()
            ));
            for r in &self.unexpected {
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

/// Parses one entry of a requested parameter list.
///
/// `name` keeps the parameter declared under that name, in this position; `name: Type = expr`
/// adds a parameter and passes `expr` at every call site. A declared parameter that the
/// request does not list is removed.
pub fn parse_param(spec: &str) -> Result<Param> {
    let spec = spec.trim();
    anyhow::ensure!(!spec.is_empty(), "empty parameter");
    match split_at_top_level(spec, ':') {
        None => {
            anyhow::ensure!(
                is_ident(spec),
                "`{spec}` is neither a parameter name nor a new parameter; a new one is written \
                 `name: Type = expression`"
            );
            Ok(Param::Keep(spec.to_string()))
        }
        Some((name, rest)) => {
            let name = name.trim();
            anyhow::ensure!(is_ident(name), "`{name}` is not a parameter name");
            let (ty, value) = split_at_top_level(rest, '=').with_context(|| {
                format!(
                    "a new parameter needs the expression to pass at every call site: \
                     `{name}: Type = expression`"
                )
            })?;
            let (ty, value) = (ty.trim(), value.trim());
            anyhow::ensure!(!ty.is_empty(), "`{name}` has no type");
            anyhow::ensure!(!value.is_empty(), "`{name}` has no call-site expression");
            Ok(Param::Add {
                name: name.to_string(),
                ty: ty.to_string(),
                value: value.to_string(),
            })
        }
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// Splits on the first occurrence of `sep` that is not nested and not part of a two-character
/// operator (`::`, `->`, `=>`, `==`, `<=`, `>=`, `!=`).
fn split_at_top_level(text: &str, sep: char) -> Option<(&str, &str)> {
    let bytes: Vec<char> = text.chars().collect();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        let prev = if i > 0 { bytes[i - 1] } else { ' ' };
        let next = bytes.get(i + 1).copied().unwrap_or(' ');
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            // `<` and `>` nest types, but `->`, `=>` and the comparisons use the same
            // characters and must not move the depth.
            '<' if prev != '-' && prev != '=' && next != '=' => depth += 1,
            '>' if prev != '-' && prev != '=' && next != '=' => depth -= 1,
            _ => {}
        }
        // `::` is not a parameter's colon and `==`, `=>`, `>=`, `<=`, `!=` are not the `=` of a
        // default; neither is the `=` of an attribute inside a type.
        let operator = prev == sep
            || next == sep
            || (sep == '=' && (next == '>' || prev == '<' || prev == '>' || prev == '!'));
        if depth == 0 && c == sep && !operator {
            let at = text
                .char_indices()
                .nth(i)
                .map(|(byte, _)| byte)
                .unwrap_or(text.len());
            return Some((&text[..at], &text[at + c.len_utf8()..]));
        }
        i += 1;
    }
    None
}

/// Byte offset of a 1-based line and column.
pub(crate) fn offset_of(text: &str, line: u32, col: u32) -> Option<usize> {
    let mut offset = 0usize;
    for (n, l) in text.lines().enumerate() {
        if n as u32 + 1 == line {
            let within: usize = l
                .chars()
                .take(col.saturating_sub(1) as usize)
                .map(char::len_utf8)
                .sum();
            return Some(offset + within);
        }
        offset += l.len() + 1;
    }
    None
}

/// The span between the parentheses of the parameter list of the function whose name starts at
/// `name_offset`, and the name itself.
pub(crate) fn param_span(text: &str, name_offset: usize) -> Option<(String, usize, usize)> {
    let rest = text.get(name_offset..)?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    // Generic parameters come between the name and the parameter list and nest.
    let mut i = name_offset + name.len();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut idx = chars.iter().position(|(b, _)| *b >= i)?;
    let mut angle = 0i32;
    loop {
        let (b, c) = *chars.get(idx)?;
        match c {
            '<' => angle += 1,
            '>' => angle -= 1,
            '(' if angle == 0 => {
                i = b;
                break;
            }
            _ if angle == 0
                && !c.is_whitespace()
                && c != '\''
                && !c.is_alphanumeric()
                && c != '_'
                && c != ','
                && c != ':'
                && c != '&'
                && c != '+'
                && c != '?'
                && c != '.' =>
            {
                return None;
            }
            _ => {}
        }
        idx += 1;
    }
    let open = i;
    let mut depth = 0i32;
    for (b, c) in text[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((name, open + 1, open + b));
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits a parameter list, respecting nesting; comments and attributes stay attached to the
/// parameter they precede.
pub(crate) fn split_params(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    let chars: Vec<char> = list.chars().collect();
    for (i, c) in chars.iter().copied().enumerate() {
        let prev = if i > 0 { chars[i - 1] } else { ' ' };
        let next = chars.get(i + 1).copied().unwrap_or(' ');
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' if prev != '-' && prev != '=' && next != '=' => depth += 1,
            '>' if prev != '-' && prev != '=' && next != '=' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// A parameter of a function declaration at the 1-based `line`:`col` of `text`: the offset of
/// the function's name, the parameter's name, and the names of the parameters that stay, in
/// order. `None` when the position is not on a parameter's name.
pub fn parameter_at(text: &str, line: u32, col: u32) -> Option<(usize, String, Vec<String>)> {
    let at = offset_of(text, line, col)?;
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let name: String = text[start..].chars().take_while(|c| is_ident(*c)).collect();
    if name.is_empty() || name == "self" {
        return None;
    }
    // The `(` that opens the list this name is in.
    let mut depth = 0i32;
    let open = text[..start].char_indices().rev().find_map(|(i, c)| {
        match c {
            ')' | ']' | '}' => depth += 1,
            '(' if depth == 0 => return Some(i),
            '(' | '[' | '{' => depth -= 1,
            _ => {}
        }
        None
    })?;
    // `fn name<…>(`: the name before the generics, and `fn` before the name.
    let mut head = text[..open].trim_end();
    if head.ends_with('>') {
        let mut angle = 0i32;
        let cut = head.char_indices().rev().find_map(|(i, c)| {
            match c {
                '>' => angle += 1,
                '<' => {
                    angle -= 1;
                    if angle == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            None
        })?;
        head = head[..cut].trim_end();
    }
    let fn_name_start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map(|(i, _)| i)?;
    if !head[..fn_name_start].trim_end().ends_with("fn") {
        return None;
    }
    let (_, list_open, list_close) = param_span(text, fn_name_start)?;
    if list_open != open + 1 || at > list_close {
        return None;
    }
    let (_, declared) = parse_declared(&text[list_open..list_close]);
    declared.iter().find(|d| d.name == name)?;
    let kept = declared
        .iter()
        .filter(|d| d.name != name)
        .map(|d| d.name.clone())
        .collect();
    Some((fn_name_start, name, kept))
}

/// The receiver (`&self` and friends, kept verbatim) and the parameters of a parameter list.
pub(crate) fn parse_declared(list: &str) -> (Option<String>, Vec<Declared>) {
    let mut receiver = None;
    let mut params = Vec::new();
    for raw in split_params(list) {
        let head = raw.trim_start_matches(['&', ' ']).trim_start();
        let head = head.strip_prefix("mut ").unwrap_or(head).trim_start();
        let is_receiver = head == "self"
            || head.starts_with("self:")
            || head.starts_with("self ")
            || head.starts_with('\'') && head.contains("self");
        if is_receiver && receiver.is_none() && params.is_empty() {
            receiver = Some(raw);
            continue;
        }
        let name = match split_at_top_level(&raw, ':') {
            Some((name, _)) => name.trim().trim_start_matches("mut ").trim().to_string(),
            None => raw.trim().to_string(),
        };
        params.push(Declared { raw, name });
    }
    (receiver, params)
}

/// What a request does to a declaration: the new parameter list as it will be written, one
/// entry per new argument (`Some(i)` is the call site's own argument number `i`, `None` is an
/// expression the request adds), and the parameters that are being removed.
#[derive(Debug)]
struct Plan {
    list: Vec<String>,
    args: Vec<Option<usize>>,
    dropped: Vec<String>,
}

/// Works out the new parameter list, the argument order at the call sites, and what is dropped.
fn plan(declared: &[Declared], request: &[Param]) -> Result<Plan> {
    let mut list = Vec::new();
    let mut args = Vec::new();
    let mut kept = Vec::new();
    for want in request {
        match want {
            Param::Keep(name) => {
                let at = declared
                    .iter()
                    .position(|d| &d.name == name)
                    .with_context(|| {
                        format!(
                            "no parameter named `{name}`; the declaration takes {}",
                            declared
                                .iter()
                                .map(|d| d.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })?;
                anyhow::ensure!(!kept.contains(&at), "`{name}` is listed twice");
                kept.push(at);
                list.push(declared[at].raw.clone());
                args.push(Some(at));
            }
            Param::Add { name, ty, .. } => {
                list.push(format!("{name}: {ty}"));
                args.push(None);
            }
        }
    }
    let dropped = declared
        .iter()
        .enumerate()
        .filter(|(i, _)| !kept.contains(i))
        .map(|(_, d)| d.name.clone())
        .collect();
    Ok(Plan {
        list,
        args,
        dropped,
    })
}

/// Formats the new parameter list the way the old one was written: on one line, or one
/// parameter per line with the original indentation.
fn format_list(old_inner: &str, receiver: Option<&str>, params: &[String]) -> String {
    let mut all: Vec<String> = Vec::new();
    if let Some(r) = receiver {
        all.push(r.trim().to_string());
    }
    all.extend(params.iter().cloned());
    if !old_inner.contains('\n') {
        return all.join(", ");
    }
    let indent = old_inner
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_else(|| "    ".to_string());
    let closing: String = indent.chars().skip(4).collect();
    let mut out = String::from("\n");
    for p in &all {
        out.push_str(&indent);
        out.push_str(p.trim());
        out.push_str(",\n");
    }
    out.push_str(&closing);
    out
}

/// The structural rule that rewrites the call sites: a placeholder per declared argument, and
/// the replacement in the requested order with the added expressions spelled out.
fn call_site_rule(
    name: &str,
    is_method: bool,
    arity: usize,
    args: &[Option<usize>],
    adds: &[&str],
) -> String {
    let pattern_args: Vec<String> = (0..arity).map(|i| format!("${{a{i}}}")).collect();
    let mut added = adds.iter();
    let replacement_args: Vec<String> = args
        .iter()
        .map(|a| match a {
            Some(i) => format!("${{a{i}}}"),
            None => added
                .next()
                .copied()
                .unwrap_or("Default::default()")
                .to_string(),
        })
        .collect();
    // rust-analyzer's placeholders are `$name`, without braces; the braces above only keep the
    // numbering readable while the lists are built.
    let clean = |v: Vec<String>| {
        v.into_iter()
            .map(|s| s.replace("${", "$").replace('}', ""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (pattern, replacement) = (clean(pattern_args), clean(replacement_args));
    if is_method {
        format!("$recv.{name}({pattern}) ==>> $recv.{name}({replacement})")
    } else {
        format!("{name}({pattern}) ==>> {name}({replacement})")
    }
}

/// The lines a call occupies, from the callee's name at `line`:`col` to the closing
/// parenthesis of its argument list.
fn call_span_lines(text: &str, line: u32, col: u32) -> Option<(u32, u32)> {
    let offset = offset_of(text, line, col)?;
    let (_, _, close) = param_span(text, offset)?;
    let (end, _) = line_col_at(text, close);
    Some((line, end.max(line)))
}

/// Groups changed line numbers into hunks, so that a call written over several lines counts as
/// one place rather than as three surprises.
fn hunks(mut lines: Vec<u32>) -> Vec<(u32, u32)> {
    lines.sort_unstable();
    lines.dedup();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for l in lines {
        match out.last_mut() {
            Some((_, end)) if l <= *end + 2 => *end = l,
            _ => out.push((l, l)),
        }
    }
    out
}

/// The lines of `old` that `new` does not keep.
fn changed_lines(old: &str, new: &str) -> Vec<u32> {
    let diff = similar::TextDiff::from_lines(old, new);
    diff.iter_all_changes()
        .filter(|c| c.tag() == similar::ChangeTag::Delete)
        .filter_map(|c| c.old_index().map(|i| i as u32 + 1))
        .collect()
}

/// Changes the parameter list of the function at `file:line:col`.
#[allow(clippy::too_many_arguments)]
pub async fn change(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    request: &[Param],
    apply: bool,
    force: bool,
) -> Result<SignatureChange> {
    change_with(
        remote,
        root,
        file,
        line,
        col,
        request,
        &Modifiers::default(),
        apply,
        force,
    )
    .await
}

/// The declared return type of the function whose parameter list closes at `close` (`()` when it
/// declares none), and `header` with it replaced by `returns`.
fn with_return_type(header: &str, close: usize, returns: &str) -> (String, String) {
    let returns = returns.trim();
    match crate::wrap_return::declared_return(header, close) {
        Some((start, end)) => {
            let was = header[start..end].to_string();
            let mut out = header.to_string();
            if returns == "()" || returns.is_empty() {
                // ` -> T` goes, from the arrow on.
                let arrow = header[..start].rfind("->").unwrap_or(start);
                let from = header[..arrow].trim_end().len();
                out.replace_range(from..end, "");
            } else {
                out.replace_range(start..end, returns);
            }
            (was, out)
        }
        None => {
            let mut out = header.to_string();
            if returns != "()" && !returns.is_empty() {
                out.insert_str(close + 1, &format!(" -> {returns}"));
            }
            ("()".to_string(), out)
        }
    }
}

/// The visibility of the function whose name starts at `name_at`, and `text` with it replaced by
/// `visibility` (`private` removes it).
fn with_visibility(text: &str, name_at: usize, visibility: &str) -> Option<(String, String)> {
    let fn_kw = text[..name_at].trim_end().strip_suffix("fn")?.len();
    let line_start = text[..fn_kw].rfind('\n').map_or(0, |i| i + 1);
    let indent_end =
        line_start + (text[line_start..].len() - text[line_start..].trim_start().len());
    let head = &text[indent_end..fn_kw];
    let (was, rest_at) = if let Some(rest) = head.strip_prefix("pub(") {
        let close = rest.find(')')?;
        (head[..4 + close + 1].to_string(), 4 + close + 1)
    } else if head.starts_with("pub ") {
        ("pub".to_string(), 3)
    } else {
        ("private".to_string(), 0)
    };
    let rest = head[rest_at..].trim_start();
    let visibility = visibility.trim();
    let new_head = if visibility == "private" || visibility.is_empty() {
        rest.to_string()
    } else {
        format!("{visibility} {rest}")
    };
    let mut out = text.to_string();
    out.replace_range(indent_end..fn_kw, &new_head);
    Some((was, out))
}

/// `text` with `async` put in front of the `fn` at `fn_at` (before `unsafe`, which comes after
/// it), or taken away.
pub(crate) fn with_async(text: &str, fn_at: usize, want: bool) -> String {
    let line_start = text[..fn_at].rfind('\n').map_or(0, |i| i + 1);
    let head = &text[line_start..fn_at];
    let mut out = text.to_string();
    if want {
        let at = match head.rfind("unsafe ") {
            Some(i) if head[i + "unsafe ".len()..].trim().is_empty() => line_start + i,
            _ => fn_at,
        };
        out.insert_str(at, "async ");
    } else if let Some(i) = head.rfind("async ") {
        out.replace_range(line_start + i..line_start + i + "async ".len(), "");
    }
    out
}

/// Whether the code at `at` is in the body of an `async fn`.
pub(crate) fn in_async_fn(text: &str, at: usize) -> bool {
    let Some((open, _)) = crate::introduce_variable::enclosing_body(text, at) else {
        return false;
    };
    let Some(fn_at) = text[..open].rfind("fn ") else {
        return false;
    };
    let start = text[..fn_at]
        .rfind(['\n', ';', '}', '{'])
        .map_or(0, |i| i + 1);
    text[start..fn_at].split_whitespace().any(|w| w == "async")
}

/// [`change`], with the return type and the visibility changed in the same edit.
#[allow(clippy::too_many_arguments)]
pub async fn change_with(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    request: &[Param],
    modifiers: &Modifiers,
    apply: bool,
    force: bool,
) -> Result<SignatureChange> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset =
        offset_of(&text, line, col).context("the declaration is not at the resolved position")?;
    let (name, open, close) = param_span(&text, offset)
        .with_context(|| format!("no function declaration at {}:{line}:{col}", file.display()))?;
    let old_inner = text[open..close].to_string();
    let (receiver, declared) = parse_declared(&old_inner);
    let Plan {
        list: new_params,
        args,
        dropped,
    } = plan(&declared, request)?;

    // A parameter the body still uses cannot just disappear. The analyzer knows where a local
    // is used; the caller gets the list instead of a file that no longer compiles.
    if !dropped.is_empty() && !force {
        let mut used = Vec::new();
        for gone in &dropped {
            let d = declared
                .iter()
                .find(|d| &d.name == gone)
                .with_context(|| format!("`{gone}` is not a declared parameter"))?;
            anyhow::ensure!(
                is_ident(gone),
                "`{gone}` is a pattern, and whether the body still uses what it binds cannot be \
                 asked; nothing was written. Pass `force: true` to remove it anyway"
            );
            // The name itself, not the start of `mut b: T` or of an attribute before it.
            let head = split_at_top_level(&d.raw, ':').map_or(d.raw.as_str(), |(h, _)| h);
            let at = text[open..close]
                .find(&d.raw)
                .and_then(|i| head.rfind(gone.as_str()).map(|n| open + i + n))
                .with_context(|| {
                    format!("`{gone}` is not where the parameter list says; nothing was written")
                })?;
            let (l, c) = line_col_at(&text, at);
            let refs = references(remote, root, file, l, c)
                .await
                .with_context(|| {
                    format!("cannot ask whether the body still uses `{gone}`; nothing was written")
                })?;
            let inside: Vec<String> = refs
                .into_iter()
                .filter(|(p, rl, _)| p == file && *rl != l)
                .map(|(p, rl, rc)| format!("{}:{rl}:{rc}", display(root, &p)))
                .collect();
            if !inside.is_empty() {
                used.push(format!(
                    "`{gone}` is used {} time(s): {}",
                    inside.len(),
                    inside.join(", ")
                ));
            }
        }
        if !used.is_empty() {
            anyhow::bail!(
                "these parameters are still used by the body:\n  {}\npass `force: true` to \
                 remove them anyway and fix the body afterwards",
                used.join("\n  ")
            );
        }
    }

    let adds: Vec<&str> = request
        .iter()
        .filter_map(|p| match p {
            Param::Add { value, .. } => Some(value.as_str()),
            Param::Keep(_) => None,
        })
        .collect();
    let order_changed =
        args.len() != declared.len() || args.iter().enumerate().any(|(i, a)| *a != Some(i));
    let head_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let was_async = text[head_start..offset]
        .split_whitespace()
        .any(|w| w == "async");
    let async_wanted = modifiers.asyncness.filter(|w| *w != was_async);
    anyhow::ensure!(
        async_wanted.is_none() || !order_changed,
        "change `async` and the parameter list in two steps"
    );

    // Every reference to the function, asked once and before anything is rewritten: the
    // effects check, the `.await`s and the reconciliation all stand on it. A question that
    // fails stops the change, forced or not — an empty list would read as "no callers".
    let refs = references(remote, root, file, line, col)
        .await
        .with_context(|| format!("cannot list the references to `{name}`; nothing was written"))?;

    // What the call sites will do, not only how they are written. `force` writes a change that
    // does not compile, which the author then sees; it does not write one that compiles and
    // quietly runs differently.
    if order_changed {
        let calls = call_sites(root, &refs)?;
        let facts = param_facts(remote, root, file, &text, open, close, &declared).await;
        let hazards = effect_hazards(&name, &declared, &facts, receiver.is_some(), &args, &calls);
        anyhow::ensure!(
            hazards.is_empty(),
            "the new parameter list would change what the program does, or it cannot be shown \
             that it does not; nothing was written, and `force` does not override this:\n  {}\n\
             bind such an argument to a local of the parameter's own type before the call \
             (`let v: T = …;`) and pass the local, keep owned parameters and references in their \
             order, or remove them in a step of their own",
            hazards.join("\n  ")
        );
    }

    // Call sites first, while the declaration still has the arity the rule matches.
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut rule = String::new();
    if order_changed {
        rule = call_site_rule(&name, receiver.is_some(), declared.len(), &args, &adds);
        let edit = structural_replace(remote, root, file, &rule).await?;
        for (path, new_text) in crate::tools::rewritten_files(&edit) {
            rewritten.insert(PathBuf::from(path), new_text);
        }
    }

    // `async` in or out: every call the analyzer knows gains or loses its `.await`.
    let mut asyncness_change = None;
    let mut not_async = Vec::new();
    if let Some(want) = async_wanted {
        asyncness_change = Some((was_async, want));
        let mut edits: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (path, rl, rc) in &refs {
            let t = match rewritten.get(path) {
                Some(t) => t.clone(),
                None => read_caller(path)?,
            };
            let Some(end) = offset_of(&t, *rl, *rc)
                .and_then(|at| param_span(&t, at))
                .map(|(_, _, close)| close + 1)
            else {
                continue; // not a call: the function used as a value
            };
            if want != t[end..].starts_with(".await") {
                edits.entry(path.clone()).or_default().push(end);
                if want && !in_async_fn(&t, end) {
                    not_async.push(format!("{}:{rl}:{rc}", display(root, path)));
                }
            }
        }
        for (path, mut ends) in edits {
            let mut t = match rewritten.get(&path) {
                Some(t) => t.clone(),
                None => read_caller(&path)?,
            };
            ends.sort_unstable();
            for end in ends.into_iter().rev() {
                if want {
                    t.insert_str(end, ".await");
                } else {
                    t.replace_range(end..end + ".await".len(), "");
                }
            }
            rewritten.insert(path, t);
        }
    }

    // Then the declaration, on top of whatever the rewrite did to its file (a recursive
    // function calls itself, and the call site is inside the body being edited).
    let base = rewritten.get(file).cloned().unwrap_or_else(|| text.clone());
    let (open, close) = if base == text {
        (open, close)
    } else {
        // Not at its old line and column: a call site above it may have come back from the
        // rewrite on fewer lines than it went in with (#58). The declaration itself is not a
        // call and the rewrite never touches it, so its own text is still there to find.
        locate_declaration(&base, &name, &old_inner).with_context(|| {
            format!(
                "`{name}`'s call sites were rewritten, but its declaration `fn {name}({})` is no \
                 longer in {} exactly once — a rewrite in the same file changed it or duplicated \
                 it. Nothing was written.",
                normalize(&old_inner),
                display(root, file)
            )
        })?
    };
    let new_inner = format_list(&old_inner, receiver.as_deref(), &new_params);
    let mut decl_text = String::with_capacity(base.len());
    decl_text.push_str(&base[..open]);
    decl_text.push_str(&new_inner);
    decl_text.push_str(&base[close..]);
    // The return type after the new list, then the visibility in front of `fn`: both in the
    // declaration's own text, in the same edit.
    let new_close = open + new_inner.len();
    let mut returns_change = None;
    if let Some(returns) = &modifiers.returns {
        let (was, out) = with_return_type(&decl_text, new_close, returns);
        if was.trim() != returns.trim() {
            returns_change = Some((was, returns.trim().to_string()));
            decl_text = out;
        }
    }
    let mut visibility_change = None;
    if let Some(visibility) = &modifiers.visibility {
        let name_at = decl_text[..open]
            .rfind(&format!("fn {name}"))
            .map(|i| i + 3)
            .context("the declaration's `fn` keyword is not where the parameter list says")?;
        let (was, out) = with_visibility(&decl_text, name_at, visibility)
            .context("the declaration's visibility could not be read")?;
        if was != visibility.trim() {
            visibility_change = Some((was, visibility.trim().to_string()));
            decl_text = out;
        }
    }
    if let Some((_, want)) = asyncness_change {
        let fn_at = decl_text[..open]
            .rfind(&format!("fn {name}"))
            .context("the declaration's `fn` keyword is not where the parameter list says")?;
        decl_text = with_async(&decl_text, fn_at, want);
    }
    rewritten.insert(file.to_path_buf(), decl_text);

    // Reconcile: what the analyzer knows is a reference against what the rewrite touched.
    // Line proximity is not good enough — a reference on the line above a rewritten one looks
    // rewritten and is not — so each reference is matched against the lines of its own call,
    // from the callee's name to the closing parenthesis.
    let mut unmatched = Vec::new();
    let mut unexpected = Vec::new();
    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut touched: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
    for (path, new_text) in &rewritten {
        let old = read_caller(path)?;
        touched.insert(path.clone(), changed_lines(&old, new_text));
        originals.insert(path.clone(), old);
    }
    // Lines a reference explains, so that what is left over can be reported as a surprise.
    let mut explained: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
    for (path, rl, rc) in &refs {
        let span = originals
            .get(path)
            .and_then(|text| call_span_lines(text, *rl, *rc))
            .unwrap_or((*rl, *rl));
        let hit = touched
            .get(path)
            .is_some_and(|lines| lines.iter().any(|l| *l >= span.0 && *l <= span.1));
        if hit {
            explained
                .entry(path.clone())
                .or_default()
                .extend(span.0..=span.1);
        } else {
            unmatched.push(format!("{}:{rl}:{rc}", display(root, path)));
        }
    }
    // The declaration's own signature explains the lines the new parameter list replaces.
    let (decl_start, _) = line_col_at(&text, open);
    let (decl_end, _) = line_col_at(&text, close);
    explained
        .entry(file.to_path_buf())
        .or_default()
        .extend(decl_start..=decl_end);
    for (path, lines) in &touched {
        let known = explained.get(path).cloned().unwrap_or_default();
        let left: Vec<u32> = lines
            .iter()
            .copied()
            .filter(|l| !known.contains(l))
            .collect();
        for (start, end) in hunks(left) {
            unexpected.push(if start == end {
                format!("{}:{start}", display(root, path))
            } else {
                format!("{}:{start}-{end}", display(root, path))
            });
        }
    }

    // The whole change judged together: the declaration and the call sites in one overlay.
    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    // A caller the rewrite did not touch still has to fit a new return type or visibility.
    let callers: Vec<PathBuf> = {
        let mut files: Vec<PathBuf> = refs
            .iter()
            .map(|(p, _, _)| p.clone())
            .filter(|p| !rewritten.contains_key(p))
            .collect();
        files.sort();
        files.dedup();
        files
    };
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &callers).await?;
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
            "the change does not compile ({} error(s)); nothing was written. Fix the request, \
             or pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        anyhow::ensure!(
            not_async.is_empty() || force,
            "{} call(s) would `.await` from a function that is not `async`; nothing was \
             written. Make those callers `async` first, or pass `force: true`:\n  {}",
            not_async.len(),
            not_async.join("\n  ")
        );
        let edit = whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(SignatureChange {
        symbol: name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&old_inner),
        new_signature: normalize(&new_inner),
        rule,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        unexpected,
        diagnostics,
        applied,
        returns: returns_change,
        visibility: visibility_change,
        asyncness: asyncness_change,
        not_async,
    })
}

/// The parameter list of `fn name(old_inner)` in `text`, found by the declaration's own text
/// rather than by a position that an earlier edit to the same file may have moved. `None`
/// unless it occurs exactly once.
pub(crate) fn locate_declaration(
    text: &str,
    name: &str,
    old_inner: &str,
) -> Option<(usize, usize)> {
    let needle = format!("fn {name}");
    let mut found = None;
    let mut from = 0;
    while let Some(i) = text[from..].find(&needle) {
        let at = from + i + 3; // the name, which is what `param_span` starts from
        from = at;
        // A longer name that starts with this one is not this function.
        let after = text[at + name.len()..].chars().next();
        if after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let Some((_, open, close)) = param_span(text, at) else {
            continue;
        };
        if text[open..close] == *old_inner {
            if found.is_some() {
                return None;
            }
            found = Some((open, close));
        }
    }
    found
}

/// A workspace edit that replaces each file wholesale, the shape the gateway answers a
/// structural rewrite with.
pub(crate) fn whole_file_edit(files: &BTreeMap<PathBuf, String>) -> serde_json::Value {
    let changes: Vec<serde_json::Value> = files
        .iter()
        .map(|(path, new_text)| {
            let old_lines = std::fs::read_to_string(path)
                .map(|t| t.lines().count())
                .unwrap_or(0);
            serde_json::json!({
                "textDocument": { "uri": prod_code_protocol::path::file_uri(path), "version": null },
                "edits": [ {
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": old_lines, "character": 0 }
                    },
                    "newText": new_text
                } ]
            })
        })
        .collect();
    serde_json::json!({ "documentChanges": changes })
}

/// Runs a structural rewrite over the whole workspace, resolved in `context`'s own scope.
///
/// The position is deliberately (0, 0): the engine then resolves the rule in the body of that
/// file's first function, which is the module the declaration lives in, so the bare name
/// resolves. A position on the declaration itself is an item position, where it may not.
pub(crate) async fn structural_replace(
    remote: SocketAddr,
    root: &Path,
    context: &Path,
    rule: &str,
) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(context)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", context))?
        .to_string();
    crate::tools::execute_lsp_query(
        remote,
        root,
        context,
        "prodCode/structuralReplace",
        serde_json::json!({
            "rule": rule,
            "scope": serde_json::Value::Null,
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": 0 },
        }),
    )
    .await
}

/// Every reference to the symbol at `file:line:col`, as (file, line, column), without the
/// declaration itself.
pub(crate) async fn references(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<Vec<Reference>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        "context": { "includeDeclaration": false },
    });
    let ask = || {
        crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/references",
            params.clone(),
        )
    };
    let mut res = ask().await?;
    // A language server still reading the project answers with no references, and a signature
    // refactoring would take that for "no callers" and leave every call site behind (#284).
    // rust-analyzer answers from a database that is already loaded.
    if crate::sync::engine_for_file(file) != Some("rust") {
        for _ in 0..crate::impact::COLD_RETRIES {
            if res.as_array().is_some_and(|refs| !refs.is_empty()) {
                break;
            }
            tokio::time::sleep(crate::impact::COLD_WAIT).await;
            res = ask().await?;
        }
    }
    parse_locations(&res)
}

/// The locations of a `textDocument/references` answer, 1-based. `null` is the protocol's "none";
/// any other answer that is not a list of locations, and any entry without a file or a start, is
/// an error: a planner that dropped it would rewrite every call site but that one.
fn parse_locations(res: &serde_json::Value) -> Result<Vec<Reference>> {
    if res.is_null() {
        return Ok(Vec::new());
    }
    let entries = res
        .as_array()
        .with_context(|| format!("the analyzer's references are not a list: {}", brief(res)))?;
    let mut out = Vec::with_capacity(entries.len());
    for (n, loc) in entries.iter().enumerate() {
        let position = |key: &str| {
            loc.pointer(&format!("/range/start/{key}"))
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
        };
        let (Some(uri), Some(l), Some(c)) = (
            loc.get("uri").and_then(|u| u.as_str()),
            position("line"),
            position("character"),
        ) else {
            anyhow::bail!(
                "reference {} of {} from the analyzer has no file or start position: {}",
                n + 1,
                entries.len(),
                brief(loc)
            );
        };
        // 1-based, and a position one past the last `u32` is in no file: an error, not a wrap.
        let (Some(line), Some(col)) = (l.checked_add(1), c.checked_add(1)) else {
            anyhow::bail!(
                "reference {} of {} from the analyzer is at line {l}, character {c} (0-based), \
                 which no file has: {}",
                n + 1,
                entries.len(),
                brief(loc)
            );
        };
        let path = local_file(uri).with_context(|| {
            format!(
                "reference {} of {} from the analyzer, `{uri}`, is not a local file URI",
                n + 1,
                entries.len()
            )
        })?;
        out.push((path, line, col));
    }
    Ok(out)
}

/// The path of a `file:` URI with no host other than `localhost`, percent-decoded; `None` for
/// any other scheme, a remote host, or a relative path. A reference the change cannot open is
/// a call site it can neither check nor rewrite.
fn local_file(uri: &str) -> Option<PathBuf> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    let path = url.to_file_path().ok()?;
    path.is_absolute().then_some(path)
}

/// An answer, cut short for an error message.
fn brief(value: &serde_json::Value) -> String {
    let text = value.to_string();
    match text.char_indices().nth(200) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// The text of a file the change reads, a caller or one it rewrites. A file that cannot be read
/// stops the change: read as empty, its calls would be neither checked nor rewritten.
fn read_caller(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}; nothing was written", path.display()))
}

/// A call of the function: where, for the report, and its arguments as written.
#[derive(Debug)]
struct CallSite {
    at: String,
    args: Vec<String>,
}

/// The references that are calls, with their arguments. A reference that does not call the
/// function (a use as a value, an import) has none and is left to the reconciliation, which
/// names it; a reference whose position does not hold a name, or whose argument list does not
/// close, stops the change.
fn call_sites(root: &Path, refs: &[Reference]) -> Result<Vec<CallSite>> {
    let mut texts: BTreeMap<&Path, String> = BTreeMap::new();
    let mut out = Vec::new();
    for (path, l, c) in refs {
        if !texts.contains_key(path.as_path()) {
            texts.insert(path.as_path(), read_caller(path)?);
        }
        let text = &texts[path.as_path()];
        let at = format!("{}:{l}:{c}", display(root, path));
        let offset = offset_of(text, *l, *c)
            .filter(|o| text[*o..].starts_with(|ch: char| ch.is_alphanumeric() || ch == '_'))
            .with_context(|| {
                format!(
                    "the analyzer's reference {at} does not point at a name; the file may have \
                     changed since it was read. Nothing was written"
                )
            })?;
        if let Some(args) = call_arguments(text, offset).with_context(|| {
            format!("the arguments of the call at {at} could not be read; nothing was written")
        })? {
            out.push(CallSite { at, args });
        }
    }
    Ok(out)
}

/// The arguments of the call whose callee's name starts at `at` (`f(…)`, `recv.f(…)`,
/// `Type::f::<T>(…)`), split at their top-level commas. `Ok(None)` when the name is not called
/// there; an error when the argument list does not close.
fn call_arguments(text: &str, at: usize) -> Result<Option<Vec<String>>> {
    let bytes = text.as_bytes();
    let skip_ws = |i: usize| i + (text[i..].len() - text[i..].trim_start().len());
    let mut i = at;
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 0x80)
    {
        i += 1;
    }
    i = skip_ws(i);
    if text[i..].starts_with("::") {
        let open = skip_ws(i + 2);
        if !text[open..].starts_with('<') {
            return Ok(None);
        }
        let mut depth = 0i32;
        let mut close = None;
        for (k, c) in text[open..].char_indices() {
            match c {
                '<' => depth += 1,
                '>' if !text[..open + k].ends_with('-') => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + k + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        i = skip_ws(close.context("the turbofish does not close")?);
    }
    if !text[i..].starts_with('(') {
        return Ok(None);
    }
    split_arguments(text, i)
        .map(Some)
        .context("the argument list does not close")
}

/// What starts at byte `i` of `text` and hides commas, brackets and comment markers inside it: a
/// string, raw string or character literal, or a comment. `Ok(Some((end, is_comment)))` with the
/// offset just past it, `Ok(None)` when none starts there (a lifetime has no closing quote), and
/// `Err(())` when one starts and does not close. Block comments nest, as Rust's do: the first
/// `*/` in `/* a /* b */ c */` does not end it. `from` is where the scan began.
fn opaque_at(text: &str, i: usize, from: usize) -> Result<Option<(usize, bool)>, ()> {
    let s = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let prev = if i > from { s[i - 1] } else { b' ' };
    match s[i] {
        // A raw string (`r"…"`, `r#"…"#`, `br"…"`): no escapes, closed by `"` and its hashes.
        b'r' if !ident(prev) || (matches!(prev, b'b' | b'c') && (i < 2 || !ident(s[i - 2]))) => {
            let mut j = i + 1;
            while s.get(j) == Some(&b'#') {
                j += 1;
            }
            if s.get(j) != Some(&b'"') {
                return Ok(None);
            }
            let close = format!("\"{}", "#".repeat(j - i - 1));
            let end = text[j + 1..].find(&close).ok_or(())? + j + 1 + close.len();
            Ok(Some((end, false)))
        }
        b'"' => {
            let mut j = i + 1;
            loop {
                match s.get(j).ok_or(())? {
                    b'\\' => j += 2,
                    b'"' => break,
                    _ => j += 1,
                }
            }
            Ok(Some((j + 1, false)))
        }
        b'\'' => {
            if s.get(i + 1) == Some(&b'\\') {
                let end = text.get(i + 3..).ok_or(())?.find('\'').ok_or(())? + i + 4;
                return Ok(Some((end, false)));
            }
            let len = text[i + 1..].chars().next().ok_or(())?.len_utf8();
            Ok((s.get(i + 1 + len) == Some(&b'\'')).then_some((i + 2 + len, false)))
        }
        b'/' if s.get(i + 1) == Some(&b'/') => Ok(Some((
            text[i..].find('\n').map_or(s.len(), |n| i + n),
            true,
        ))),
        b'/' if s.get(i + 1) == Some(&b'*') => {
            let mut depth = 0usize;
            let mut j = i;
            while j + 1 < s.len() {
                match (s[j], s[j + 1]) {
                    (b'/', b'*') => {
                        depth += 1;
                        j += 2;
                    }
                    (b'*', b'/') => {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            return Ok(Some((j, true)));
                        }
                    }
                    _ => j += 1,
                }
            }
            Err(())
        }
        _ => Ok(None),
    }
}

/// `expr` with each comment replaced by as many spaces as it has bytes, so that what is left is
/// code and offsets stay where they were; `None` when a literal or a comment does not close.
fn blank_comments(expr: &str) -> Option<String> {
    let mut out = expr.as_bytes().to_vec();
    let mut i = 0;
    while i < expr.len() {
        match opaque_at(expr, i, 0).ok()? {
            Some((end, comment)) => {
                if comment {
                    out[i..end].fill(b' ');
                }
                i = end;
            }
            None => i += 1,
        }
    }
    String::from_utf8(out).ok()
}

/// The arguments between the `(` at `open` and its `)`. Commas count only outside brackets,
/// string and character literals, comments (nested ones too) and turbofish generics
/// (`Vec::<(u8, u8)>::new()`).
fn split_arguments(text: &str, open: usize) -> Option<Vec<String>> {
    let s = text.as_bytes();
    let mut args = Vec::new();
    let (mut depth, mut angle) = (0i32, 0i32);
    let mut start = open + 1;
    let mut i = open + 1;
    while i < s.len() {
        if let Some((end, _)) = opaque_at(text, i, open + 1).ok()? {
            i = end;
            continue;
        }
        let c = s[i];
        let prev = if i > open + 1 { s[i - 1] } else { b' ' };
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b')' => {
                let last = text[start..i].trim();
                if !last.is_empty() {
                    args.push(last.to_string());
                }
                return Some(args);
            }
            b']' | b'}' => return None,
            b'<' if angle > 0 || text[..i].trim_end().ends_with("::") => angle += 1,
            b'>' if angle > 0 && prev != b'-' => angle -= 1,
            b',' if depth == 0 && angle == 0 => {
                args.push(text[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// What evaluating an argument can do, as far as its text and the analyzer show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArgKind {
    /// A literal: nothing to evaluate, and nothing another argument does changes it.
    Literal,
    /// A read of a local, a constant or a static (`x`, `a::B`, `&mut x`, `x as u64`) passed where
    /// no conversion can run code: no effect of its own, but another argument's effect can change
    /// what it reads.
    Place,
    /// Anything else: a call, a macro, an operator, an index, `?`, a block — it may do
    /// something, or panic.
    Effectful,
    /// It looks like a read, but it can run code the text does not show, and nothing rules that
    /// out; the reason.
    Unproven(&'static str),
}

/// Why a field read is not a plain read.
const FIELD_DEREF: &str = "reading a field calls a user `Deref` when the value's own type does \
                           not have that field";
/// Why an argument for a reference parameter is not a plain read.
const REF_COERCION: &str = "an argument for a reference parameter can be converted by a user \
                            `Deref` (`&Wrapper` passed for `&Inner`)";
/// Why an argument for a parameter whose type is not known is not a plain read.
const UNCONFIRMED_TYPE: &str = "the analyzer does not confirm that the parameter's type is a \
                                built-in scalar, a struct, an enum or a union, which no conversion \
                                into runs code";

/// What evaluating `expr`, passed for a parameter with `facts`, can do. Comments are not code:
/// they are blanked first, nested ones whole.
fn classify_arg(expr: &str, facts: &ParamFacts) -> ArgKind {
    let conversion = if facts.coercion_free {
        None
    } else if facts.reference {
        Some(REF_COERCION)
    } else {
        Some(UNCONFIRMED_TYPE)
    };
    match blank_comments(expr) {
        Some(code) => classify(code.trim(), conversion),
        None => ArgKind::Effectful,
    }
}

/// `conversion` is why converting the value to the parameter's type may run code, `None` when it
/// cannot.
fn classify(e: &str, conversion: Option<&'static str>) -> ArgKind {
    if let Some(rest) = e.strip_prefix('&') {
        let rest = rest.trim_start();
        let rest = match rest.strip_prefix("mut") {
            Some(r) if r.starts_with(char::is_whitespace) => r,
            _ => rest,
        };
        // Taking a reference runs nothing; converting it to the parameter's type may.
        return match (classify(rest.trim(), None), conversion) {
            (ArgKind::Place, Some(why)) => ArgKind::Unproven(why),
            (kind, _) => kind,
        };
    }
    // A cast is between built-in types, and what it makes is not converted by a `Deref`.
    if let Some((value, ty)) = e.rsplit_once(" as ")
        && is_path(ty.trim())
    {
        return classify(value.trim(), None);
    }
    if is_literal(e) {
        ArgKind::Literal
    } else if is_path(e) {
        conversion.map_or(ArgKind::Place, ArgKind::Unproven)
    } else if is_place(e) {
        ArgKind::Unproven(FIELD_DEREF)
    } else {
        ArgKind::Effectful
    }
}

fn is_literal(e: &str) -> bool {
    if matches!(e, "true" | "false" | "()") {
        return true;
    }
    let number = e.strip_prefix('-').unwrap_or(e);
    if number.starts_with(|c: char| c.is_ascii_digit()) {
        return !number.contains("..")
            && number
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    }
    is_string_literal(e) || is_char_literal(e)
}

fn is_string_literal(e: &str) -> bool {
    let Some(quote) = e.find('"') else {
        return false;
    };
    let prefix = &e[..quote];
    let body = &e[quote + 1..];
    if matches!(prefix, "" | "b" | "c") {
        let mut escaped = false;
        for (i, c) in body.char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => return i + 1 == body.len(),
                _ => {}
            }
        }
        return false;
    }
    let raw = prefix.strip_prefix(['b', 'c']).unwrap_or(prefix);
    let Some(hashes) = raw.strip_prefix('r') else {
        return false;
    };
    if !hashes.chars().all(|c| c == '#') {
        return false;
    }
    let close = format!("\"{hashes}");
    body.find(&close) == Some(body.len().wrapping_sub(close.len()))
}

fn is_char_literal(e: &str) -> bool {
    let inner = e
        .strip_prefix("b'")
        .or_else(|| e.strip_prefix('\''))
        .and_then(|r| r.strip_suffix('\''));
    match inner {
        Some("\\'") => true,
        Some(i) if i.starts_with('\\') => i.len() > 1 && !i[1..].contains('\''),
        Some(i) => i.chars().count() == 1,
        None => false,
    }
}

/// `x`, `a::B`, `self.field.0`: a path, then fields (`.await` is not one).
fn is_place(e: &str) -> bool {
    let mut parts = e.split('.');
    parts.next().is_some_and(is_path)
        && parts.all(|p| {
            (is_ident(p) && p != "await")
                || (!p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
}

fn is_path(e: &str) -> bool {
    let e = e.strip_prefix("::").unwrap_or(e);
    !e.is_empty() && e.split("::").all(is_ident)
}

/// The built-in scalar types, by the names that usually mean them.
const SCALARS: [&str; 16] = [
    "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128",
    "isize", "f32", "f64",
];

/// Types written with syntax rather than a name, which no declaration can shadow and which are
/// dropped without running code: references, raw and function pointers.
fn is_pointer(ty: &str) -> bool {
    ty.starts_with('&')
        || ty.starts_with("*const ")
        || ty.starts_with("*mut ")
        || ty.starts_with("fn(")
        || ty.starts_with("fn (")
        || ty.starts_with("unsafe fn")
        || ty.starts_with("unsafe extern ")
        || ty.starts_with("extern ")
}

/// The names in `ty` that have to be the built-in types they are spelled as for a value of it to
/// be dropped without running code, with their offsets in `ty`; `None` when it may run code
/// whatever the names are (`String`, a generic `T`, a type of the crate's own). References, raw
/// and function pointers never do, and scalars, `()`, and tuples, arrays and options of those do
/// not — if the names are the built-in ones: `enum Option<T>` with a `Drop`, or a `struct u32`,
/// is spelled the same.
fn drop_free_names(ty: &str) -> Option<Vec<(usize, String)>> {
    let mut names = Vec::new();
    drop_free_at(ty, 0, &mut names).then_some(names)
}

fn drop_free_at(ty: &str, base: usize, names: &mut Vec<(usize, String)>) -> bool {
    let base = base + (ty.len() - ty.trim_start().len());
    let ty = ty.trim();
    if ty == "()" || is_pointer(ty) {
        return true;
    }
    if SCALARS.contains(&ty) {
        names.push((base, ty.to_string()));
        return true;
    }
    if let Some(inner) = ty.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        return split_at_top_level(inner, ';')
            .is_some_and(|(elem, _)| drop_free_at(elem, base + 1, names));
    }
    if let Some(inner) = ty.strip_prefix('(').and_then(|t| t.strip_suffix(')')) {
        let (mut rest, mut at) = (inner, base + 1);
        loop {
            let (elem, tail) = match split_at_top_level(rest, ',') {
                Some((elem, tail)) => (elem, Some(tail)),
                None => (rest, None),
            };
            // `(T,)` ends with an empty element; nothing else may be empty.
            let trailing = tail.is_none() && elem.trim().is_empty();
            if !trailing && !drop_free_at(elem, at, names) {
                return false;
            }
            match tail {
                Some(tail) => {
                    at += elem.len() + 1;
                    rest = tail;
                }
                None => return true,
            }
        }
    }
    if let Some(inner) = ty.strip_prefix("Option<").and_then(|t| t.strip_suffix('>')) {
        names.push((base, "Option".to_string()));
        return drop_free_at(inner, base + "Option<".len(), names);
    }
    false
}

/// Whether converting an argument to `ty` can run code, and if that turns on a name, which one.
/// `Some(None)`: it cannot, whatever the names (a raw or function pointer, a tuple, an array,
/// `()`: no `Deref` makes one). `Some(Some((offset, name)))`: it cannot if `name`, at `offset` in
/// `ty`, is a built-in scalar or a struct, enum or union rather than an alias that may stand for
/// a reference. `None`: it can — a reference is the target of `Deref` coercion, and `impl` or
/// `dyn` are not a type the analyzer can be asked about by name.
fn coercion_name(ty: &str) -> Option<Option<(usize, String)>> {
    let base = ty.len() - ty.trim_start().len();
    let ty = ty.trim();
    if ty.starts_with('&') {
        return None;
    }
    if ty == "()" || is_pointer(ty) || ty.starts_with('(') || ty.starts_with('[') {
        return Some(None);
    }
    let path = &ty[..ty.find('<').unwrap_or(ty.len())];
    if !is_path(path) || (path.len() < ty.len() && !ty.ends_with('>')) {
        return None;
    }
    let at = path.rfind("::").map_or(0, |i| i + 2);
    Some(Some((base + at, path[at..].to_string())))
}

/// The code blocks of a hover before its documentation, without their language tags.
fn hover_blocks(markdown: &str) -> Vec<&str> {
    let head = markdown.split("\n---").next().unwrap_or("");
    head.split("```")
        .skip(1)
        .step_by(2)
        .map(|block| block.split_once('\n').map_or("", |(_, body)| body).trim())
        .collect()
}

/// Whether a hover over `name` says it is the built-in scalar, or the standard library's
/// `Option`, that the name usually means. rust-analyzer describes a built-in type by its name
/// alone, and a declared one by the module it is in and then the declaration: a `struct u32` or
/// an `enum Option<T>` of the crate's own reads `crate_name` and `struct u32`.
fn hover_is_builtin(markdown: &str, name: &str) -> bool {
    let blocks = hover_blocks(markdown);
    if name == "Option" {
        return blocks.len() == 2
            && matches!(blocks[0], "core::option" | "std::option")
            && blocks[1].starts_with("pub enum Option<");
    }
    SCALARS.contains(&name) && blocks == [name]
}

/// Whether a hover over `name` says it is a struct, an enum or a union, as opposed to a type
/// alias or a generic parameter, either of which may stand for a reference.
fn hover_is_adt(markdown: &str, name: &str) -> bool {
    let blocks = hover_blocks(markdown);
    let [_, item] = blocks[..] else {
        return false;
    };
    let item = match item.strip_prefix("pub(") {
        Some(rest) => rest.split_once(')').map_or("", |(_, r)| r).trim_start(),
        None => item.strip_prefix("pub ").unwrap_or(item),
    };
    ["struct ", "enum ", "union "].iter().any(|kw| {
        item.strip_prefix(kw).is_some_and(|rest| {
            rest.strip_prefix(name)
                .is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
        })
    })
}

/// What the analyzer confirmed about a parameter's type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ParamFacts {
    /// Dropping a value of it runs no code.
    drop_free: bool,
    /// No conversion of an argument into it runs code.
    coercion_free: bool,
    /// It is written as a reference, the type `Deref` coercion converts to.
    reference: bool,
    /// Names the type would be drop-free by if they were the built-in types, which the analyzer
    /// did not confirm they are.
    unconfirmed: Vec<String>,
}

/// The hover's markdown at byte `at` of `file`, asked once per place; `None` when it fails or
/// has none, which confirms nothing.
async fn hover_markdown(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    at: usize,
    asked: &mut BTreeMap<usize, Option<String>>,
) -> Option<String> {
    if let Some(known) = asked.get(&at) {
        return known.clone();
    }
    let (line, col) = line_col_at(text, at);
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
    .ok();
    let markdown = hover
        .as_ref()
        .and_then(|h| h.get("contents"))
        .and_then(|c| {
            c.as_str()
                .or_else(|| c.get("value").and_then(|v| v.as_str()))
        })
        .map(str::to_string);
    asked.insert(at, markdown.clone());
    markdown
}

/// What the analyzer confirms about the type of each declared parameter, asked by hovering the
/// names in the declaration's own text (its parameter list is `text[open..close]`). A name the
/// analyzer does not describe as expected is not taken on trust.
async fn param_facts(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    open: usize,
    close: usize,
    declared: &[Declared],
) -> Vec<ParamFacts> {
    let mut asked = BTreeMap::new();
    let mut out = Vec::with_capacity(declared.len());
    // In order, each after the one before: `a: u32` is also the end of `aa: u32`.
    let mut from = open;
    for d in declared {
        let mut facts = ParamFacts::default();
        let found = text[from..close].find(&d.raw).map(|i| from + i);
        let typed = found.and_then(|at| {
            from = at + d.raw.len();
            split_at_top_level(&d.raw, ':').map(|(head, ty)| (at + head.len() + 1, ty))
        });
        let Some((ty_at, ty)) = typed else {
            out.push(facts);
            continue;
        };
        facts.reference = ty.trim_start().starts_with('&');
        if let Some(names) = drop_free_names(ty) {
            facts.drop_free = true;
            for (offset, name) in names {
                let hover = hover_markdown(remote, root, file, text, ty_at + offset, &mut asked);
                if !hover.await.is_some_and(|h| hover_is_builtin(&h, &name)) {
                    facts.drop_free = false;
                    if !facts.unconfirmed.contains(&name) {
                        facts.unconfirmed.push(name);
                    }
                }
            }
        }
        facts.coercion_free = match coercion_name(ty) {
            None => false,
            Some(None) => true,
            Some(Some((offset, name))) => {
                hover_markdown(remote, root, file, text, ty_at + offset, &mut asked)
                    .await
                    .is_some_and(|h| hover_is_builtin(&h, &name) || hover_is_adt(&h, &name))
            }
        };
        out.push(facts);
    }
    out
}

/// What the new argument order `args` and the parameters it leaves out would change at run time,
/// one line per place: two arguments that would be evaluated the other way round when either
/// can have an effect the other sees, two owned parameters that would be dropped the other way
/// round, and a removed argument that does something or is an owned value the function drops.
/// `facts` is what the analyzer confirmed about each parameter's type; what it did not confirm
/// counts as able to run code. The receiver is evaluated first before and after, so it is not an
/// argument here; a call written `Type::f(recv, …)` passes it first and it is skipped.
fn effect_hazards(
    name: &str,
    declared: &[Declared],
    facts: &[ParamFacts],
    has_receiver: bool,
    args: &[Option<usize>],
    calls: &[CallSite],
) -> Vec<String> {
    let unknown = ParamFacts::default();
    let fact = |i: usize| facts.get(i).unwrap_or(&unknown);
    let kept: Vec<usize> = args.iter().flatten().copied().collect();
    // (i, j), declared i before j, that the new order passes j before i.
    let mut swapped = Vec::new();
    for (p, &later) in kept.iter().enumerate() {
        for &earlier in &kept[p + 1..] {
            if earlier < later {
                swapped.push((earlier, later));
            }
        }
    }
    let removed: Vec<usize> = (0..declared.len()).filter(|i| !kept.contains(i)).collect();
    let mut out = Vec::new();
    for &(i, j) in &swapped {
        let (a, b) = (&declared[i], &declared[j]);
        if !fact(i).drop_free && !fact(j).drop_free {
            let mut line = format!(
                "`{name}` drops `{}` before `{}` when it returns (parameters are dropped in \
                 reverse order of declaration); the new order drops `{}` first",
                b.raw.trim(),
                a.raw.trim(),
                a.name
            );
            let mut unconfirmed: Vec<&str> = Vec::new();
            for n in fact(i).unconfirmed.iter().chain(&fact(j).unconfirmed) {
                if !unconfirmed.contains(&n.as_str()) {
                    unconfirmed.push(n);
                }
            }
            if !unconfirmed.is_empty() {
                line.push_str(&format!(
                    " (the analyzer does not confirm that `{}` is the built-in or standard \
                     library type the name usually means, and a type of that name may have a \
                     `Drop`)",
                    unconfirmed.join("`, `")
                ));
            }
            out.push(line);
        }
    }
    for call in calls {
        let own = if has_receiver && call.args.len() == declared.len() + 1 {
            &call.args[1..]
        } else {
            &call.args[..]
        };
        if own.len() != declared.len() {
            out.push(format!(
                "{}: the call passes {} argument(s) and `{name}` declares {}, so what the \
                 rewrite does to it cannot be checked",
                call.at,
                own.len(),
                declared.len()
            ));
            continue;
        }
        let kinds: Vec<ArgKind> = own
            .iter()
            .enumerate()
            .map(|(i, a)| classify_arg(a, fact(i)))
            .collect();
        for &d in &removed {
            let param = &declared[d];
            match kinds[d] {
                ArgKind::Effectful => out.push(format!(
                    "{}: `{}` is evaluated for `{}`, and removing the parameter removes what it \
                     does",
                    call.at, own[d], param.name
                )),
                ArgKind::Unproven(why) => out.push(format!(
                    "{}: `{}` is evaluated for `{}`, and removing the parameter may remove what \
                     it does: {why}",
                    call.at, own[d], param.name
                )),
                _ if !fact(d).drop_free => out.push(format!(
                    "{}: `{}` is moved into `{}` and dropped when `{name}` returns; without the \
                     parameter it is dropped at another time, or not at all",
                    call.at,
                    own[d],
                    param.raw.trim()
                )),
                _ => {}
            }
        }
        for &(i, j) in &swapped {
            let independent = kinds[i] == ArgKind::Literal
                || kinds[j] == ArgKind::Literal
                || (kinds[i] == ArgKind::Place && kinds[j] == ArgKind::Place);
            if !independent {
                let mut line = format!(
                    "{}: `{}` and `{}` would be evaluated in the opposite order",
                    call.at, own[i], own[j]
                );
                let mut why: Vec<&str> = Vec::new();
                for kind in [kinds[i], kinds[j]] {
                    if let ArgKind::Unproven(reason) = kind
                        && !why.contains(&reason)
                    {
                        why.push(reason);
                    }
                }
                if !why.is_empty() {
                    line.push_str(&format!(" ({})", why.join("; ")));
                }
                out.push(line);
            }
        }
    }
    out
}

/// How many files [`unreported_callers`] names at most: each one is opened and checked with the
/// rewritten ones.
const UNREPORTED_LIMIT: usize = 20;

/// The files of `file`'s language under `root`, other than `file` and `checked`, that write
/// `name(`: where a caller can be that the analyzer did not report. A language server may answer
/// `references` from an index that is not there yet (sourcekit-lsp reads the one a build writes,
/// and finds nothing in a package never built, #294), and a caller a refactoring did not rewrite
/// then breaks unseen unless its file is checked together with the rewritten ones. A file that
/// only has another function of the same name costs a check and changes nothing. Rust is not
/// searched: rust-analyzer answers from the crate graph it has loaded.
pub(crate) fn unreported_callers(
    root: &Path,
    file: &Path,
    name: &str,
    checked: &[PathBuf],
) -> Vec<PathBuf> {
    let family = |p: &Path| match crate::lang::language_id_for_path(p) {
        "c" | "cpp" | "objective-c" | "objective-cpp" => "c",
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => "javascript",
        other => other,
    };
    let wanted = family(file);
    if name.is_empty() || matches!(wanted, "rust" | "plaintext") {
        return Vec::new();
    }
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let base = canonical(root);
    let known: Vec<PathBuf> = checked
        .iter()
        .map(|p| canonical(p))
        .chain([canonical(file)])
        .collect();
    let mut found = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .max_filesize(Some(1 << 20))
        .build()
        .flatten()
    {
        let path = entry.path();
        if !entry.file_type().is_some_and(|t| t.is_file()) || family(path) != wanted {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let path = canonical(path);
        if !writes_call(&text, name) || known.contains(&path) {
            continue;
        }
        // Spelled under `root` as the caller gave it, so that it is translated like the others.
        found.push(match path.strip_prefix(&base) {
            Ok(rel) => root.join(rel),
            Err(_) => path,
        });
        if found.len() == UNREPORTED_LIMIT {
            break;
        }
    }
    found.sort();
    found
}

/// Whether `text` writes `name(` with `name` as a whole word, a call or a declaration.
fn writes_call(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(ident)
            && text[at + name.len()..].trim_start().starts_with('(')
    })
}

pub(crate) fn line_col_at(text: &str, offset: usize) -> (u32, u32) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32 + 1;
    let col = before
        .rsplit('\n')
        .next()
        .map(|l| l.chars().count() as u32 + 1)
        .unwrap_or(1);
    (line, col)
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// A parameter list on one line, for the report.
fn normalize(list: &str) -> String {
    let one_line = list.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.trim_end_matches(',').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file that calls the function is found though no analyzer reported it; the declaring
    /// file, a file already checked, another language, and a longer name that starts the same
    /// are not (#294). C and C++ are one family: a C function is called from C++ too.
    #[test]
    fn files_that_call_the_function_but_were_not_reported_are_found() {
        let ws = prod_code_testkit::Workspace::new(&[
            (
                "Sources/Shop/Pricing.swift",
                "func price(qty: Int) -> Int { qty }\n",
            ),
            ("Sources/Shop/main.swift", "print(price (qty: 3))\n"),
            ("Sources/Shop/Checked.swift", "print(price(qty: 4))\n"),
            (
                "Sources/Shop/Other.swift",
                "let a = prices(1) + unit_price(2)\n",
            ),
            ("tools/notes.py", "price(3)\n"),
            ("src/pricing.c", "int price(int qty) { return qty; }\n"),
            ("src/app.cpp", "int main() { return price(3); }\n"),
            ("src/lib.rs", "fn f() { price(3); }\n"),
        ]);
        let root = ws.root();
        assert_eq!(
            unreported_callers(
                &root,
                &root.join("Sources/Shop/Pricing.swift"),
                "price",
                &[root.join("Sources/Shop/Checked.swift")],
            ),
            vec![root.join("Sources/Shop/main.swift")]
        );
        assert_eq!(
            unreported_callers(&root, &root.join("src/pricing.c"), "price", &[]),
            vec![root.join("src/app.cpp")]
        );
        assert!(
            unreported_callers(&root, &root.join("src/lib.rs"), "price", &[]).is_empty(),
            "Rust is not searched"
        );
        assert!(writes_call("x = price(1)", "price"));
        assert!(!writes_call("x = price", "price"));
    }

    /// A language server that answers "no references" while it reads the project is asked
    /// again before the answer is believed; rust-analyzer is believed at once (#284).
    #[tokio::test]
    async fn an_empty_answer_from_a_cold_server_is_asked_again() {
        let ws = prod_code_testkit::Workspace::new(&[
            ("pricing.py", "def price(qty):\n    return qty\n"),
            ("lib.rs", "pub fn price() {}\n"),
        ]);
        let root_buf = ws.root();
        let root = root_buf.as_path();
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = std::sync::Arc::clone(&asked);
        let caller = root.join("cart.py");
        let gateway = prod_code_testkit::ScriptedGateway::start(move |method, _| {
            if method != "textDocument/references" {
                return serde_json::Value::Null;
            }
            // Empty for the first two questions, as a server still indexing answers.
            if count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                return serde_json::json!([]);
            }
            serde_json::json!([{
                "uri": url::Url::from_file_path(&caller).unwrap().to_string(),
                "range": { "start": { "line": 4, "character": 11 }, "end": { "line": 4, "character": 16 } }
            }])
        })
        .await;
        let refs = references(gateway.addr(), root, &root.join("pricing.py"), 1, 5)
            .await
            .unwrap();
        assert_eq!(refs.len(), 1, "{refs:?}");
        assert_eq!((refs[0].1, refs[0].2), (5, 12));
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 3);

        asked.store(0, std::sync::atomic::Ordering::SeqCst);
        let refs = references(gateway.addr(), root, &root.join("lib.rs"), 1, 8)
            .await
            .unwrap();
        assert!(refs.is_empty());
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// #442: a reply that is not a list of locations, or an entry without a file or a start, is
    /// an error rather than a caller that silently is not there; `null` is the protocol's "none".
    #[test]
    fn a_malformed_reference_answer_is_an_error_not_a_missing_caller() {
        assert!(
            parse_locations(&serde_json::Value::Null)
                .unwrap()
                .is_empty()
        );
        let good = serde_json::json!([
            { "uri": "file:///w/a.rs", "range": { "start": { "line": 1, "character": 4 } } }
        ]);
        assert_eq!(
            parse_locations(&good).unwrap(),
            [(PathBuf::from("/w/a.rs"), 2, 5)]
        );
        let cases = [
            (serde_json::json!({ "error": "busy" }), "not a list"),
            (
                serde_json::json!([good[0].clone(), { "range": good[0]["range"].clone() }]),
                "reference 2 of 2",
            ),
            (
                serde_json::json!([{ "uri": "file:///w/a.rs" }]),
                "reference 1 of 1",
            ),
            (
                serde_json::json!([{ "uri": "file:///w/a.rs",
                    "range": { "start": { "line": -1, "character": 0 } } }]),
                "no file or start position",
            ),
            (
                serde_json::json!([{ "uri": "file:///w/a.rs",
                    "range": { "start": { "line": u32::MAX, "character": 0 } } }]),
                "which no file has",
            ),
            (
                serde_json::json!([{ "uri": "file:///w/a.rs",
                    "range": { "start": { "line": 0, "character": u32::MAX } } }]),
                "which no file has",
            ),
            (
                serde_json::json!([{ "uri": "file:///w/a.rs",
                    "range": { "start": { "line": u64::from(u32::MAX) + 1, "character": 0 } } }]),
                "no file or start position",
            ),
        ];
        for uri in [
            "untitled:Untitled-1",
            "https://example.com/a.rs",
            "file://build-host/w/a.rs",
            "/w/a.rs",
            "a.rs",
        ] {
            let reply = serde_json::json!([{ "uri": uri,
                "range": { "start": { "line": 0, "character": 0 } } }]);
            let err = parse_locations(&reply).unwrap_err().to_string();
            assert!(err.contains("is not a local file URI"), "{uri}: {err}");
        }
        // Percent-encoded and `localhost` spellings are the same local file.
        let spelled = serde_json::json!([
            { "uri": "file:///w/a%20b.rs", "range": { "start": { "line": 0, "character": 0 } } },
            { "uri": "file://localhost/w/a.rs", "range": { "start": { "line": 0, "character": 0 } } }
        ]);
        assert_eq!(
            parse_locations(&spelled).unwrap(),
            [
                (PathBuf::from("/w/a b.rs"), 1, 1),
                (PathBuf::from("/w/a.rs"), 1, 1)
            ]
        );
        for (reply, why) in cases {
            let err = parse_locations(&reply).unwrap_err().to_string();
            assert!(err.contains(why), "{reply}: {err}");
        }
    }

    /// Facts for parameters the analyzer described as expected: every name in their types is the
    /// built-in type, or a struct or an enum, it is spelled as.
    fn confirmed(declared: &[Declared]) -> Vec<ParamFacts> {
        declared
            .iter()
            .map(|d| {
                let ty = split_at_top_level(&d.raw, ':').map_or("", |(_, ty)| ty);
                ParamFacts {
                    drop_free: drop_free_names(ty).is_some(),
                    coercion_free: coercion_name(ty).is_some(),
                    reference: ty.trim_start().starts_with('&'),
                    unconfirmed: Vec::new(),
                }
            })
            .collect()
    }

    /// #442: what the text of an argument, and what the analyzer confirmed about the parameter's
    /// type, say about evaluating it.
    #[test]
    fn arguments_are_told_apart_by_what_evaluating_them_can_do() {
        let free = ParamFacts {
            drop_free: true,
            coercion_free: true,
            ..Default::default()
        };
        let reference = ParamFacts {
            drop_free: true,
            reference: true,
            ..Default::default()
        };
        let unknown = ParamFacts::default();
        for literal in [
            "1",
            "-2",
            "0x1F_u8",
            "1.5f32",
            "true",
            "\"a, b\"",
            "b\"x\"",
            "r#\"q\"#",
            "'c'",
            "'\\n'",
            "b'x'",
            "&5",
            "&\"lit\"",
            "/* c */ 7",
            "\"/* not a comment */\"",
        ] {
            for facts in [&free, &reference, &unknown] {
                assert_eq!(classify_arg(literal, facts), ArgKind::Literal, "{literal}");
            }
        }
        for place in [
            "x",
            "crate::LIMIT",
            "&k",
            "&mut buf",
            "& mut buf",
            "n as u64",
            "x /* a, /* b, */ c */",
            "/* a */ x // b\n",
        ] {
            assert_eq!(classify_arg(place, &free), ArgKind::Place, "{place}");
        }
        // A cast makes a built-in value that no `Deref` converts.
        assert_eq!(classify_arg("n as u64", &unknown), ArgKind::Place);
        // A field read can go through `Deref`, whatever the parameter.
        for field in ["self.a.0", "&self.items", "a.n", "&mut w.buf", "a /* */ .n"] {
            let kind = classify_arg(field, &free);
            assert!(
                kind == ArgKind::Unproven(FIELD_DEREF) || kind == ArgKind::Effectful,
                "{field}: {kind:?}"
            );
        }
        assert_eq!(classify_arg("a.n", &free), ArgKind::Unproven(FIELD_DEREF));
        // Passed for a reference, a value or a reference to it can be converted by `Deref`; for
        // a type the analyzer did not describe, by whatever that type turns out to be.
        for arg in ["k", "&owned", "&mut buf", "crate::LIMIT"] {
            assert_eq!(
                classify_arg(arg, &reference),
                ArgKind::Unproven(REF_COERCION),
                "{arg}"
            );
            assert_eq!(
                classify_arg(arg, &unknown),
                ArgKind::Unproven(UNCONFIRMED_TYPE),
                "{arg}"
            );
        }
        for effect in [
            "x /* /* */",
            "/* unclosed x",
            "mark() /* /* */ */",
            "/* x, /* y */ */ mark()",
            "mark(\"a\")",
            "x.len()",
            "v[0]",
            "a + 1",
            "*r",
            "f()?",
            "fut.await",
            "vec![1]",
            "{ x }",
            "\"a\" \"b\"",
            "1..2",
            "\"a\".len()",
            "Noisy(\"x\")",
            "|x| x",
            "&mut make()",
        ] {
            assert_eq!(classify_arg(effect, &free), ArgKind::Effectful, "{effect}");
        }
    }

    /// #442: a comment ends where its nesting does, so a comma, a quote or a call inside a
    /// nested comment is not an argument, and an argument after it is not hidden in it.
    #[test]
    fn nested_block_comments_are_one_comment() {
        let text = "join(/* a, /* b, \" */ c, */ x, /* ' */ mark(\"y\"))";
        assert_eq!(
            call_arguments(text, 0).unwrap().unwrap(),
            ["/* a, /* b, \" */ c, */ x", "/* ' */ mark(\"y\")"]
        );
        let blanked = blank_comments("/* a /* b */ c */ x").unwrap();
        assert_eq!(blanked.trim(), "x");
        assert_eq!(blanked.len(), "/* a /* b */ c */ x".len());
        assert_eq!(
            blank_comments("\"/*\" /* é */ y").unwrap(),
            format!("\"/*\" {}y", " ".repeat("/* é */ ".len()))
        );
        assert!(call_arguments("join(x /* /* */, y)", 0).is_err());
        assert!(blank_comments("x /* /* */").is_none());
    }

    /// #442: rust-analyzer's hovers, verbatim in shape: a built-in type is described by its name
    /// alone, a declared one by its module and then its declaration.
    #[test]
    fn a_hover_confirms_a_builtin_or_a_declared_type_only_as_it_is_written() {
        let builtin = "\n```rust\nu32\n```\n\n---\n\nThe 32-bit unsigned integer type.";
        let option = "\n```rust\ncore::option\n```\n\n```rust\npub enum Option<T> {\n    None,\n    Some( /* … */ ),\n}\n```\n\n---\n\nThe `Option` type.";
        let own_option = "```rust\nfixture\n```\n\n```rust\npub enum Option<T> {\n    None,\n    Some( /* … */ ),\n}\n```";
        let own_u32 = "```rust\nfixture\n```\n\n```rust\npub(crate) struct u32\n```";
        let alias = "```rust\nfixture\n```\n\n```rust\npub type Inner = &'static Wrap\n```";
        let generic = "```rust\nT\n```";
        assert!(hover_is_builtin(builtin, "u32"));
        assert!(hover_is_builtin(option, "Option"));
        assert!(!hover_is_builtin(own_option, "Option"));
        assert!(!hover_is_builtin(own_u32, "u32"));
        assert!(!hover_is_builtin(builtin, "u64"));
        assert!(!hover_is_builtin(generic, "T"));
        assert!(hover_is_adt(option, "Option"));
        assert!(hover_is_adt(own_option, "Option"));
        assert!(hover_is_adt(own_u32, "u32"));
        assert!(!hover_is_adt(alias, "Inner"));
        assert!(!hover_is_adt(generic, "T"));
        assert!(!hover_is_adt(own_u32, "u3"));
    }

    #[test]
    fn a_conversion_into_a_type_is_ruled_out_by_its_shape_or_its_name() {
        assert_eq!(coercion_name("&Inner"), None);
        assert_eq!(coercion_name(" &'a mut [u8]"), None);
        assert_eq!(coercion_name("impl AsRef<str>"), None);
        assert_eq!(coercion_name("dyn Fn()"), None);
        for free in ["*const u8", "fn(u32) -> u32", "(u8, &str)", "[u8; 4]", "()"] {
            assert_eq!(coercion_name(free), Some(None), "{free}");
        }
        assert_eq!(coercion_name(" u32"), Some(Some((1, "u32".into()))));
        assert_eq!(
            coercion_name("std::option::Option<&T>"),
            Some(Some((13, "Option".into())))
        );
        assert_eq!(coercion_name("Vec<u8"), None);
    }

    #[test]
    fn call_arguments_are_split_at_their_own_commas_only() {
        let text = "let n = join(\"a, (b\", ',', Vec::<(u8, u8)>::new(), f(x, y), r#\"q\"#, 'a', /* c, */ z);\n";
        assert_eq!(
            call_arguments(text, text.find("join").unwrap())
                .unwrap()
                .unwrap(),
            [
                "\"a, (b\"",
                "','",
                "Vec::<(u8, u8)>::new()",
                "f(x, y)",
                "r#\"q\"#",
                "'a'",
                "/* c, */ z"
            ]
        );
        let method = "    make().send::<u8>(a, b)\n";
        assert_eq!(
            call_arguments(method, method.find("send").unwrap())
                .unwrap()
                .unwrap(),
            ["a", "b"]
        );
        // A use as a value and an import are not calls.
        let value = "let g = join;\nmap(join)\n";
        assert_eq!(
            call_arguments(value, value.find("join").unwrap()).unwrap(),
            None
        );
        assert_eq!(
            call_arguments(value, value.rfind("join").unwrap()).unwrap(),
            None
        );
        assert_eq!(
            call_arguments("join(\n    a,\n    b,\n)", 0)
                .unwrap()
                .unwrap(),
            ["a", "b"]
        );
        assert!(call_arguments("join()", 0).unwrap().unwrap().is_empty());
        assert!(call_arguments("join(a, b", 0).is_err());
    }

    /// A type is drop-free by its shape, and by names the analyzer then has to confirm; each
    /// name comes with where it is, which is where the analyzer is asked.
    #[test]
    fn only_types_without_drop_code_are_drop_free() {
        let names = |ty: &str| {
            drop_free_names(ty).map(|n| {
                n.into_iter()
                    .map(|(at, name)| {
                        assert_eq!(&ty[at..at + name.len()], name, "{ty}");
                        name
                    })
                    .collect::<Vec<_>>()
            })
        };
        for (free, expected) in [
            ("u32", vec!["u32"]),
            (" u32", vec!["u32"]),
            ("&str", vec![]),
            ("&mut Vec<String>", vec![]),
            ("*const u8", vec![]),
            ("fn(u32) -> u32", vec![]),
            ("(u8, bool)", vec!["u8", "bool"]),
            ("( u8 ,bool, )", vec!["u8", "bool"]),
            ("[u8; 4]", vec!["u8"]),
            ("Option<&T>", vec!["Option"]),
            ("Option<(u32, [i8; 2])>", vec!["Option", "u32", "i8"]),
            ("()", vec![]),
        ] {
            let expected: Vec<String> = expected.into_iter().map(String::from).collect();
            assert_eq!(names(free), Some(expected), "{free}");
        }
        for owned in [
            "String",
            "Vec<u8>",
            "T",
            "impl Fn()",
            "Noisy",
            "(u8, String)",
            "[String; 2]",
            "Option<String>",
            "Box<u8>",
            "core::primitive::u32",
            "(u8,,u8)",
        ] {
            assert_eq!(names(owned), None, "{owned}");
        }
    }

    /// The calls to `name` in `text`, each reported by its line.
    fn sites(text: &str, name: &str) -> Vec<CallSite> {
        text.match_indices(name)
            .filter_map(|(at, _)| {
                call_arguments(text, at).unwrap().map(|args| CallSite {
                    at: line_col_at(text, at).0.to_string(),
                    args,
                })
            })
            .collect()
    }

    /// #442, the reproduction: a reorder of `f(mark("a"), mark("b"))` runs the marks the other
    /// way round, a reorder of two owned parameters drops them the other way round, and removing
    /// a parameter removes what its argument did. A literal, a string and a reference do not.
    #[test]
    fn a_reorder_or_removal_that_changes_effects_or_drops_is_named() {
        const DEMO: &str = "pub fn demo(k: &u32) {\n    eff_pair(mark(\"a\"), mark(\"b\"));\n    eff_owned(x, y);\n    eff_unused(1, mark(\"b\"));\n    eff_simple(3, \"lit\", k);\n    eff_pair(\"a\", mark(\"b\"));\n}\n";
        let swap = [Some(1), Some(0)];
        let pair = parse_declared("first: &str, second: &str").1;
        assert_eq!(
            effect_hazards(
                "eff_pair",
                &pair,
                &confirmed(&pair),
                false,
                &swap,
                &sites(DEMO, "eff_pair")
            ),
            ["2: `mark(\"a\")` and `mark(\"b\")` would be evaluated in the opposite order"],
            "a literal and a call on line 6 are independent"
        );
        let owned = parse_declared("x: Noisy, y: Noisy").1;
        let hazards = effect_hazards(
            "eff_owned",
            &owned,
            &confirmed(&owned),
            false,
            &swap,
            &sites(DEMO, "eff_owned"),
        );
        assert_eq!(hazards.len(), 1, "{hazards:?}");
        assert!(
            hazards[0].contains("`eff_owned` drops `y: Noisy` before `x: Noisy`"),
            "{hazards:?}"
        );
        let unused = parse_declared("a: u32, _b: &str").1;
        assert_eq!(
            effect_hazards(
                "eff_unused",
                &unused,
                &confirmed(&unused),
                false,
                &[Some(0)],
                &sites(DEMO, "eff_unused")
            ),
            [
                "4: `mark(\"b\")` is evaluated for `_b`, and removing the parameter removes what \
              it does"
            ]
        );
        let simple = parse_declared("n: u32, s: &str, r: &u32").1;
        let facts = confirmed(&simple);
        let calls = sites(DEMO, "eff_simple");
        assert!(
            effect_hazards(
                "s",
                &simple,
                &facts,
                false,
                &[Some(2), Some(0), Some(1)],
                &calls
            )
            .is_empty(),
            "a reference moved past literals keeps its meaning"
        );
        // `k` passed for `&u32` may be a `&Wrapper` that `Deref` converts: removing it may
        // remove that call.
        assert_eq!(
            effect_hazards("s", &simple, &facts, false, &[Some(1), Some(0)], &calls),
            [format!(
                "5: `k` is evaluated for `r`, and removing the parameter may remove what it \
                 does: {REF_COERCION}"
            )]
        );
        // Two scalars the analyzer confirmed are reordered and removed freely; unconfirmed, the
        // same names may be a `struct u32` with a `Drop`.
        let scalars = parse_declared("a: u32, b: u32").1;
        let calls = sites("f(p, /* /* */ q */ q)", "f");
        let facts = confirmed(&scalars);
        assert!(effect_hazards("f", &scalars, &facts, false, &swap, &calls).is_empty());
        assert!(effect_hazards("f", &scalars, &facts, false, &[Some(0)], &calls).is_empty());
        let unconfirmed = vec![
            ParamFacts {
                unconfirmed: vec!["u32".into()],
                ..Default::default()
            };
            2
        ];
        let hazards = effect_hazards("f", &scalars, &unconfirmed, false, &swap, &calls);
        assert_eq!(hazards.len(), 2, "{hazards:?}");
        assert!(
            hazards[0].contains("drops `b: u32` before `a: u32`")
                && hazards[0].contains("does not confirm that `u32` is the built-in"),
            "{hazards:?}"
        );
        assert!(
            hazards[1].ends_with(&format!("in the opposite order ({UNCONFIRMED_TYPE})")),
            "{hazards:?}"
        );
        // An owned value that is removed is dropped somewhere else, or never.
        let guard = parse_declared("a: u32, g: Guard").1;
        let hazards = effect_hazards(
            "f",
            &guard,
            &confirmed(&guard),
            false,
            &[Some(0)],
            &sites("f(1, g)", "f"),
        );
        assert!(
            hazards[0].contains("`g` is moved into `g: Guard`"),
            "{hazards:?}"
        );
        // A call that does not pass what the declaration takes cannot be judged.
        let hazards = effect_hazards(
            "eff_pair",
            &pair,
            &confirmed(&pair),
            false,
            &swap,
            &sites("eff_pair(a)", "eff_pair"),
        );
        assert!(hazards[0].contains("passes 1 argument(s)"), "{hazards:?}");
    }

    /// The receiver is evaluated first before and after a reorder, so it is not an argument; a
    /// call written with the type's path passes it first, and it is skipped.
    #[test]
    fn a_method_receiver_keeps_its_place_and_a_path_call_is_read_past_it() {
        let (receiver, declared) = parse_declared("&self, a: &str, b: &str");
        let calls = "fn f(s: S, x: &str) {\n    make().m(x, \"lit\");\n    s.m(mark(\"a\"), \"b\");\n    s.m(x, mark(\"b\"));\n    S::m(&s, mark(\"a\"), mark(\"b\"));\n}\n";
        let hazards = effect_hazards(
            "m",
            &declared,
            &confirmed(&declared),
            receiver.is_some(),
            &[Some(1), Some(0)],
            &sites(calls, "m("),
        );
        assert_eq!(
            hazards,
            [
                format!(
                    "4: `x` and `mark(\"b\")` would be evaluated in the opposite order \
                     ({REF_COERCION})"
                ),
                "5: `mark(\"a\")` and `mark(\"b\")` would be evaluated in the opposite order"
                    .to_string()
            ]
        );
    }

    #[test]
    fn a_parameter_is_found_with_its_function_and_the_ones_that_stay() {
        let t =
            "impl S {\n    pub fn join<T: Into<String>>(&self, a: T, mut b: &str, c: u8) {}\n}\n";
        let (fn_at, name, kept) = parameter_at(t, 2, 51).expect("`b` is a parameter");
        assert_eq!(&t[fn_at..fn_at + 4], "join");
        assert_eq!(name, "b");
        assert_eq!(kept, ["a", "c"]);
        // `a` is one too; the function's name and `self` are not.
        assert_eq!(parameter_at(t, 2, 41).map(|p| p.1).as_deref(), Some("a"));
        assert!(parameter_at(t, 2, 12).is_none());
        assert!(parameter_at(t, 2, 35).is_none());
        let call = "fn f(x: u8) {\n    g(x, 1);\n}\n";
        assert!(
            parameter_at(call, 2, 7).is_none(),
            "an argument is not a parameter"
        );
    }

    #[test]
    fn a_return_type_is_replaced_added_and_removed() {
        let t = "pub fn total(xs: &[u32]) -> u32 {\n    0\n}\n";
        let close = t.find(')').unwrap();
        let (was, out) = with_return_type(t, close, "u64");
        assert_eq!(was, "u32");
        assert!(
            out.starts_with("pub fn total(xs: &[u32]) -> u64 {"),
            "{out}"
        );
        let (_, out) = with_return_type(t, close, "()");
        assert!(out.starts_with("pub fn total(xs: &[u32]) {"), "{out}");
        let none = "fn log(s: &str) {\n}\n";
        let (was, out) = with_return_type(none, none.find(')').unwrap(), "bool");
        assert_eq!(was, "()");
        assert!(out.starts_with("fn log(s: &str) -> bool {"), "{out}");
        let generic = "fn f<T>(x: T) -> Vec<T> where T: Clone {\n}\n";
        let (was, out) = with_return_type(generic, generic.find(')').unwrap(), "Option<T>");
        assert_eq!(was, "Vec<T>");
        assert!(out.contains("-> Option<T> where T: Clone"), "{out}");
    }

    #[test]
    fn a_visibility_is_replaced_added_and_removed() {
        let t = "    pub async fn run() {}\n";
        let at = t.find("run").unwrap();
        let (was, out) = with_visibility(t, at, "pub(crate)").unwrap();
        assert_eq!(was, "pub");
        assert_eq!(out, "    pub(crate) async fn run() {}\n");
        let (was, out) = with_visibility(&out, out.find("run").unwrap(), "private").unwrap();
        assert_eq!(was, "pub(crate)");
        assert_eq!(out, "    async fn run() {}\n");
        let (was, out) = with_visibility(&out, out.find("run").unwrap(), "pub").unwrap();
        assert_eq!(was, "private");
        assert_eq!(out, "    pub async fn run() {}\n");
    }

    #[test]
    fn parses_a_kept_parameter_and_a_new_one() {
        assert_eq!(parse_param(" root ").unwrap(), Param::Keep("root".into()));
        assert_eq!(
            parse_param("budget: usize = 0").unwrap(),
            Param::Add {
                name: "budget".into(),
                ty: "usize".into(),
                value: "0".into()
            }
        );
    }

    #[test]
    fn a_new_parameter_keeps_commas_and_arrows_inside_its_type() {
        let p = parse_param("map: HashMap<String, Vec<u8>> = HashMap::new()").unwrap();
        assert_eq!(
            p,
            Param::Add {
                name: "map".into(),
                ty: "HashMap<String, Vec<u8>>".into(),
                value: "HashMap::new()".into()
            }
        );
        let f = parse_param("f: fn(u32) -> u32 = |x| x").unwrap();
        assert!(matches!(f, Param::Add { ref ty, .. } if ty == "fn(u32) -> u32"));
    }

    #[test]
    fn a_new_parameter_without_an_expression_is_refused() {
        let err = parse_param("budget: usize").unwrap_err().to_string();
        assert!(err.contains("expression"), "{err}");
    }

    #[test]
    fn finds_the_parameter_list_past_generics_and_lifetimes() {
        let text = "fn f<'a, T: Into<String>>(a: &'a str, b: T) -> u32 { 0 }\n";
        let (name, open, close) = param_span(text, 3).unwrap();
        assert_eq!(name, "f");
        assert_eq!(&text[open..close], "a: &'a str, b: T");
    }

    #[test]
    fn splits_parameters_without_splitting_their_types() {
        let params = split_params("a: HashMap<String, u64>, b: (u32, u32), c: impl Fn() -> u32");
        assert_eq!(params.len(), 3);
        assert_eq!(params[2], "c: impl Fn() -> u32");
    }

    #[test]
    fn a_receiver_is_kept_out_of_the_parameters() {
        let (recv, params) = parse_declared("&mut self, file: &Path, text: &str");
        assert_eq!(recv.as_deref(), Some("&mut self"));
        assert_eq!(
            params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["file", "text"]
        );
    }

    #[test]
    fn a_reorder_becomes_a_rule_with_a_placeholder_per_argument() {
        let declared = parse_declared("remote: SocketAddr, root: &Path").1;
        let request = [Param::Keep("root".into()), Param::Keep("remote".into())];
        let plan = plan(&declared, &request).unwrap();
        assert!(plan.dropped.is_empty());
        assert_eq!(
            call_site_rule("validate", false, declared.len(), &plan.args, &[]),
            "validate($a0, $a1) ==>> validate($a1, $a0)"
        );
    }

    #[test]
    fn an_added_parameter_is_spelled_out_at_the_call_sites() {
        let declared = parse_declared("path: &Path").1;
        let request = [
            Param::Keep("path".into()),
            Param::Add {
                name: "budget".into(),
                ty: "usize".into(),
                value: "4096".into(),
            },
        ];
        let plan = plan(&declared, &request).unwrap();
        assert_eq!(plan.list, ["path: &Path", "budget: usize"]);
        assert_eq!(
            call_site_rule("read", true, declared.len(), &plan.args, &["4096"]),
            "$recv.read($a0) ==>> $recv.read($a0, 4096)"
        );
    }

    #[test]
    fn a_dropped_parameter_is_reported_by_name() {
        let declared = parse_declared("a: u32, b: u32").1;
        let plan = plan(&declared, &[Param::Keep("a".into())]).unwrap();
        assert_eq!(plan.dropped, ["b"]);
    }

    #[test]
    fn an_unknown_parameter_names_the_ones_there_are() {
        let declared = parse_declared("a: u32, b: u32").1;
        let err = plan(&declared, &[Param::Keep("c".into())])
            .unwrap_err()
            .to_string();
        assert!(err.contains("a, b"), "{err}");
    }

    #[test]
    fn a_multiline_list_stays_multiline() {
        let old = "\n    a: u32,\n    b: u32,\n";
        let out = format_list(old, None, &["b: u32".into(), "a: u32".into()]);
        assert_eq!(out, "\n    b: u32,\n    a: u32,\n");
    }

    #[test]
    fn a_call_spans_from_its_name_to_its_closing_parenthesis() {
        let text = "fn main() {\n    join(\n        \"a\",\n        \"b\",\n    );\n}\n";
        assert_eq!(call_span_lines(text, 2, 5), Some((2, 5)));
    }

    #[test]
    fn a_reference_is_matched_against_its_own_call_not_its_neighbours() {
        // The line above a rewritten one is not rewritten, however close it is: this is the
        // case a proximity check called done, hiding a reference the rule never matched.
        let old = "let f = join;\nlet s = join(a, b);\n";
        let new = "let f = join;\nlet s = join(b, a);\n";
        let changed = changed_lines(old, new);
        assert_eq!(changed, [2]);
        // The function used as a value has no argument list at all, so there is no call span
        // and the reference stands for its own line — which nothing rewrote.
        assert_eq!(call_span_lines(old, 1, 9), None);
        let span = call_span_lines(old, 1, 9).unwrap_or((1, 1));
        assert!(
            !changed.iter().any(|l| *l >= span.0 && *l <= span.1),
            "the value use was not rewritten"
        );
    }

    #[test]
    fn a_position_becomes_an_offset_and_back() {
        let text = "fn a() {}\nfn b(x: u32) {}\n";
        let offset = offset_of(text, 2, 4).expect("line 2 exists");
        assert_eq!(&text[offset..offset + 1], "b");
        assert_eq!(line_col_at(text, offset), (2, 4));
    }

    #[test]
    fn the_signature_is_reported_on_one_line() {
        assert_eq!(normalize("\n    a: u32,\n    b: u32,\n"), "a: u32, b: u32");
    }

    #[test]
    fn async_goes_before_unsafe_and_callers_are_told_apart() {
        let t = "pub unsafe fn raw() {}\n";
        let out = with_async(t, t.find("fn raw").unwrap(), true);
        assert_eq!(out, "pub async unsafe fn raw() {}\n");
        assert_eq!(with_async(&out, out.find("fn raw").unwrap(), false), t);
        let plain = "fn f() {}\n";
        assert_eq!(with_async(plain, 0, true), "async fn f() {}\n");
        let callers = "async fn a() {\n    load(1)\n}\nfn b() {\n    load(2)\n}\n";
        assert!(in_async_fn(callers, callers.find("load(1)").unwrap()));
        assert!(!in_async_fn(callers, callers.find("load(2)").unwrap()));
        assert!(!in_async_fn("load(3)", 0));
    }

    #[test]
    fn a_whole_file_edit_covers_the_file_it_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        std::fs::write(&path, "fn a() {}\nfn b() {}\n").unwrap();
        let mut files = BTreeMap::new();
        files.insert(path.clone(), "fn a() {}\n".to_string());
        let edit = whole_file_edit(&files);
        let change = &edit["documentChanges"][0];
        assert_eq!(change["edits"][0]["range"]["start"]["line"], 0);
        assert_eq!(change["edits"][0]["range"]["end"]["line"], 2);
        assert_eq!(change["edits"][0]["newText"], "fn a() {}\n");
    }

    #[test]
    fn consecutive_changed_lines_are_one_hunk() {
        assert_eq!(hunks(vec![10, 11, 12, 40]), [(10, 12), (40, 40)]);
    }

    #[test]
    fn a_declaration_is_found_by_its_text_once_and_only_once() {
        let text =
            "fn caller() { join(1, 2); }\n\nfn join(a: u8, b: u8) {}\nfn joined(a: u8, b: u8) {}\n";
        let (open, close) = locate_declaration(text, "join", "a: u8, b: u8").expect("found");
        assert_eq!(&text[open..close], "a: u8, b: u8");
        assert!(
            text[..open].ends_with("fn join("),
            "the one declared as `join`, not `joined` or the call"
        );
        // Changed, or there twice: not guessed at.
        assert_eq!(locate_declaration(text, "join", "a: u16, b: u8"), None);
        let twice = "fn join(a: u8) {}\nmod m { fn join(a: u8) {} }\n";
        assert_eq!(locate_declaration(twice, "join", "a: u8"), None);
    }
}
