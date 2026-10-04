//! Extracting a delegate: some fields of a struct and the methods that work only on them move
//! into a new helper type, which the struct then holds (Extract Class).
//!
//! rust-analyzer offers delegates for one field at a time (`generate_delegate_methods`,
//! `generate_delegate_trait`); nothing moves a group of fields with their behaviour. Here:
//! - the fields named leave the struct, and one field of the new type takes their place;
//! - the methods named move to `impl Helper`, and the struct keeps a method of the same
//!   signature that forwards to it, so no caller changes;
//! - every other access to a moved field goes through the new field (`a.city` becomes
//!   `a.address.city`), found through the analyzer's references;
//! - every struct literal of the type builds the helper (`Account { city, .. }` becomes
//!   `Account { address: Address { city }, .. }`).
//!
//! A moved method may use only the moved fields and the other moved methods. The whole change is
//! type-checked before anything is written.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use crate::parameter_object::Language;

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Top-level pieces of `text` separated by `sep`, as byte ranges, outside every bracket.
pub fn split_top(text: &str, sep: char) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    let mut prev = ' ';
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' => depth += 1,
            '>' if prev != '-' && prev != '=' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push((start, i));
                start = i + c.len_utf8();
            }
            _ => {}
        }
        prev = c;
    }
    if !text[start..].trim().is_empty() {
        out.push((start, text.len()));
    }
    out
}

/// One field of a struct: its text from its first attribute or doc line to its comma.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub vis: String,
    /// The field's text without the trailing comma, first line trimmed of its indentation.
    pub text: String,
}

/// A named struct: where its declaration (attributes included) starts, its braces and fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructDecl {
    pub name: String,
    pub vis: String,
    pub start: usize,
    pub open: usize,
    pub close: usize,
    pub derive: Option<String>,
    pub fields: Vec<Field>,
}

/// The struct whose `struct` keyword is on the line of `at`.
pub fn parse_struct(text: &str, at: usize) -> Result<StructDecl> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    let kw = text[line_start..line_end]
        .find("struct ")
        .map(|i| line_start + i)
        .context("no `struct` on this line")?;
    let vis = text[line_start..kw].trim().to_string();
    let name: String = text[kw + 7..]
        .trim_start()
        .chars()
        .take_while(|c| is_ident(*c))
        .collect();
    anyhow::ensure!(!name.is_empty(), "the struct has no name");
    let after_name = kw + 7 + text[kw + 7..].find(&name).unwrap_or(0) + name.len();
    let rest = text[after_name..].trim_start();
    anyhow::ensure!(
        rest.starts_with('{'),
        "`{name}` is not a plain struct with named fields (generic and tuple structs are not supported)"
    );
    let open = after_name + (text[after_name..].len() - rest.len());
    let close = crate::parameter_object::matching_bracket(text, open)
        .context("the struct is not closed")?;
    // The attributes and doc comments above belong to the declaration.
    let mut start = line_start;
    let mut derive = None;
    loop {
        let above = text[..start].trim_end_matches('\n');
        let prev_start = above.rfind('\n').map_or(0, |i| i + 1);
        let prev = above[prev_start..].trim();
        if start == 0 || !(prev.starts_with("#[") || prev.starts_with("///")) {
            break;
        }
        if prev.starts_with("#[derive(") {
            derive = Some(prev.to_string());
        }
        start = prev_start;
    }
    let body = &text[open + 1..close];
    let mut fields = Vec::new();
    for (s, e) in split_top(body, ',') {
        let chunk = body[s..e].trim();
        let decl = crate::extract_trait::declaration(chunk).1;
        let (fvis, bare) = crate::extract_trait::visibility(decl);
        let Some((fname, _)) = bare.split_once(':') else {
            continue;
        };
        fields.push(Field {
            name: fname.trim().to_string(),
            vis: fvis.to_string(),
            text: chunk.to_string(),
        });
    }
    Ok(StructDecl {
        name,
        vis,
        start,
        open,
        close,
        derive,
        fields,
    })
}

/// The inherent `impl Name` blocks of `text`, as (start of `impl`, open, close).
pub fn impl_blocks(text: &str, name: &str) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("impl ") {
        if text[..i].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let Some(open) = text[i..].find('{').map(|o| i + o) else {
            continue;
        };
        let header = text[i + 5..open].trim();
        if header == name
            && let Some(close) = crate::parameter_object::matching_bracket(text, open)
        {
            out.push((i, open, close));
        }
    }
    out
}

/// The parameter names of a signature, `self` left out: `(&self, a: u32, mut b: T)` gives `a, b`.
pub fn argument_names(sig: &str) -> Option<Vec<String>> {
    let open = sig.find('(')?;
    let close = crate::parameter_object::matching_bracket(sig, open)?;
    let mut out = Vec::new();
    for (s, e) in split_top(&sig[open + 1..close], ',') {
        let p = sig[open + 1 + s..open + 1 + e].trim();
        if p.is_empty() || p.ends_with("self") {
            continue;
        }
        let pat = p.split_once(':')?.0.trim();
        let pat = pat.strip_prefix("mut ").unwrap_or(pat);
        if !pat.chars().all(is_ident) {
            return None;
        }
        out.push(pat.to_string());
    }
    Some(out)
}

