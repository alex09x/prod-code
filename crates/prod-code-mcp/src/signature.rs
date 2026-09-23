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
//!   before anything is written, which is what catches a reorder of two different types.

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
}

impl SignatureChange {
    pub fn render(&self, diff_budget: usize) -> String {
        let mut out = format!(
            "`{}` ({})\n\n- was: ({})\n- now: ({})\n",
            self.symbol, self.file, self.old_signature, self.new_signature
        );
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
            let Some(d) = declared.iter().find(|d| &d.name == gone) else {
                continue;
            };
            let at = text[open..close].find(&d.raw).map(|i| open + i);
            let Some(at) = at else { continue };
            let (l, c) = line_col_at(&text, at);
            let refs = references(remote, root, file, l, c)
                .await
                .unwrap_or_default();
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
    rewritten.insert(file.to_path_buf(), decl_text);

    // Reconcile: what the analyzer knows is a reference against what the rewrite touched.
    // Line proximity is not good enough — a reference on the line above a rewritten one looks
    // rewritten and is not — so each reference is matched against the lines of its own call,
    // from the callee's name to the closing parenthesis.
    let refs = references(remote, root, file, line, col)
        .await
        .unwrap_or_default();
    let mut unmatched = Vec::new();
    let mut unexpected = Vec::new();
    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut touched: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
    for (path, new_text) in &rewritten {
        let old = std::fs::read_to_string(path).unwrap_or_default();
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
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &[]).await?;
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
                "textDocument": { "uri": format!("file://{}", path.display()), "version": null },
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
    let res = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/references",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "context": { "includeDeclaration": false },
        }),
    )
    .await?;
    let mut out = Vec::new();
    for loc in res.as_array().into_iter().flatten() {
        let Some(uri) = loc.get("uri").and_then(|u| u.as_str()) else {
            continue;
        };
        let path = PathBuf::from(crate::remote_fs::uri_to_path(uri));
        let l = loc
            .pointer("/range/start/line")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32
            + 1;
        let c = loc
            .pointer("/range/start/character")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32
            + 1;
        out.push((path, l, c));
    }
    Ok(out)
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