/// Rewrites every struct literal `Name { .. }` (and `Self { .. }` inside `self_ranges`) in
/// `text` whose entries include one of `moved`: those entries go into `field: Helper { .. }`.
pub fn rewrite_literals(
    text: &str,
    name: &str,
    self_ranges: &[(usize, usize)],
    moved: &[String],
    field: &str,
    helper: &str,
) -> Result<String> {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for word in [name, "Self"] {
        for (i, _) in text.match_indices(word) {
            let before = text[..i].trim_end();
            if text[..i].chars().next_back().is_some_and(is_ident)
                || text[i + word.len()..].chars().next().is_some_and(is_ident)
                || before.ends_with("struct")
                || before.ends_with("impl")
                || before.ends_with("for")
                || before.ends_with("->")
                || (word == "Self" && !self_ranges.iter().any(|(s, e)| *s < i && i < *e))
            {
                continue;
            }
            let rest = &text[i + word.len()..];
            let trimmed = rest.trim_start();
            if !trimmed.starts_with('{') {
                continue;
            }
            let open = i + word.len() + (rest.len() - trimmed.len());
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
                continue;
            };
            let body = &text[open + 1..close];
            let entries: Vec<&str> = split_top(body, ',')
                .into_iter()
                .map(|(s, e)| body[s..e].trim())
                .filter(|e| !e.is_empty())
                .collect();
            let key = |e: &str| -> String { e.split(':').next().unwrap_or(e).trim().to_string() };
            let inner: Vec<&str> = entries
                .iter()
                .copied()
                .filter(|e| moved.contains(&key(e)))
                .collect();
            if inner.is_empty() {
                continue;
            }
            anyhow::ensure!(
                !entries.iter().any(|e| e.starts_with("..")),
                "a literal or pattern of `{name}` uses `..`; rewrite it by hand"
            );
            let indent: String = body
                .trim_start_matches(['\n'])
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let nested = format!("{field}: {helper} {{ {} }}", inner.join(", "));
            let mut all: Vec<String> = Vec::new();
            let mut placed = false;
            for e in &entries {
                if moved.contains(&key(e)) {
                    if !placed {
                        all.push(nested.clone());
                        placed = true;
                    }
                } else {
                    all.push(e.to_string());
                }
            }
            let new_body = if body.contains('\n') {
                let close_indent: String = text[..close]
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect();
                format!(
                    "\n{indent}{},\n{close_indent}",
                    all.join(&format!(",\n{indent}"))
                )
            } else {
                format!(" {} ", all.join(", "))
            };
            edits.push((open + 1, close, new_body));
        }
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_string();
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    Ok(out)
}

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Extracted {
    pub helper: String,
    pub field: String,
    pub fields: Vec<String>,
    pub methods: Vec<String>,
    #[serde(skip)]
    pub root: PathBuf,
    pub rewritten: Vec<(String, String)>,
    pub accesses: usize,
    pub unmatched: Vec<String>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Extracted {
    pub fn render(&self) -> String {
        let mut out = format!(
            "`{}: {}` holds {}; {} method(s) moved, {} access(es) rerouted\n",
            self.field,
            self.helper,
            self.fields.join(", "),
            self.methods.len(),
            self.accesses
        );
        for (path, new_text) in &self.rewritten {
            let full = Path::new(path);
            let old_text = if self.applied {
                crate::refactor::text_before_apply(full)
            } else {
                std::fs::read_to_string(full).unwrap_or_default()
            };
            let rel = full
                .strip_prefix(&self.root)
                .unwrap_or(full)
                .display()
                .to_string();
            out.push('\n');
            out.push_str(
                &similar::TextDiff::from_lines(old_text.as_str(), new_text.as_str())
                    .unified_diff()
                    .context_radius(1)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );
        }
        if !self.unmatched.is_empty() {
            out.push_str("\nunresolved references (the edit is incomplete):\n");
            for reference in &self.unmatched {
                out.push_str(&format!("  • {reference}\n"));
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
        out.push_str(if self.applied {
            "\n[applied]\n"
        } else {
            "\nnothing was written; pass `apply: true` to make this edit\n"
        });
        out
    }
}

/// The struct and impl changes in the declaring file, once every outside access already goes
/// through `field`: the fields leave the struct, the helper type is declared after it, and the
/// methods move behind forwarding methods.
pub fn restructure(
    text: &str,
    at: usize,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<String> {
    let decl = parse_struct(text, at)?;
    for f in fields {
        anyhow::ensure!(
            decl.fields.iter().any(|d| &d.name == f),
            "`{}` has no field `{f}`; it has {}",
            decl.name,
            decl.fields
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    anyhow::ensure!(
        decl.fields.iter().all(|d| d.name != field),
        "`{}` already has a field `{field}`",
        decl.name
    );
    let moved_fields: Vec<&Field> = decl
        .fields
        .iter()
        .filter(|f| fields.contains(&f.name))
        .collect();
    let widest = if moved_fields.iter().any(|f| f.vis == "pub") {
        "pub"
    } else {
        moved_fields
            .iter()
            .map(|f| f.vis.as_str())
            .find(|v| !v.is_empty())
            .unwrap_or("")
    };
    let indent = "    ";
    let mut kept: Vec<String> = Vec::new();
    let mut placed = false;
    for f in &decl.fields {
        if fields.contains(&f.name) {
            if !placed {
                let v = if widest.is_empty() {
                    String::new()
                } else {
                    format!("{widest} ")
                };
                kept.push(format!("{indent}{v}{field}: {helper},"));
                placed = true;
            }
        } else {
            kept.push(format!("{indent}{},", f.text));
        }
    }
    let struct_vis = if decl.vis.is_empty() {
        String::new()
    } else {
        format!("{} ", decl.vis)
    };
    let new_struct_body = format!("{{\n{}\n}}", kept.join("\n"));
    let helper_decl = format!(
        "{}{struct_vis}struct {helper} {{\n{}\n}}\n",
        decl.derive
            .as_ref()
            .map(|d| format!("{d}\n"))
            .unwrap_or_default(),
        moved_fields
            .iter()
            .map(|f| format!("{indent}{},", f.text))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Methods: taken out of every `impl Name` block, a forwarding method left in their place.
    let mut out = text.to_string();
    let mut moved_items: Vec<String> = Vec::new();
    let mut found: Vec<String> = Vec::new();
    for (_, open, close) in impl_blocks(text, &decl.name).into_iter().rev() {
        let items = crate::extract_trait::items(text, open, close);
        for item in items.iter().rev() {
            let Some(mname) = item.name.as_ref().filter(|n| methods.contains(n)) else {
                continue;
            };
            let chunk = &text[item.start..item.end];
            let (leading, decl_text) = crate::extract_trait::declaration(chunk);
            let sig = crate::extract_trait::signature(decl_text)
                .with_context(|| format!("cannot read the signature of `{mname}`"))?;
            anyhow::ensure!(
                sig.contains("&self") || sig.contains("&mut self"),
                "`{mname}` does not take `&self` or `&mut self`; only methods on a borrowed \
                 receiver can forward to the helper"
            );
            // It may use only the moved fields and the other moved methods.
            let body = &decl_text[sig.len()..];
            for (i, _) in body.match_indices("self.") {
                let used: String = body[i + 5..].chars().take_while(|c| is_ident(*c)).collect();
                anyhow::ensure!(
                    fields.contains(&used) || methods.contains(&used),
                    "`{mname}` uses `self.{used}`, which does not move to `{helper}`"
                );
            }
            let names = argument_names(sig).with_context(|| {
                format!("`{mname}` has a parameter pattern this cannot forward")
            })?;
            let item_indent: String = text[..item.start]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let attrs: String = leading
                .iter()
                .map(|l| format!("{l}\n{item_indent}"))
                .collect();
            let stub = format!(
                "{attrs}{sig} {{\n{item_indent}    self.{field}.{mname}({})\n{item_indent}}}",
                names.join(", ")
            );
            out.replace_range(item.start..item.end, &stub);
            moved_items.push(format!("{item_indent}{}", chunk.trim()));
            found.push(mname.clone());
        }
    }
    for m in methods {
        anyhow::ensure!(
            found.contains(m),
            "`{}` has no method `{m}` in an inherent `impl` block",
            decl.name
        );
    }
    moved_items.reverse();

    // The struct itself: rewritten last, it comes before every impl block in the offsets above
    // only if it is declared first; find it again in the text as it is now.
    let decl_now = parse_struct(
        &out,
        out.find(&format!("struct {} ", decl.name)).unwrap_or(at),
    )?;
    let helper_impl = if moved_items.is_empty() {
        String::new()
    } else {
        format!("\nimpl {helper} {{\n{}\n}}\n", moved_items.join("\n\n"))
    };
    out.replace_range(
        decl_now.open..decl_now.close + 1,
        format!("{new_struct_body}\n\n{helper_decl}{helper_impl}").trim_end(),
    );
    Ok(out)
}

fn is_ident_str(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_ident)
}

fn reindent(text: &str, indent: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                let stripped = if l.len() >= min_indent {
                    &l[min_indent..]
                } else {
                    l.trim_start()
                };
                format!("{indent}{stripped}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn extract_param_names_ts(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() {
            continue;
        }
        let before_colon = p.split(':').next().unwrap_or(p).trim();
        let name = before_colon.split_whitespace().last().unwrap_or("").trim_start_matches("...");
        if is_ident_str(name) {
            names.push(name.to_string());
        }
    }
    names
}

fn extract_param_names_py(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() || p == "self" || p.starts_with("self:") {
            continue;
        }
        let before_equal = p.split('=').next().unwrap_or(p).trim();
        let before_colon = before_equal.split(':').next().unwrap_or(before_equal).trim();
        let name = before_colon.trim_start_matches('*');
        if is_ident_str(name) {
            names.push(name.to_string());
        }
    }
    names
}

fn extract_param_names_cpp(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() {
            continue;
        }
        let before_equal = p.split('=').next().unwrap_or(p).trim();
        let name = before_equal.split_whitespace().last().unwrap_or("").trim_matches(['&', '*']);
        if is_ident_str(name) {
            names.push(name.to_string());
        }
    }
    names
}

fn extract_param_names_swift(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() {
            continue;
        }
        let before_colon = p.split(':').next().unwrap_or(p).trim();
        let parts: Vec<&str> = before_colon.split_whitespace().collect();
        if parts.len() >= 2 {
            let label = parts[0];
            let name = parts[1];
            if label == "_" {
                names.push(name.to_string());
            } else {
                names.push(format!("{label}: {name}"));
            }
        } else if let Some(name) = parts.first() && is_ident_str(name) {
            names.push(format!("{name}: {name}"));
        }
    }
    names
}

fn extract_param_names_go(params: &str) -> Vec<String> {
    let mut names = Vec::new();
    for piece in split_top(params, ',') {
        let p = params[piece.0..piece.1].trim();
        if p.is_empty() {
            continue;
        }
        let name = p.split_whitespace().next().unwrap_or("");
        if is_ident_str(name) {
            names.push(name.to_string());
        }
    }
    names
}

#[allow(clippy::too_many_arguments)]
pub fn restructure_ts(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    is_js: bool,
) -> Result<(String, String)> {
    let (open_brace, close_brace, owner, is_export) = {
        let mut found = None;
        for (i, _) in text.match_indices("class ") {
            if i > 0 && text[..i].chars().next_back().is_some_and(is_ident) {
                continue;
            }
            let after = &text[i + 6..];
            let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if name.is_empty() {
                continue;
            }
            if let Some(sym) = symbol_opt && sym != name {
                continue;
            }
            let Some(open_rel) = after.find('{') else { continue };
            let open = i + 6 + open_rel;
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else { continue };
            if let Some(line) = line_opt && line > 0 {
                let (sl, _) = crate::signature::position_at(text, i)?;
                let (el, _) = crate::signature::position_at(text, close)?;
                if line < sl || line > el {
                    continue;
                }
            }
            let before_class = text[..i].trim_end();
            let is_export = before_class.ends_with("export") || before_class.ends_with("export default");
            found = Some((open, close, name, is_export));
            break;
        }
        found.with_context(|| {
            if let Some(sym) = symbol_opt {
                format!("class `{sym}` not found")
            } else {
                "no class found at the specified location".to_string()
            }
        })?
    };

    let body = &text[open_brace + 1..close_brace];

    let mut field_decls: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }
        if let Some(semi) = trimmed.strip_suffix(';') {
            let decl = semi.trim();
            let without_init = decl.split('=').next().unwrap_or(decl).trim();
            if let Some((lhs, ty)) = without_init.split_once(':') {
                let fname = lhs.split_whitespace().last().unwrap_or("").trim();
                if is_ident_str(fname) && !fname.starts_with("return") {
                    field_decls.insert(fname.to_string(), (line.to_string(), Some(ty.trim().to_string())));
                }
            } else {
                let fname = without_init.split_whitespace().last().unwrap_or("").trim();
                if is_ident_str(fname) && !fname.starts_with("constructor") && !fname.starts_with("return") {
                    field_decls.insert(fname.to_string(), (line.to_string(), None));
                }
            }
        }
    }

    let mut ctor_assigned_fields: BTreeMap<String, String> = BTreeMap::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("this.") && trimmed.contains('=') && trimmed.ends_with(';') {
            let without_semi = &trimmed[..trimmed.len() - 1];
            if let Some((lhs, rhs)) = without_semi.split_once('=') {
                let fname = lhs.trim().trim_start_matches("this.").trim();
                if is_ident_str(fname) {
                    ctor_assigned_fields.insert(fname.to_string(), rhs.trim().to_string());
                }
            }
        }
    }

    for f in fields {
        anyhow::ensure!(
            field_decls.contains_key(f) || ctor_assigned_fields.contains_key(f) || body.contains(&format!("this.{f}")),
            "`{owner}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !field_decls.contains_key(field) && !ctor_assigned_fields.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    struct TsMethod {
        vis: String,
        is_async: bool,
        params: String,
        ret_type: Option<String>,
        full_text: String,
        body: String,
        start_line: usize,
        end_line: usize,
    }

    let mut moved_methods: BTreeMap<String, TsMethod> = BTreeMap::new();
    let mut offset = 0;
    while offset < body.len() {
        let slice = &body[offset..];
        let Some(paren_rel) = slice.find('(') else { break };
        let paren_pos = offset + paren_rel;
        let before_paren = body[offset..paren_pos].trim();
        let last_word = before_paren.split_whitespace().last().unwrap_or("");
        if is_ident_str(last_word)
            && last_word != "constructor"
            && last_word != "if"
            && last_word != "while"
            && last_word != "for"
            && last_word != "switch"
            && let Some(close_paren_rel) = body[paren_pos..].find(')')
        {
            let close_paren = paren_pos + close_paren_rel;
            let params = body[paren_pos + 1..close_paren].trim().to_string();
            let after_close = &body[close_paren + 1..];
            if let Some(open_b_rel) = after_close.find('{') {
                let open_b = close_paren + 1 + open_b_rel;
                let between = body[close_paren + 1..open_b].trim();
                let ret_type = between.strip_prefix(':').map(|s| s.trim().to_string());
                    if let Some(close_b_rel) = crate::parameter_object::matching_bracket(&body[open_b..], 0) {
                        let close_b = open_b + close_b_rel;
                        let line_start = body[..offset + slice[..paren_rel].rfind('\n').map_or(0, |x| x + 1)]
                            .rfind('\n')
                            .map_or(0, |x| x + 1);
                        let m_start = body[line_start..paren_pos]
                            .find(last_word)
                            .map(|x| line_start + x)
                            .unwrap_or(paren_pos - last_word.len());
                        let header_part = body[line_start..m_start].trim();
                        let is_async = header_part.contains("async");
                        let vis = if header_part.contains("private") {
                            "private".to_string()
                        } else if header_part.contains("protected") {
                            "protected".to_string()
                        } else if header_part.contains("public") {
                            "public".to_string()
                        } else {
                            String::new()
                        };
                        let method_text = body[line_start..=close_b].trim().to_string();
                        let m_body = body[open_b + 1..close_b].to_string();
                        let start_line = body[..line_start].matches('\n').count();
                        let end_line = body[..close_b].matches('\n').count();
                        let method_name = last_word.to_string();
                        if methods.contains(&method_name) {
                            moved_methods.insert(
                                method_name,
                                TsMethod {
                                    vis,
                                    is_async,
                                    params,
                                    ret_type,
                                    full_text: method_text,
                                    body: m_body,
                                    start_line,
                                    end_line,
                                },
                            );
                        }
                        offset = close_b + 1;
                        continue;
                    }
                }
            }
        offset = paren_pos + 1;
    }

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        for (idx, _) in minfo.body.match_indices("this.") {
            let after = &minfo.body[idx + 5..];
            let ident: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if !ident.is_empty() && ident != "constructor" && !fields.contains(&ident) && !methods.contains(&ident) {
                anyhow::bail!("`{mname}` uses `{ident}`, which does not move to `{helper}`");
            }
        }
    }

    // Build Helper class
    let export_kw = if is_export { "export " } else { "" };
    let mut helper_lines = Vec::new();
    helper_lines.push(format!("{export_kw}class {helper} {{"));
    for f in fields {
        if is_js {
            helper_lines.push(format!("    {f};"));
        } else if let Some((_, Some(ty))) = field_decls.get(f) {
            helper_lines.push(format!("    public {f}: {ty};"));
        } else {
            helper_lines.push(format!("    public {f}: any;"));
        }
    }
    helper_lines.push(String::new());
    let ctor_params = if is_js {
        fields.join(", ")
    } else {
        fields
            .iter()
            .map(|f| {
                if let Some((_, Some(ty))) = field_decls.get(f) {
                    format!("{f}: {ty}")
                } else {
                    format!("{f}: any")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    helper_lines.push(format!("    constructor({ctor_params}) {{"));
    for f in fields {
        helper_lines.push(format!("        this.{f} = {f};"));
    }
    helper_lines.push("    }".to_string());

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        let reindented = reindent(&minfo.full_text, "    ");
        let pub_reindented = if reindented.trim_start().starts_with("private ") {
            reindented.replacen("private ", "public ", 1)
        } else if reindented.trim_start().starts_with("protected ") {
            reindented.replacen("protected ", "public ", 1)
        } else if !reindented.trim_start().starts_with("public ") && !is_js {
            reindented.replacen("    ", "    public ", 1)
        } else {
            reindented
        };
        helper_lines.push(pub_reindented);
    }
    helper_lines.push("}".to_string());
    let helper_text = helper_lines.join("\n");

    // Rewrite Owner class
    let mut new_body_lines = Vec::new();
    let mut delegate_field_placed = false;
    let mut inside_ctor = false;
    let mut ctor_replaced = false;

    let helper_init_args = fields
        .iter()
        .map(|f| {
            if let Some(rhs) = ctor_assigned_fields.get(f) {
                rhs.clone()
            } else if let Some((decl, _)) = field_decls.get(f) {
                decl.split_once('=')
                    .map(|(_, rhs)| rhs.trim().trim_end_matches(';').trim().to_string())
                    .unwrap_or_else(|| {
                        if is_js {
                            "undefined".to_string()
                        } else {
                            "undefined as any".to_string()
                        }
                    })
            } else {
                if is_js {
                    "undefined".to_string()
                } else {
                    "undefined as any".to_string()
                }
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let delegate_initializer = if ctor_assigned_fields.is_empty() {
        format!(" = new {helper}({helper_init_args})")
    } else {
        String::new()
    };

    for (line_index, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        let is_moved_field_decl = fields.iter().any(|f| {
            if let Some((decl_line, _)) = field_decls.get(f) {
                decl_line.trim() == trimmed
            } else {
                false
            }
        });

        if is_moved_field_decl {
            if !delegate_field_placed {
                let vis = if is_js { "" } else { "public " };
                let type_ann = if is_js { "" } else { ": " };
                let type_name = if is_js { "" } else { helper };
                new_body_lines.push(format!(
                    "    {vis}{field}{type_ann}{type_name}{delegate_initializer};"
                ));
                delegate_field_placed = true;
            }
            continue;
        }

        if trimmed.starts_with("constructor") && trimmed.contains('{') {
            inside_ctor = true;
        }

        if inside_ctor {
            let is_moved_field_assign = fields.iter().any(|f| {
                trimmed.starts_with(&format!("this.{f} =")) || trimmed.starts_with(&format!("this.{f}="))
            });
            if is_moved_field_assign {
                if !ctor_replaced {
                    new_body_lines.push(format!("        this.{field} = new {helper}({helper_init_args});"));
                    ctor_replaced = true;
                }
                continue;
            }
            if trimmed.contains('}') {
                inside_ctor = false;
            }
        }

        let is_moved_method = moved_methods
            .values()
            .any(|minfo| minfo.start_line <= line_index && line_index <= minfo.end_line);
        if is_moved_method {
            continue;
        }

        new_body_lines.push(line.to_string());
    }

    if !delegate_field_placed {
        let vis = if is_js { "" } else { "public " };
        let type_ann = if is_js { "" } else { ": " };
        let type_name = if is_js { "" } else { helper };
        new_body_lines.insert(
            0,
            format!("    {vis}{field}{type_ann}{type_name}{delegate_initializer};"),
        );
    }

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_ts(&minfo.params);
        let ret_ann = if let Some(ret) = &minfo.ret_type {
            format!(": {ret}")
        } else {
            String::new()
        };
        let async_kw = if minfo.is_async { "async " } else { "" };
        let vis = if minfo.vis.is_empty() {
            if is_js { String::new() } else { "public ".to_string() }
        } else {
            format!("{} ", minfo.vis)
        };
        let forwarding = format!(
            "    {vis}{async_kw}{m}({}){ret_ann} {{\n        return this.{field}.{m}({});\n    }}",
            minfo.params,
            arg_names.join(", ")
        );
        new_body_lines.push(String::new());
        new_body_lines.push(forwarding);
    }

    let new_body = new_body_lines.join("\n");
    let mut out = text.to_string();
    out.replace_range(open_brace + 1..close_brace, &format!("\n{new_body}\n"));
    let owner_marker = format!("class {owner}");
    let struct_marker = format!("struct {owner}");
    let owner_start = text[..open_brace]
        .rfind(&owner_marker)
        .or_else(|| text[..open_brace].rfind(&struct_marker))
        .context("cannot locate the selected C++ declaration for helper insertion")?;
    out.insert_str(owner_start, &format!("{helper_text}\n\n"));
    let final_text = out;
    Ok((final_text, owner))
}

pub fn restructure_py(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut class_idx = None;
    let mut owner_name = String::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("class ") && trimmed.contains(':') {
            let after = &trimmed[6..];
            let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if name.is_empty() {
                continue;
            }
            if let Some(sym) = symbol_opt && sym != name {
                continue;
            }
            if let Some(l) = line_opt && l > 0 && l != (idx as u32 + 1) {
                continue;
            }
            owner_name = name;
            class_idx = Some(idx);
            break;
        }
    }

    let c_idx = class_idx.with_context(|| {
        if let Some(sym) = symbol_opt {
            format!("class `{sym}` not found")
        } else {
            "no class found at the specified location".to_string()
        }
    })?;

    let class_line = lines[c_idx];
    let class_indent = class_line.len() - class_line.trim_start().len();

    let mut end_class_idx = lines.len();
    for (idx, line) in lines.iter().enumerate().skip(c_idx + 1) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let ind = line.len() - line.trim_start().len();
        if ind <= class_indent {
            end_class_idx = idx;
            break;
        }
    }

    let mut ctor_assigned_fields: BTreeMap<String, String> = BTreeMap::new();
    let mut class_fields: BTreeMap<String, String> = BTreeMap::new();

    struct PyMethod {
        params: String,
        full_text: String,
        body: String,
        start_line: usize,
        end_line: usize,
    }

    let mut moved_methods: BTreeMap<String, PyMethod> = BTreeMap::new();
    let mut idx = c_idx + 1;

    while idx < end_class_idx {
        let line = lines[idx];
        let trimmed = line.trim_start();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let after_def = if let Some(s) = trimmed.strip_prefix("async def ") { s } else { &trimmed[4..] };
            if let Some(p_open) = after_def.find('(') {
                let mname = after_def[..p_open].trim().to_string();
                if let Some(p_close) = after_def.rfind(')') {
                    let params = after_def[p_open + 1..p_close].trim().to_string();
                    let m_indent = line.len() - trimmed.len();
                    let mut m_end = end_class_idx;
                    for (k, ml) in lines.iter().enumerate().take(end_class_idx).skip(idx + 1) {
                        if ml.trim().is_empty() || ml.trim_start().starts_with('#') {
                            continue;
                        }
                        let mind = ml.len() - ml.trim_start().len();
                        if mind <= m_indent {
                            m_end = k;
                            break;
                        }
                    }
                    let m_lines = &lines[idx..m_end];
                    let full_text = m_lines.join("\n");
                    let body_lines = if m_lines.len() > 1 { &m_lines[1..] } else { &[] };
                    let body = body_lines.join("\n");

                    if mname == "__init__" {
                        for bl in body_lines {
                            let bt = bl.trim();
                            if bt.starts_with("self.")
                                && let Some((lhs, rhs)) = bt.split_once('=')
                            {
                                let fname = lhs.trim().trim_start_matches("self.").trim();
                                if is_ident_str(fname) {
                                    ctor_assigned_fields.insert(fname.to_string(), rhs.trim().to_string());
                                }
                            }
                        }
                    } else if methods.contains(&mname) {
                        moved_methods.insert(
                            mname.clone(),
                            PyMethod {
                                params,
                                full_text,
                                body,
                                start_line: idx,
                                end_line: m_end,
                            },
                        );
                    }
                    idx = m_end;
                    continue;
                }
            }
        } else if trimmed.contains('=') || trimmed.contains(':') {
            let first = trimmed.split(&['=', ':'][..]).next().unwrap_or("").trim();
            if is_ident_str(first) {
                class_fields.insert(first.to_string(), trimmed.to_string());
            }
        }
        idx += 1;
    }

    for f in fields {
        anyhow::ensure!(
            class_fields.contains_key(f) || ctor_assigned_fields.contains_key(f) || text.contains(&format!("self.{f}")),
            "`{owner_name}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !class_fields.contains_key(field) && !ctor_assigned_fields.contains_key(field),
        "`{owner_name}` already has a field `{field}`"
    );

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner_name}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        for (pos, _) in minfo.body.match_indices("self.") {
            let after = &minfo.body[pos + 5..];
            let ident: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if !ident.is_empty() && !fields.contains(&ident) && !methods.contains(&ident) {
                anyhow::bail!("`{mname}` uses `{ident}`, which does not move to `{helper}`");
            }
        }
    }

    let mut helper_lines = Vec::new();
    helper_lines.push(format!("class {helper}:"));
    let helper_init_params = fields
        .iter()
        .map(|f| format!("{f}=None"))
        .collect::<Vec<_>>()
        .join(", ");
    helper_lines.push(format!("    def __init__(self, {helper_init_params}):"));
    for f in fields {
        helper_lines.push(format!("        self.{f} = {f}"));
    }
    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        let reindented = reindent(&minfo.full_text, "    ");
        helper_lines.push(reindented);
    }
    let helper_text = helper_lines.join("\n");

    let mut new_class_lines = Vec::new();
    let mut inside_init = false;
    let mut init_replaced = false;

    let owner_init_args = fields
        .iter()
        .map(|f| {
            if let Some(rhs) = ctor_assigned_fields.get(f) {
                rhs.clone()
            } else {
                f.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let mut k = c_idx;
    while k < end_class_idx {
        let line = lines[k];
        let trimmed = line.trim();

        if trimmed.starts_with("def __init__") {
            inside_init = true;
            new_class_lines.push(line.to_string());
            k += 1;
            continue;
        }

        if inside_init {
            if trimmed.starts_with("def ") {
                inside_init = false;
            } else {
                let is_moved_assign = fields.iter().any(|f| {
                    trimmed.starts_with(&format!("self.{f} =")) || trimmed.starts_with(&format!("self.{f}="))
                });
                if is_moved_assign {
                    if !init_replaced {
                        new_class_lines.push(format!("        self.{field} = {helper}({owner_init_args})"));
                        init_replaced = true;
                    }
                    k += 1;
                    continue;
                }
            }
        }

        let is_in_moved_method = moved_methods.values().any(|minfo| {
            minfo.start_line <= k && k < minfo.end_line
        });
        if is_in_moved_method {
            k += 1;
            continue;
        }

        let is_moved_class_field = fields.iter().any(|f| {
            trimmed.starts_with(&format!("{f} =")) || trimmed.starts_with(&format!("{f}:"))
        });
        if is_moved_class_field {
            k += 1;
            continue;
        }

        new_class_lines.push(line.to_string());
        k += 1;
    }

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_py(&minfo.params);
        let other_params = if minfo.params.is_empty() || minfo.params == "self" {
            String::new()
        } else {
            let after_self = minfo.params.strip_prefix("self").unwrap_or(&minfo.params).trim();
            if after_self.starts_with(',') {
                after_self.to_string()
            } else if !after_self.is_empty() {
                format!(", {after_self}")
            } else {
                String::new()
            }
        };
        new_class_lines.push(String::new());
        new_class_lines.push(format!("    def {m}(self{other_params}):"));
        new_class_lines.push(format!("        return self.{field}.{m}({})", arg_names.join(", ")));
    }

    let mut final_lines = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i == c_idx {
            final_lines.push(helper_text.clone());
            final_lines.push(String::new());
            final_lines.push(String::new());
            for cl in &new_class_lines {
                final_lines.push(cl.clone());
            }
        } else if i > c_idx && i < end_class_idx {
            continue;
        } else {
            final_lines.push(line.to_string());
        }
    }

    Ok((final_lines.join("\n"), owner_name))
}

pub fn restructure_cpp(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<(String, String)> {
    let (owner_start, open_brace, close_brace, owner) = {
        let mut found = None;
        for keyword in ["class ", "struct "] {
            for (i, _) in text.match_indices(keyword) {
                if i > 0 && text[..i].chars().next_back().is_some_and(is_ident) {
                    continue;
                }
                let after = &text[i + keyword.len()..];
                let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
                if name.is_empty() {
                    continue;
                }
                if let Some(sym) = symbol_opt && sym != name {
                    continue;
                }
                let Some(open_rel) = after.find('{') else { continue };
                let open = i + keyword.len() + open_rel;
                let Some(close) = crate::parameter_object::matching_bracket(text, open) else { continue };
                if let Some(l) = line_opt && l > 0 {
                    let (sl, _) = crate::signature::position_at(text, i)?;
                    let (el, _) = crate::signature::position_at(text, close)?;
                    if l < sl || l > el {
                        continue;
                    }
                }
                found = Some((i, open, close, name));
                break;
            }
            if found.is_some() {
                break;
            }
        }
        found.with_context(|| {
            if let Some(sym) = symbol_opt {
                format!("class or struct `{sym}` not found")
            } else {
                "no class/struct found at the specified location".to_string()
            }
        })?
    };

    let body = &text[open_brace + 1..close_brace];

    let mut field_decls: BTreeMap<String, String> = BTreeMap::new();
    let mut field_types: BTreeMap<String, String> = BTreeMap::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }
        if let Some(semi) = trimmed.strip_suffix(';') {
            let decl = semi.trim();
            if !decl.contains('(') {
                let parts: Vec<&str> = decl.split_whitespace().collect();
                if parts.len() >= 2 {
                    let fname = parts.last().unwrap().trim_matches(|c| !is_ident(c));
                    let ty = parts[..parts.len() - 1].join(" ");
                    if is_ident_str(fname) {
                        field_decls.insert(fname.to_string(), line.to_string());
                        field_types.insert(fname.to_string(), ty);
                    }
                }
            }
        }
    }

    for f in fields {
        anyhow::ensure!(
            field_decls.contains_key(f),
            "`{owner}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !field_decls.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    struct CppMethod {
        sig: String,
        params: String,
        full_text: String,
        body: String,
        start_line: usize,
        end_line: usize,
    }
    let mut moved_methods: BTreeMap<String, CppMethod> = BTreeMap::new();
    let mut offset = 0;
    while offset < body.len() {
        let slice = &body[offset..];
        let Some(paren_rel) = slice.find('(') else { break };
        let paren_pos = offset + paren_rel;
        let before_paren = body[offset..paren_pos].trim();
        let last_word = before_paren.split_whitespace().last().unwrap_or("");
        if is_ident_str(last_word)
            && last_word != owner
            && last_word != "if"
            && last_word != "while"
            && last_word != "for"
            && let Some(close_paren_rel) = body[paren_pos..].find(')')
        {
            let close_paren = paren_pos + close_paren_rel;
            let params = body[paren_pos + 1..close_paren].trim().to_string();
            if let Some(open_b_rel) = body[close_paren + 1..].find('{') {
                let open_b = close_paren + 1 + open_b_rel;
                if let Some(close_b_rel) = crate::parameter_object::matching_bracket(&body[open_b..], 0) {
                    let close_b = open_b + close_b_rel;
                    let line_start = body[..offset + slice[..paren_rel].rfind('\n').map_or(0, |x| x + 1)]
                        .rfind('\n')
                        .map_or(0, |x| x + 1);
                    let sig = body[line_start..open_b].trim().to_string();
                    let m_body = body[open_b + 1..close_b].to_string();
                    let full_text = body[line_start..=close_b].trim().to_string();
                    let start_line = body[..line_start].matches('\n').count();
                    let end_line = body[..close_b].matches('\n').count();
                    let mname = last_word.to_string();
                    if methods.contains(&mname) {
                        moved_methods.insert(
                            mname,
                            CppMethod {
                                sig,
                                params,
                                full_text,
                                body: m_body,
                                start_line,
                                end_line,
                            },
                        );
                    }
                    offset = close_b + 1;
                    continue;
                }
            }
        }
        offset = paren_pos + 1;
    }

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        for fname in field_decls.keys() {
            if !fields.contains(fname) {
                let direct_use = minfo.body.contains(&format!("this->{fname}")) || minfo.body.contains(fname);
                if direct_use {
                    anyhow::bail!("`{mname}` uses `{fname}`, which does not move to `{helper}`");
                }
            }
        }
    }

    let mut helper_lines = Vec::new();
    helper_lines.push(format!("class {helper} {{"));
    helper_lines.push("public:".to_string());
    for f in fields {
        let ty = field_types.get(f).map(|s| s.as_str()).unwrap_or("auto");
        helper_lines.push(format!("    {ty} {f};"));
    }
    helper_lines.push(String::new());
    helper_lines.push(format!("    {helper}() = default;"));
    let ctor_params = fields
        .iter()
        .map(|f| {
            let ty = field_types.get(f).map(|s| s.as_str()).unwrap_or("auto");
            format!("{ty} {f}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let ctor_init = fields
        .iter()
        .map(|f| format!("{f}({f})"))
        .collect::<Vec<_>>()
        .join(", ");
    helper_lines.push(format!("    {helper}({ctor_params}) : {ctor_init} {{}}"));
    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        helper_lines.push(reindent(&minfo.full_text, "    "));
    }
    helper_lines.push("};".to_string());
    let helper_text = helper_lines.join("\n");

    let mut new_body_lines = Vec::new();
    let mut delegate_placed = false;

    for (line_index, line) in body.lines().enumerate() {
        if moved_methods
            .values()
            .any(|minfo| minfo.start_line <= line_index && line_index <= minfo.end_line)
        {
            continue;
        }
        let trimmed = line.trim();
        let is_moved_field = fields.iter().any(|f| {
            if let Some(decl) = field_decls.get(f) {
                decl.trim() == trimmed
            } else {
                false
            }
        });
        if is_moved_field {
            if !delegate_placed {
                new_body_lines.push(format!("    {helper} {field};"));
                delegate_placed = true;
            }
            continue;
        }

        if trimmed.starts_with(':') || (trimmed.contains(':') && trimmed.contains('(')) {
            let mut updated_line = line.to_string();
            let mut init_args = Vec::new();
            for f in fields {
                if let Some(pos) = updated_line.find(&format!("{f}(")) {
                    let after = &updated_line[pos + f.len() + 1..];
                    if let Some(end) = after.find(')') {
                        let arg = &after[..end];
                        init_args.push(arg.to_string());
                    }
                }
            }
            if !init_args.is_empty() {
                for f in fields {
                    if let Some(pos) = updated_line.find(&format!("{f}(")) {
                        let after = &updated_line[pos + f.len() + 1..];
                        if let Some(end) = after.find(')') {
                            let full_term = &updated_line[pos..pos + f.len() + 1 + end + 1];
                            updated_line = updated_line.replace(full_term, "");
                        }
                    }
                }
                updated_line = updated_line.replace(", ,", ",");
                if let Some(colon_pos) = updated_line.find(':') {
                    let before_colon = &updated_line[..colon_pos + 1];
                    let after_colon = updated_line[colon_pos + 1..].trim();
                    let joined = init_args.join(", ");
                    if after_colon.is_empty() || after_colon.starts_with('{') {
                        updated_line = format!("{before_colon} {field}({joined}) {after_colon}");
                    } else {
                        updated_line = format!("{before_colon} {field}({joined}), {after_colon}");
                    }
                }
            }
            new_body_lines.push(updated_line);
            continue;
        }

        new_body_lines.push(line.to_string());
    }

    if !delegate_placed {
        new_body_lines.insert(0, format!("    {helper} {field};"));
    }

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_cpp(&minfo.params);
        new_body_lines.push(String::new());
        new_body_lines.push(format!("    {} {{\n        return {field}.{m}({});\n    }}", minfo.sig, arg_names.join(", ")));
    }

    let new_body = new_body_lines.join("\n");
    let mut out = text.to_string();
    out.replace_range(open_brace + 1..close_brace, &format!("\n{new_body}\n"));
    out.insert_str(owner_start, &format!("{helper_text}\n\n"));
    Ok((out, owner))
}

pub fn restructure_swift(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<(String, String)> {
    let (open_brace, close_brace, owner) = {
        let mut found = None;
        for keyword in ["struct ", "class "] {
            for (i, _) in text.match_indices(keyword) {
                if i > 0 && text[..i].chars().next_back().is_some_and(is_ident) {
                    continue;
                }
                let after = &text[i + keyword.len()..];
                let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
                if name.is_empty() {
                    continue;
                }
                if let Some(sym) = symbol_opt && sym != name {
                    continue;
                }
                let Some(open_rel) = after.find('{') else { continue };
                let open = i + keyword.len() + open_rel;
                let Some(close) = crate::parameter_object::matching_bracket(text, open) else { continue };
                if let Some(l) = line_opt && l > 0 {
                    let (sl, _) = crate::signature::position_at(text, i)?;
                    let (el, _) = crate::signature::position_at(text, close)?;
                    if l < sl || l > el {
                        continue;
                    }
                }
                found = Some((open, close, name));
                break;
            }
            if found.is_some() {
                break;
            }
        }
        found.with_context(|| {
            if let Some(sym) = symbol_opt {
                format!("struct or class `{sym}` not found")
            } else {
                "no struct/class found at specified location".to_string()
            }
        })?
    };

    let body = &text[open_brace + 1..close_brace];
    let mut prop_decls: BTreeMap<String, String> = BTreeMap::new();
    let mut prop_types: BTreeMap<String, String> = BTreeMap::new();

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }
        if trimmed.contains("var ") || trimmed.contains("let ") {
            let decl = trimmed.strip_prefix("public ").unwrap_or(trimmed);
            let after_kw = if let Some(s) = decl.strip_prefix("var ") { s } else if let Some(s) = decl.strip_prefix("let ") { s } else { continue };
            if let Some((lhs, ty_part)) = after_kw.split_once(':') {
                let pname = lhs.trim();
                let ty = ty_part.split('=').next().unwrap_or(ty_part).trim();
                if is_ident_str(pname) {
                    prop_decls.insert(pname.to_string(), line.to_string());
                    prop_types.insert(pname.to_string(), ty.to_string());
                }
            }
        }
    }

    for f in fields {
        anyhow::ensure!(
            prop_decls.contains_key(f) || body.contains(&format!("self.{f}")),
            "`{owner}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !prop_decls.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    struct SwiftMethod {
        sig: String,
        params: String,
        full_text: String,
        body: String,
        start_line: usize,
        end_line: usize,
    }

    let mut moved_methods: BTreeMap<String, SwiftMethod> = BTreeMap::new();
    let mut offset = 0;
    while offset < body.len() {
        let slice = &body[offset..];
        let Some(paren_rel) = slice.find('(') else { break };
        let paren_pos = offset + paren_rel;
        let before_paren = body[offset..paren_pos].trim();
        let last_word = before_paren.split_whitespace().last().unwrap_or("");
        if is_ident_str(last_word)
            && last_word != "init"
            && last_word != "if"
            && last_word != "while"
            && last_word != "for"
            && let Some(close_paren_rel) = body[paren_pos..].find(')')
        {
            let close_paren = paren_pos + close_paren_rel;
            let params = body[paren_pos + 1..close_paren].trim().to_string();
            if let Some(open_b_rel) = body[close_paren + 1..].find('{') {
                    let open_b = close_paren + 1 + open_b_rel;
                    if let Some(close_b_rel) = crate::parameter_object::matching_bracket(&body[open_b..], 0) {
                        let close_b = open_b + close_b_rel;
                        let line_start = body[..offset + slice[..paren_rel].rfind('\n').map_or(0, |x| x + 1)]
                            .rfind('\n')
                            .map_or(0, |x| x + 1);
                        let sig = body[line_start..open_b].trim().to_string();
                        let m_body = body[open_b + 1..close_b].to_string();
                        let full_text = body[line_start..=close_b].trim().to_string();
                        let start_line = body[..line_start].matches('\n').count();
                        let end_line = body[..close_b].matches('\n').count();
                        let mname = last_word.to_string();
                        if methods.contains(&mname) {
                            moved_methods.insert(
                                mname,
                                SwiftMethod {
                                    sig,
                                    params,
                                    full_text,
                                    body: m_body,
                                    start_line,
                                    end_line,
                                },
                            );
                        }
                        offset = close_b + 1;
                        continue;
                    }
                }
            }
        offset = paren_pos + 1;
    }

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        for (idx, _) in minfo.body.match_indices("self.") {
            let after = &minfo.body[idx + 5..];
            let ident: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if !ident.is_empty() && !fields.contains(&ident) && !methods.contains(&ident) {
                anyhow::bail!("`{mname}` uses `{ident}`, which does not move to `{helper}`");
            }
        }
    }

    let mut helper_lines = Vec::new();
    helper_lines.push(format!("public struct {helper} {{"));
    for f in fields {
        let ty = prop_types.get(f).map(|s| s.as_str()).unwrap_or("String");
        helper_lines.push(format!("    public var {f}: {ty}"));
    }
    helper_lines.push(String::new());
    let init_params = fields
        .iter()
        .map(|f| {
            let ty = prop_types.get(f).map(|s| s.as_str()).unwrap_or("String");
            format!("{f}: {ty}")
        })
        .collect::<Vec<_>>()
        .join(", ");
    helper_lines.push(format!("    public init({init_params}) {{"));
    for f in fields {
        helper_lines.push(format!("        self.{f} = {f}"));
    }
    helper_lines.push("    }".to_string());
    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        helper_lines.push(reindent(&minfo.full_text, "    "));
    }
    helper_lines.push("}".to_string());
    let helper_text = helper_lines.join("\n");

    let mut new_body_lines = Vec::new();
    let mut prop_placed = false;
    let mut inside_init = false;
    let mut init_replaced = false;

    let init_call_args = fields
        .iter()
        .map(|f| format!("{f}: {f}"))
        .collect::<Vec<_>>()
        .join(", ");

    for (line_index, line) in body.lines().enumerate() {
        if moved_methods
            .values()
            .any(|minfo| minfo.start_line <= line_index && line_index <= minfo.end_line)
        {
            continue;
        }
        let trimmed = line.trim();
        let is_moved_prop = fields.iter().any(|f| {
            if let Some(decl) = prop_decls.get(f) {
                decl.trim() == trimmed
            } else {
                false
            }
        });
        if is_moved_prop {
            if !prop_placed {
                new_body_lines.push(format!("    public var {field}: {helper}"));
                prop_placed = true;
            }
            continue;
        }

        if trimmed.contains("init(") && trimmed.contains('{') {
            inside_init = true;
        }

        if inside_init {
            let is_moved_assign = fields.iter().any(|f| {
                trimmed.starts_with(&format!("self.{f} =")) || trimmed.starts_with(&format!("self.{f}="))
            });
            if is_moved_assign {
                if !init_replaced {
                    new_body_lines.push(format!("        self.{field} = {helper}({init_call_args})"));
                    init_replaced = true;
                }
                continue;
            }
            if trimmed.contains('}') {
                inside_init = false;
            }
        }

        new_body_lines.push(line.to_string());
    }

    if !prop_placed {
        new_body_lines.insert(0, format!("    public var {field}: {helper}"));
    }

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_swift(&minfo.params);
        new_body_lines.push(String::new());
        new_body_lines.push(format!("    {} {{\n        return {field}.{m}({})\n    }}", minfo.sig, arg_names.join(", ")));
    }

    let new_body = new_body_lines.join("\n");
    let mut out = text.to_string();
    out.replace_range(open_brace + 1..close_brace, &format!("\n{new_body}\n"));
    let final_text = format!("{helper_text}\n\n{out}");
    Ok((final_text, owner))
}

pub fn restructure_go(
    text: &str,
    symbol_opt: Option<&str>,
    line_opt: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
) -> Result<(String, String)> {
    let (struct_open, struct_close, owner) = {
        let mut found = None;
        for (i, _) in text.match_indices("type ") {
            let after = &text[i + 5..];
            let name: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if name.is_empty() {
                continue;
            }
            if let Some(sym) = symbol_opt && sym != name {
                continue;
            }
            let rest = after[name.len()..].trim_start();
            if !rest.starts_with("struct") {
                continue;
            }
            let Some(open_rel) = after.find('{') else { continue };
            let open = i + 5 + open_rel;
            let Some(close) = crate::parameter_object::matching_bracket(text, open) else { continue };
            if let Some(l) = line_opt && l > 0 {
                let (sl, _) = crate::signature::position_at(text, i)?;
                let (el, _) = crate::signature::position_at(text, close)?;
                if l < sl || l > el {
                    continue;
                }
            }
            found = Some((open, close, name));
            break;
        }
        found.with_context(|| {
            if let Some(sym) = symbol_opt {
                format!("type `{sym}` struct not found")
            } else {
                "no struct found at specified location".to_string()
            }
        })?
    };

    let struct_body = &text[struct_open + 1..struct_close];
    let mut field_decls: BTreeMap<String, String> = BTreeMap::new();
    let mut field_types: BTreeMap<String, String> = BTreeMap::new();

    for line in struct_body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let fname = parts[0];
            let ftype = parts[1];
            if is_ident_str(fname) {
                field_decls.insert(fname.to_string(), line.to_string());
                field_types.insert(fname.to_string(), ftype.to_string());
            }
        }
    }

    for f in fields {
        anyhow::ensure!(
            field_decls.contains_key(f),
            "`{owner}` has no field `{f}`"
        );
    }
    anyhow::ensure!(
        !field_decls.contains_key(field),
        "`{owner}` already has a field `{field}`"
    );

    struct GoMethod {
        recv_var: String,
        recv_is_ptr: bool,
        params: String,
        ret_type: String,
        body: String,
        start: usize,
        end: usize,
    }

    let mut moved_methods: BTreeMap<String, GoMethod> = BTreeMap::new();
    let mut offset = 0;
    while offset < text.len() {
        let slice = &text[offset..];
        let Some(pos) = slice.find("func ") else { break };
        let func_pos = offset + pos;
        let after_func = &text[func_pos + 5..];
        if after_func.starts_with('(') && let Some(close_recv) = after_func.find(')') {
            let recv_slice = after_func[1..close_recv].trim();
            let recv_parts: Vec<&str> = recv_slice.split_whitespace().collect();
            if recv_parts.len() >= 2 {
                let rvar = recv_parts[0];
                let rty = recv_parts[1];
                let is_ptr = rty.starts_with('*');
                let rname = rty.trim_start_matches('*');
                if rname == owner {
                    let after_recv = after_func[close_recv + 1..].trim_start();
                    let mname: String = after_recv.chars().take_while(|c| is_ident(*c)).collect();
                    if !mname.is_empty()
                        && methods.contains(&mname)
                        && let Some(p_open) = after_recv.find('(')
                        && let Some(p_close) = after_recv[p_open..].find(')')
                    {
                        let params = after_recv[p_open + 1..p_open + p_close].trim().to_string();
                        let after_params = after_recv[p_open + p_close + 1..].trim_start();
                        if let Some(b_open_rel) = after_params.find('{') {
                            let b_open = func_pos + 5 + close_recv + 1 + (after_func[close_recv + 1..].len() - after_params.len()) + b_open_rel;
                            let ret_type = after_params[..b_open_rel].trim().to_string();
                            if let Some(b_close_rel) = crate::parameter_object::matching_bracket(text, b_open) {
                                let body = text[b_open + 1..b_close_rel].to_string();
                                moved_methods.insert(
                                    mname,
                                    GoMethod {
                                        recv_var: rvar.to_string(),
                                        recv_is_ptr: is_ptr,
                                        params,
                                        ret_type,
                                        body,
                                        start: func_pos,
                                        end: b_close_rel + 1,
                                    },
                                );
                                offset = b_close_rel + 1;
                                continue;
                            }
                        }
                    }
                }
            }
        }
        offset = func_pos + 5;
    }

    for m in methods {
        anyhow::ensure!(
            moved_methods.contains_key(m),
            "`{owner}` has no method `{m}`"
        );
    }

    for (mname, minfo) in &moved_methods {
        let needle = format!("{}.", minfo.recv_var);
        for (pos, _) in minfo.body.match_indices(&needle) {
            let after = &minfo.body[pos + needle.len()..];
            let ident: String = after.chars().take_while(|c| is_ident(*c)).collect();
            if !ident.is_empty() && !fields.contains(&ident) && !methods.contains(&ident) {
                anyhow::bail!("`{mname}` uses `{ident}`, which does not move to `{helper}`");
            }
        }
    }

    let mut helper_lines = Vec::new();
    helper_lines.push(format!("type {helper} struct {{"));
    for f in fields {
        let ty = field_types.get(f).map(|s| s.as_str()).unwrap_or("string");
        helper_lines.push(format!("    {f} {ty}"));
    }
    helper_lines.push("}".to_string());

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        helper_lines.push(String::new());
        let r_ptr = if minfo.recv_is_ptr { "*" } else { "" };
        let ret_sp = if minfo.ret_type.is_empty() { String::new() } else { format!(" {}", minfo.ret_type) };
        let helper_method = format!(
            "func ({} {r_ptr}{helper}) {m}({}){ret_sp} {{\n{}\n}}",
            minfo.recv_var,
            minfo.params,
            minfo.body.trim()
        );
        helper_lines.push(helper_method);
    }
    let helper_text = helper_lines.join("\n");

    let mut out = text.to_string();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    for m in methods {
        let minfo = moved_methods.get(m).unwrap();
        let arg_names = extract_param_names_go(&minfo.params);
        let r_ptr = if minfo.recv_is_ptr { "*" } else { "" };
        let ret_sp = if minfo.ret_type.is_empty() { String::new() } else { format!(" {}", minfo.ret_type) };
        let ret_kw = if minfo.ret_type.is_empty() { "" } else { "return " };
        let forwarding = format!(
            "func ({} {r_ptr}{owner}) {m}({}){ret_sp} {{\n    {ret_kw}{}.{field}.{m}({})\n}}",
            minfo.recv_var,
            minfo.params,
            minfo.recv_var,
            arg_names.join(", ")
        );
        edits.push((minfo.start, minfo.end, forwarding));
    }

    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (s, e, repl) in edits {
        out.replace_range(s..e, &repl);
    }

    let new_struct_open = out.find(&format!("type {owner} struct")).context("cannot re-find owner struct")?;
    let b_open = out[new_struct_open..].find('{').map(|x| new_struct_open + x).unwrap();
    let b_close = crate::parameter_object::matching_bracket(&out, b_open).unwrap();
    let cur_struct_body = &out[b_open + 1..b_close];

    let mut new_struct_body_lines = Vec::new();
    let mut field_placed = false;

    for line in cur_struct_body.lines() {
        let trimmed = line.trim();
        let is_moved = fields.iter().any(|f| {
            if let Some(decl) = field_decls.get(f) {
                decl.trim() == trimmed
            } else {
                false
            }
        });
        if is_moved {
            if !field_placed {
                new_struct_body_lines.push(format!("    {field} {helper}"));
                field_placed = true;
            }
            continue;
        }
        new_struct_body_lines.push(line.to_string());
    }

    if !field_placed {
        new_struct_body_lines.insert(0, format!("    {field} {helper}"));
    }

    out.replace_range(b_open + 1..b_close, &format!("\n{}\n", new_struct_body_lines.join("\n")));

    let final_pos = out.find(&format!("type {owner} struct")).unwrap();
    out.insert_str(final_pos, &format!("{helper_text}\n\n"));
    Ok((out, owner))
}

fn rewrite_go_literals(
    text: &str,
    owner: &str,
    fields: &[String],
    field: &str,
    helper: &str,
) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (i, _) in text.match_indices(owner) {
        let before = &text[..i];
        if before.chars().next_back().is_some_and(is_ident)
            || text[i + owner.len()..].chars().next().is_some_and(is_ident)
            || before.trim_end().ends_with("type")
        {
            continue;
        }
        let rest = &text[i + owner.len()..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('{') {
            continue;
        }
        let open = i + owner.len() + (rest.len() - trimmed.len());
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let body = &text[open + 1..close];
        let entries: Vec<&str> = split_top(body, ',')
            .into_iter()
            .map(|(s, e)| body[s..e].trim())
            .filter(|e| !e.is_empty())
            .collect();
        let key = |e: &str| -> String { e.split(':').next().unwrap_or(e).trim().to_string() };
        let inner: Vec<&str> = entries
            .iter()
            .copied()
            .filter(|e| fields.contains(&key(e)))
            .collect();
        if inner.is_empty() {
            continue;
        }
        let indent: String = body
            .trim_start_matches(['\n'])
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let nested = format!("{field}: {helper}{{ {} }}", inner.join(", "));
        let mut all: Vec<String> = Vec::new();
        let mut placed = false;
        for e in &entries {
            if fields.contains(&key(e)) {
                if !placed {
                    all.push(nested.clone());
                    placed = true;
                }
            } else {
                all.push(e.to_string());
            }
        }
        let new_body = if body.contains('\n') {
            let close_indent: String = text[..close]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            format!(
                "\n{indent}{},\n{close_indent}",
                all.join(&format!(",\n{indent}"))
            )
        } else {
            format!(" {} ", all.join(", "))
        };
        edits.push((open + 1, close, new_body));
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_string();
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    out
}

pub fn rewrite_external_file(
    code: &str,
    lang: Language,
    owner: &str,
    field: &str,
    fields: &[String],
    helper: &str,
) -> (String, usize) {
    let code_transformed;
    let code = if lang == Language::Go {
        code_transformed = rewrite_go_literals(code, owner, fields, field, helper);
        &code_transformed
    } else {
        code
    };
    let mut out = String::new();
    let mut accesses = 0;

    for line in code.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') || trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if trimmed.starts_with("import ") || trimmed.starts_with("from ") || trimmed.starts_with("package ") {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let mut current_line = line.to_string();
        for f in fields {
            let needle = format!(".{f}");
            if current_line.contains(&needle) {
                let mut new_line = String::new();
                let mut rest = current_line.as_str();
                while let Some(pos) = rest.find(&needle) {
                    let before = &rest[..pos];
                    let after = &rest[pos + needle.len()..];
                    let before_trimmed = before.trim_end();
                    let is_inside_helper = before_trimmed.ends_with(helper)
                        || (before_trimmed.ends_with("this") && current_line.contains(&format!("class {helper}")))
                        || (before_trimmed.ends_with("self") && current_line.contains(&format!("class {helper}")));
                    let is_method_call = after.trim_start().starts_with('(');
                    let is_ident_continuation = after.chars().next().is_some_and(is_ident);

                    if is_inside_helper || is_method_call || is_ident_continuation {
                        new_line.push_str(&rest[..pos + needle.len()]);
                        rest = after;
                    } else {
                        new_line.push_str(before);
                        new_line.push_str(&format!(".{field}.{f}"));
                        accesses += 1;
                        rest = after;
                    }
                }
                new_line.push_str(rest);
                current_line = new_line;
            }

            if lang == Language::Cpp || lang == Language::C {
                let arrow_needle = format!("->{f}");
                if current_line.contains(&arrow_needle) {
                    let mut new_line = String::new();
                    let mut rest = current_line.as_str();
                    while let Some(pos) = rest.find(&arrow_needle) {
                        let before = &rest[..pos];
                        let after = &rest[pos + arrow_needle.len()..];
                        let is_method_call = after.trim_start().starts_with('(');
                        let is_ident_continuation = after.chars().next().is_some_and(is_ident);

                        if is_method_call || is_ident_continuation {
                            new_line.push_str(&rest[..pos + arrow_needle.len()]);
                            rest = after;
                        } else {
                            new_line.push_str(before);
                            new_line.push_str(&format!("->{field}.{f}"));
                            accesses += 1;
                            rest = after;
                        }
                    }
                    new_line.push_str(rest);
                    current_line = new_line;
                }
            }
        }

        out.push_str(&current_line);
        out.push('\n');
    }

    if !code.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    (out, accesses)
}

fn owner_region<'a>(text: &'a str, lang: Language, owner: &str) -> Option<&'a str> {
    if lang == Language::Go {
        return Some(text);
    }
    let markers = match lang {
        Language::TypeScript | Language::JavaScript | Language::Python => {
            vec![format!("class {owner}")]
        }
        Language::Cpp | Language::C => {
            vec![format!("class {owner}"), format!("struct {owner}")]
        }
        Language::Swift => vec![
            format!("class {owner}"),
            format!("struct {owner}"),
            format!("actor {owner}"),
        ],
        Language::Go => return Some(text),
        Language::Rust | Language::Java => return None,
    };
    let start = markers
        .iter()
        .filter_map(|marker| text.rfind(marker))
        .max()?;
    if lang == Language::Python {
        return Some(&text[start..]);
    }
    let open = start + text[start..].find('{')?;
    let close = crate::parameter_object::matching_bracket(text, open)?;
    Some(&text[open + 1..close])
}

/// Extracts `fields` and `methods` of the struct at `line`:`col` of `file` into `helper`, held
/// in the new field `field` for Rust.
#[allow(clippy::too_many_arguments)]
pub async fn extract_delegate_rust(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    apply: bool,
    force: bool,
) -> Result<Extracted> {
    for n in [helper, field] {
        anyhow::ensure!(
            !n.is_empty() && n.chars().all(is_ident),
            "`{n}` is not an identifier"
        );
    }
    anyhow::ensure!(!fields.is_empty(), "name at least one field");
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = if let (Some(l), Some(c)) = (line, col) && l > 0 && c > 0 {
        crate::signature::offset_of(&text, l, c).context("the position is not in the file")?
    } else if let Some(sym) = symbol {
        let needle = format!("struct {sym}");
        text.find(&needle)
            .with_context(|| format!("struct `{sym}` not found in {}", file.display()))?
    } else {
        anyhow::bail!("provide either line and character or symbol");
    };
    let decl = parse_struct(&text, at)?;
    // Check the struct side first, so a wrong name fails before any analyzer query.
    restructure(&text, at, fields, methods, helper, field)?;

    // The moved methods keep `self.city`; every other access gets the new field in front.
    let moved_ranges: Vec<(usize, usize)> = impl_blocks(&text, &decl.name)
        .into_iter()
        .flat_map(|(_, open, close)| crate::extract_trait::items(&text, open, close))
        .filter(|i| i.name.as_ref().is_some_and(|n| methods.contains(n)))
        .map(|i| (i.start, i.end))
        .collect();
    let mut inserts: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
    let body = &text[decl.open + 1..decl.close];
    for f in fields {
        let Some(rel_at) = body.match_indices(f.as_str()).map(|(i, _)| i).find(|i| {
            !body[..*i].chars().next_back().is_some_and(is_ident)
                && body[i + f.len()..].trim_start().starts_with(':')
        }) else {
            continue;
        };
        let (fl, fc) = crate::signature::position_at(&text, decl.open + 1 + rel_at)?;
        let refs = crate::signature::references(remote, root, file, fl, fc)
            .await
            .with_context(|| format!("cannot find the uses of `{f}`; nothing was planned"))?;
        for (path, rl, rc) in refs {
            let other = if path == file {
                text.clone()
            } else {
                std::fs::read_to_string(&path).with_context(|| {
                    format!(
                        "cannot read {}, where the analyzer reports a use of `{f}`; nothing was \
                         planned",
                        path.display()
                    )
                })?
            };
            // An access passed over would still name a field the struct no longer has (#446).
            let off = crate::signature::offset_of(&other, rl, rc).with_context(|| {
                format!(
                    "the analyzer places a use of `{f}` at {}:{rl}:{rc}, which is not in the \
                     file; nothing was planned",
                    path.display()
                )
            })?;
            // A stale position names something else, and a prefix there would break it.
            anyhow::ensure!(
                other[off..].starts_with(f.as_str())
                    && !other[off + f.len()..].starts_with(is_ident),
                "the analyzer places a use of `{f}` at {}:{rl}:{rc}, but the file says otherwise; \
                 nothing was planned",
                path.display()
            );
            if path == file && moved_ranges.iter().any(|(s, e)| *s <= off && off < *e) {
                continue;
            }
            // A field access (`a.city`); a literal or pattern entry is left to the literal pass.
            if other[..off].ends_with('.') {
                inserts.entry(path).or_default().push(off);
            }
        }
    }
    let accesses: usize = inserts.values().map(|v| v.len()).sum();
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    files.insert(file.to_path_buf(), text.clone());
    for (path, mut offs) in inserts {
        let mut t = match files.get(&path) {
            Some(t) => t.clone(),
            None => std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?,
        };
        offs.sort_unstable();
        offs.dedup();
        for off in offs.into_iter().rev() {
            t.insert_str(off, &format!("{field}."));
        }
        files.insert(path, t);
    }
    // The declaring file: the struct, the helper and the methods.
    let declaring = files.get(file).cloned().unwrap_or_default();
    let at_now = declaring
        .find(&format!("struct {} ", decl.name))
        .unwrap_or(at);
    let restructured = restructure(&declaring, at_now, fields, methods, helper, field)?;
    files.insert(file.to_path_buf(), restructured);
    // Struct literals, in every file that touches the fields.
    let paths: Vec<PathBuf> = files.keys().cloned().collect();
    for path in paths {
        let t = files[&path].clone();
        let self_ranges: Vec<(usize, usize)> = impl_blocks(&t, &decl.name)
            .into_iter()
            .map(|(s, _, e)| (s, e))
            .collect();
        let rewritten = rewrite_literals(&t, &decl.name, &self_ranges, fields, field, helper)?;
        files.insert(path, rewritten);
    }
    files.retain(|p, t| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true));

    let edits: Vec<(PathBuf, String)> = files.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
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
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }
    Ok(Extracted {
        helper: helper.to_string(),
        field: field.to_string(),
        fields: fields.to_vec(),
        methods: methods.to_vec(),
        root: root.to_path_buf(),
        rewritten: files
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        accesses,
        unmatched: Vec::new(),
        diagnostics,
        applied,
    })
}

/// Extracts `fields` and `methods` of a struct or class across languages into `helper`, held
/// in the new field `field`.
#[allow(clippy::too_many_arguments)]
pub async fn extract_delegate_polyglot(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    apply: bool,
    force: bool,
    _verify: Option<&str>,
) -> Result<Extracted> {
    for n in [helper, field] {
        anyhow::ensure!(
            !n.is_empty() && n.chars().all(is_ident),
            "`{n}` is not an identifier"
        );
    }
    anyhow::ensure!(!fields.is_empty(), "name at least one field");

    let lang = Language::of(file).with_context(|| format!("unsupported language for {}", file.display()))?;
    if lang == Language::Rust {
        return extract_delegate_rust(
            remote,
            root,
            file,
            symbol,
            line,
            col,
            fields,
            methods,
            helper,
            field,
            apply,
            force,
        )
        .await;
    }
    if lang == Language::Java {
        anyhow::bail!("extract_delegate does not support Java yet");
    }

    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read {}", file.display()))?;

    let (restructured, owner) = match lang {
        Language::TypeScript => restructure_ts(&text, symbol, line, fields, methods, helper, field, false)?,
        Language::JavaScript => restructure_ts(&text, symbol, line, fields, methods, helper, field, true)?,
        Language::Python => restructure_py(&text, symbol, line, fields, methods, helper, field)?,
        Language::Cpp | Language::C => restructure_cpp(&text, symbol, line, fields, methods, helper, field)?,
        Language::Swift => restructure_swift(&text, symbol, line, fields, methods, helper, field)?,
        Language::Go => restructure_go(&text, symbol, line, fields, methods, helper, field)?,
        Language::Rust | Language::Java => unreachable!(),
    };

    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut accesses = 0;
    let mut unmatched = Vec::new();
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let canonical_owner_file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let mut references_by_file_line: BTreeMap<PathBuf, BTreeMap<u32, usize>> = BTreeMap::new();
    for moved_field in fields {
        let position = match crate::encapsulate_field::field_position_in_lsp(
            remote,
            root,
            file,
            &owner,
            moved_field,
        )
        .await
        {
            Ok(position) => position,
            Err(err) => {
                unmatched.push(format!(
                    "{}: references for moved field `{moved_field}` could not be resolved: {err:#}",
                    display(root, file)
                ));
                continue;
            }
        };
        let Some((line, col)) = position else {
            continue;
        };
        match crate::signature::references(remote, root, file, line, col).await {
            Ok(references) => {
                for (path, line, _) in references {
                    let path = std::fs::canonicalize(&path).unwrap_or(path);
                    if !path.starts_with(&canonical_root) || Language::of(&path) != Some(lang) {
                        unmatched.push(format!(
                            "{}: moved-field reference is outside the supported workspace language",
                            display(root, &path)
                        ));
                        continue;
                    }
                    if path != canonical_owner_file {
                        *references_by_file_line
                            .entry(path)
                            .or_default()
                            .entry(line)
                            .or_default() += 1;
                    }
                }
            }
            Err(err) => unmatched.push(format!(
                "{}: references for moved field `{moved_field}` could not be resolved: {err:#}",
                display(root, file)
            )),
        }
    }

    let owner_body = owner_region(&restructured, lang, &owner).unwrap_or(&restructured);
    let (owner_probe, _) = rewrite_external_file(owner_body, lang, &owner, field, fields, helper);
    if owner_probe != owner_body {
        unmatched.push(format!(
            "{}: remaining owner methods contain moved-field accesses that need semantic resolution",
            display(root, file)
        ));
    }
    if matches!(lang, Language::Cpp | Language::C)
        && fields.iter().any(|moved| owner_body.contains(moved))
    {
        unmatched.push(format!(
            "{}: unqualified moved-field uses need semantic resolution",
            display(root, file)
        ));
    }
    files.insert(file.to_path_buf(), restructured);

    let mut processed = std::collections::BTreeSet::new();
    for entry in ignore::WalkBuilder::new(root).build().flatten() {
        let path = entry.path();
        if path.is_file() && path != file && Language::of(path) == Some(lang)
            && let Ok(content) = std::fs::read_to_string(path)
                && fields.iter().any(|f| content.contains(f)) {
                let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                processed.insert(canonical.clone());
                if lang == Language::Go {
                    let (rewritten, _) = rewrite_external_file(&content, lang, &owner, field, fields, helper);
                    if rewritten != content {
                        unmatched.push(format!(
                            "{}: Go literals need package-aware semantic resolution before they can be rewritten",
                            display(root, path)
                        ));
                    }
                    continue;
                }
                let refs = references_by_file_line.get(&canonical);
                let mut rewritten = String::with_capacity(content.len());
                let mut rewrite_failed = false;
                let mut file_accesses = 0usize;
                for (index, raw_line) in content.split_inclusive('\n').enumerate() {
                    let line_number = index as u32 + 1;
                    let expected = refs.and_then(|by_line| by_line.get(&line_number)).copied().unwrap_or(0);
                    let (body, ending) = if let Some(body) = raw_line.strip_suffix("\r\n") {
                        (body, "\r\n")
                    } else if let Some(body) = raw_line.strip_suffix('\n') {
                        (body, "\n")
                    } else {
                        (raw_line, "")
                    };
                    let (line, actual) = rewrite_external_file(body, lang, &owner, field, fields, helper);
                    if actual != expected {
                        if actual > 0 || expected > 0 {
                            unmatched.push(format!(
                                "{}:{}: {actual} textual moved-field access(es) do not match {expected} analyzer reference(s)",
                                display(root, path),
                                line_number
                            ));
                            rewrite_failed = true;
                            rewritten.push_str(raw_line);
                            continue;
                        }
                    }
                    file_accesses += actual;
                    rewritten.push_str(&line);
                    rewritten.push_str(ending);
                }
                if refs.is_some_and(|by_line| by_line.keys().any(|line| *line as usize > content.lines().count())) {
                    unmatched.push(format!(
                        "{}: analyzer reference points beyond end of file",
                        display(root, path)
                    ));
                    rewrite_failed = true;
                }
                if rewrite_failed {
                    continue;
                }
                if rewritten != content {
                    accesses += file_accesses;
                    files.insert(path.to_path_buf(), rewritten);
                } else if file_accesses > 0 {
                    accesses += file_accesses;
                }
            }
    }
    for path in references_by_file_line.keys() {
        if !processed.contains(path) {
                    unmatched.push(format!(
                "{}: analyzer reference did not resolve to a readable workspace source file",
                display(root, path)
                    ));
        }
    }

    files.retain(|p, t| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true));

    let edits: Vec<(PathBuf, String)> = files.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
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
            unmatched.is_empty(),
            "{} reference(s) cannot be safely matched to `{owner}`; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }

    Ok(Extracted {
        helper: helper.to_string(),
        field: field.to_string(),
        fields: fields.to_vec(),
        methods: methods.to_vec(),
        root: root.to_path_buf(),
        rewritten: files
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        accesses,
        unmatched,
        diagnostics,
        applied,
    })
}

/// Backwards-compatible entry point for extract_delegate.
#[allow(clippy::too_many_arguments)]
pub async fn extract_delegate(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    apply: bool,
    force: bool,
) -> Result<Extracted> {
    extract_delegate_polyglot(
        remote,
        root,
        file,
        None,
        Some(line),
        Some(col),
        fields,
        methods,
        helper,
        field,
        apply,
        force,
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "#[derive(Debug, Clone)]\npub struct Account {\n    pub owner: String,\n    pub street: String,\n    pub city: String,\n    balance: u64,\n}\n\nimpl Account {\n    pub fn new(owner: &str, street: &str, city: &str) -> Self {\n        Account {\n            owner: owner.into(),\n            street: street.into(),\n            city: city.into(),\n            balance: 0,\n        }\n    }\n\n    pub fn address(&self) -> String {\n        format!(\"{}, {}\", self.street, self.city)\n    }\n\n    pub fn moves_to(&mut self, city: &str) {\n        self.city = city.to_string();\n    }\n\n    pub fn deposit(&mut self, n: u64) {\n        self.balance += n;\n    }\n}\n";

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_struct_and_its_fields_are_read_with_their_attributes() {
        let d = parse_struct(ACCOUNT, ACCOUNT.find("pub struct").unwrap()).unwrap();
        assert_eq!(d.name, "Account");
        assert_eq!(d.derive.as_deref(), Some("#[derive(Debug, Clone)]"));
        let names: Vec<&str> = d.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["owner", "street", "city", "balance"]);
        assert_eq!(split_top("a: HashMap<K, V>, b: (u8, u8)", ',').len(), 2);
        assert_eq!(
            argument_names("pub fn f(&mut self, a: u32, mut b: Vec<(u8, u8)>) -> u8"),
            Some(strings(&["a", "b"]))
        );
        assert_eq!(argument_names("fn g(&self, (a, b): (u8, u8))"), None);
    }

    #[test]
    fn the_fields_and_methods_move_behind_a_forwarding_method() {
        let at = ACCOUNT.find("pub struct").unwrap();
        let fields = strings(&["street", "city"]);
        let methods = strings(&["address", "moves_to"]);
        let out = restructure(ACCOUNT, at, &fields, &methods, "Address", "address").unwrap();
        for expected in [
            "pub struct Account {\n    pub owner: String,\n    pub address: Address,\n    balance: u64,\n}",
            "#[derive(Debug, Clone)]\npub struct Address {\n    pub street: String,\n    pub city: String,\n}",
            "impl Address {\n    pub fn address(&self) -> String {\n        format!(\"{}, {}\", self.street, self.city)\n    }",
            "    pub fn moves_to(&mut self, city: &str) {\n        self.address.moves_to(city)\n    }",
            "    pub fn deposit(&mut self, n: u64) {\n        self.balance += n;\n    }",
        ] {
            assert!(out.contains(expected), "{expected}\n---\n{out}");
        }
        let blocks = impl_blocks(&out, "Account");
        let ranges: Vec<(usize, usize)> = blocks.iter().map(|(s, _, e)| (*s, *e)).collect();
        let lit =
            rewrite_literals(&out, "Account", &ranges, &fields, "address", "Address").unwrap();
        assert!(
            lit.contains("            owner: owner.into(),\n            address: Address { street: street.into(), city: city.into() },\n            balance: 0,\n"),
            "{lit}"
        );

        // A moved method that uses a field that stays is refused, and so is an unknown name.
        let err = restructure(
            ACCOUNT,
            at,
            &fields,
            &strings(&["deposit"]),
            "Address",
            "address",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("uses `self.balance`"), "{err}");
        let err =
            restructure(ACCOUNT, at, &strings(&["zip"]), &[], "Address", "address").unwrap_err();
        assert!(format!("{err}").contains("has no field `zip`"), "{err}");
        let err = rewrite_literals(
            "fn f() { let Account { city, .. } = a; }",
            "Account",
            &[],
            &fields,
            "address",
            "Address",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("uses `..`"), "{err}");

        // A tuple or generic struct, and a method that does not borrow its receiver.
        for text in ["pub struct P(u8);\n", "pub struct G<T> {\n    t: T,\n}\n"] {
            let err = parse_struct(text, 0).unwrap_err();
            assert!(format!("{err}").contains("not a plain struct"), "{err}");
        }
        let by_value = ACCOUNT.replace(
            "    pub fn moves_to(&mut self, city: &str) {",
            "    pub fn moves_to(mut self, city: &str) {",
        );
        let err = restructure(&by_value, at, &fields, &methods, "Address", "address").unwrap_err();
        assert!(format!("{err}").contains("does not take `&self`"), "{err}");
    }

    #[test]
    fn cpp_delegate_helper_is_inserted_after_existing_includes() {
        let source = "#include <string>\n\nclass Account {\npublic:\n    std::string city;\n};\n";
        let (rewritten, _) = restructure_cpp(
            source,
            Some("Account"),
            None,
            &strings(&["city"]),
            &[],
            "Location",
            "location",
        )
        .unwrap();
        let include = rewritten.find("#include <string>").unwrap();
        let helper = rewritten.find("class Location").unwrap();
        let owner = rewritten.find("class Account").unwrap();
        assert!(include < helper && helper < owner, "{rewritten}");
    }
}
