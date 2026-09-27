//! Extracting a trait from a chosen subset of an inherent `impl`'s methods.
//!
//! rust-analyzer's `generate_trait_from_impl` takes every method of the block, names the trait
//! `NewTrait`, keeps it private and leaves every caller in another module without the trait in
//! scope, so each call there stops compiling. This moves only the methods named, gives the trait
//! the widest visibility among them, imports it into every file that calls one of them, and
//! type-checks the whole change before anything is written.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// What the change did, or would do if it were applied.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Extracted {
    pub trait_name: String,
    pub type_name: String,
    #[serde(skip)]
    pub root: PathBuf,
    pub file: String,
    pub methods: Vec<String>,
    pub kept: Vec<String>,
    pub imports: Vec<(String, String)>,
    pub rewritten: Vec<(String, String)>,
    pub diagnostics: Vec<String>,
    pub applied: bool,
}

impl Extracted {
    pub fn render(&self) -> String {
        let mut out = format!(
            "`trait {}` for `{}` ({})\n\n- into the trait: {}\n",
            self.trait_name,
            self.type_name,
            self.file,
            self.methods.join(", ")
        );
        if !self.kept.is_empty() {
            out.push_str(&format!("- still inherent: {}\n", self.kept.join(", ")));
        }
        if !self.imports.is_empty() {
            out.push_str("imports:\n");
            for (file, line) in &self.imports {
                out.push_str(&format!("  {file}: added `{line}`\n"));
            }
        }
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
    unicode_ident::is_xid_continue(c) || c == '_'
}

fn valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name != "_"
        && (unicode_ident::is_xid_start(first) || first == '_')
        && chars.all(is_ident)
        && !matches!(
            name,
            "Self"
                | "abstract"
                | "as"
                | "async"
                | "await"
                | "become"
                | "box"
                | "break"
                | "const"
                | "continue"
                | "crate"
                | "do"
                | "dyn"
                | "else"
                | "enum"
                | "extern"
                | "false"
                | "final"
                | "fn"
                | "for"
                | "gen"
                | "if"
                | "impl"
                | "in"
                | "let"
                | "loop"
                | "macro"
                | "match"
                | "mod"
                | "move"
                | "mut"
                | "override"
                | "priv"
                | "pub"
                | "ref"
                | "return"
                | "self"
                | "static"
                | "struct"
                | "super"
                | "trait"
                | "true"
                | "try"
                | "type"
                | "typeof"
                | "union"
                | "unsafe"
                | "unsized"
                | "use"
                | "virtual"
                | "where"
                | "while"
                | "yield"
        )
}

/// One item of an `impl` block: its text runs from `start` (its comments, docs and attributes
/// included) to `end`, exclusive.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub start: usize,
    pub end: usize,
    /// The function's name, when the item is one.
    pub name: Option<String>,
}

/// An inherent `impl Type { … }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct ImplBlock {
    /// Where the `impl` keyword starts. Text before it is never replaced.
    pub start: usize,
    pub open: usize,
    pub close: usize,
    /// The declaration between `impl` and the self type, including `<` and `>`.
    pub generics: String,
    /// The declared lifetime/type/const parameter names, ready for a type argument list.
    pub generic_args: Vec<String>,
    pub self_ty: String,
    /// The original header after `impl`, without surrounding whitespace.
    pub header: String,
    /// The original `where` clause, when present.
    pub where_clause: String,
    /// Whether the original header put the `where` clause on another line.
    pub where_on_newline: bool,
    pub items: Vec<Item>,
}

fn matching_angle(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut previous = None;
    for (i, c) in text.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if previous != Some('-') && depth == 1 => return Some(i),
            '>' if previous != Some('-') && depth > 1 => depth -= 1,
            _ => {}
        }
        previous = Some(c);
    }
    None
}

fn top_level_word(text: &str, wanted: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => angle += 1,
            b'>' if angle > 0 && bytes.get(i.wrapping_sub(1)) != Some(&b'-') => angle -= 1,
            b'(' => paren += 1,
            b')' if paren > 0 => paren -= 1,
            b'[' => square += 1,
            b']' if square > 0 => square -= 1,
            _ => {}
        }
        if text.is_char_boundary(i)
            && angle == 0
            && paren == 0
            && square == 0
            && text[i..].starts_with(wanted)
            && !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + wanted.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn split_top_level(text: &str) -> Result<Vec<&str>> {
    let mut parts = Vec::new();
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut brace = 0i32;
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '<' => angle += 1,
            '>' if !text[..i].ends_with('-') => angle -= 1,
            '(' => paren += 1,
            ')' => paren -= 1,
            '[' => square += 1,
            ']' => square -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            ',' if angle == 0 && paren == 0 && square == 0 && brace == 0 => {
                parts.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        anyhow::ensure!(
            angle >= 0 && paren >= 0 && square >= 0 && brace >= 0,
            "unbalanced generic parameter declaration"
        );
    }
    anyhow::ensure!(
        angle == 0 && paren == 0 && square == 0 && brace == 0,
        "unbalanced generic parameter declaration"
    );
    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    Ok(parts)
}

fn has_top_level_equals(text: &str) -> bool {
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut brace = 0i32;
    for (i, c) in text.char_indices() {
        match c {
            '<' => angle += 1,
            '>' if !text[..i].ends_with('-') && angle > 0 => angle -= 1,
            '(' => paren += 1,
            ')' if paren > 0 => paren -= 1,
            '[' => square += 1,
            ']' if square > 0 => square -= 1,
            '{' => brace += 1,
            '}' if brace > 0 => brace -= 1,
            '=' if angle == 0 && paren == 0 && square == 0 && brace == 0 => return true,
            _ => {}
        }
    }
    false
}

fn generic_arguments(generics: &str) -> Result<Vec<String>> {
    if generics.is_empty() {
        return Ok(Vec::new());
    }
    let inner = generics
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .context("malformed generic parameter declaration")?;
    let mut names = Vec::new();
    for parameter in split_top_level(inner)? {
        anyhow::ensure!(
            !parameter.contains('#'),
            "attributes on generic parameters are not supported"
        );
        anyhow::ensure!(
            !parameter.contains('!'),
            "macros in generic parameters are not supported"
        );
        anyhow::ensure!(
            !has_top_level_equals(parameter),
            "generic parameter defaults are not valid on an inherent impl"
        );
        let name = if let Some(lifetime) = parameter.strip_prefix('\'') {
            let name: String = lifetime.chars().take_while(|c| is_ident(*c)).collect();
            let rest = lifetime[name.len()..].trim_start();
            anyhow::ensure!(
                valid_ident(&name) && name != "_" && (rest.is_empty() || rest.starts_with(':')),
                "`{parameter}` has a malformed lifetime parameter"
            );
            format!("'{name}")
        } else if let Some(constant) = parameter.strip_prefix("const ") {
            let name: String = constant
                .trim_start()
                .chars()
                .take_while(|c| is_ident(*c))
                .collect();
            let rest = constant.trim_start()[name.len()..].trim_start();
            anyhow::ensure!(
                valid_ident(&name) && rest.starts_with(':'),
                "`{parameter}` has a malformed const parameter"
            );
            name
        } else {
            let name: String = parameter.chars().take_while(|c| is_ident(*c)).collect();
            let rest = parameter[name.len()..].trim_start();
            anyhow::ensure!(
                valid_ident(&name) && (rest.is_empty() || rest.starts_with(':')),
                "`{parameter}` has a malformed type parameter"
            );
            name
        };
        names.push(name);
    }
    anyhow::ensure!(!names.is_empty(), "the generic parameter list is empty");
    Ok(names)
}

fn impl_body_open(text: &str, impl_at: usize) -> Result<usize> {
    let bytes = text.as_bytes();
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut i = impl_at + 4;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => angle += 1,
            b'>' if angle > 0 && bytes.get(i.wrapping_sub(1)) != Some(&b'-') => angle -= 1,
            b'(' => paren += 1,
            b')' if paren > 0 => paren -= 1,
            b'[' => square += 1,
            b']' if square > 0 => square -= 1,
            b'{' if angle == 0 && paren == 0 && square == 0 => return Ok(i),
            b';' if angle == 0 && paren == 0 && square == 0 => break,
            _ => {}
        }
        i += 1;
    }
    anyhow::bail!("the `impl` has no body")
}

fn inside_macro(text: &str, at: usize) -> bool {
    let mut opens = Vec::new();
    for (i, byte) in text[..at].bytes().enumerate() {
        match byte {
            b'{' => opens.push(i),
            b'}' => {
                opens.pop();
            }
            _ => {}
        }
    }
    opens.into_iter().any(|open| {
        let line_start = text[..open].rfind('\n').map_or(0, |i| i + 1);
        let prefix = text[line_start..open].trim_end();
        prefix.ends_with('!') || prefix.contains("macro_rules!")
    })
}

/// The inherent `impl` block whose header holds `at`, or which `at` is inside.
pub fn impl_block(text: &str, at: usize) -> Result<ImplBlock> {
    let mut search = (at + 4).min(text.len());
    let impl_at = loop {
        let i = text[..search]
            .rfind("impl")
            .context("no `impl` block at this position")?;
        let whole = !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + 4..].chars().next().is_some_and(is_ident);
        if whole {
            break i;
        }
        search = i;
    };
    let line_start = text[..impl_at].rfind('\n').map_or(0, |i| i + 1);
    anyhow::ensure!(
        !inside_macro(text, impl_at),
        "impl blocks generated inside macros are not supported; expand the macro first"
    );
    let before = text[line_start..impl_at].trim_end();
    anyhow::ensure!(
        !["default", "unsafe", "const"]
            .iter()
            .any(|qualifier| before.ends_with(qualifier)),
        "specialized, unsafe, and const impl blocks are not supported"
    );
    let previous = text[..line_start]
        .trim_end()
        .lines()
        .next_back()
        .unwrap_or("")
        .trim();
    anyhow::ensure!(
        !previous.starts_with("#["),
        "attributes on impl blocks are not supported; remove or expand the conditional impl first"
    );
    let open = impl_body_open(text, impl_at)?;
    let close = crate::parameter_object::matching_bracket(text, open)
        .context("the `impl` block is not closed")?;
    anyhow::ensure!(at <= close, "the position is not in an `impl` block");
    let header = text[impl_at + 4..open].trim();
    anyhow::ensure!(
        !header.contains('#'),
        "attributes in an impl header are not supported"
    );
    anyhow::ensure!(
        !header.contains('!'),
        "macros in an impl header are not supported"
    );
    let (generics, after_generics) = if header.starts_with('<') {
        let close =
            matching_angle(header).context("the impl's generic parameter list is not closed")?;
        (&header[..=close], header[close + 1..].trim_start())
    } else {
        ("", header)
    };
    let where_at = top_level_word(after_generics, "where");
    let (self_ty, where_clause, where_on_newline) = match where_at {
        Some(i) => {
            let before_where = &after_generics[..i];
            (
                before_where.trim(),
                after_generics[i..].trim(),
                before_where.contains('\n'),
            )
        }
        None => (after_generics.trim(), "", false),
    };
    anyhow::ensure!(!self_ty.is_empty(), "the inherent impl has no self type");
    anyhow::ensure!(
        top_level_word(self_ty, "for").is_none(),
        "`impl {header}` already implements a trait"
    );
    let generic_args = generic_arguments(generics)?;
    Ok(ImplBlock {
        start: impl_at,
        open,
        close,
        generics: generics.to_string(),
        generic_args,
        self_ty: self_ty.to_string(),
        header: header.to_string(),
        where_clause: where_clause.to_string(),
        where_on_newline,
        items: items(text, open, close),
    })
}

/// The items between the braces `open` and `close`. An item ends at a `;` or at the `}` that
/// closes its body, at the block's own depth; comments and strings are skipped.
pub fn items(text: &str, open: usize, close: usize) -> Vec<Item> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    let mut i = open + 1;
    while i < close {
        let c = bytes[i];
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            start.get_or_insert(i);
            i = text[i..close].find('\n').map_or(close, |n| i + n);
            continue;
        }
        if c == b'"' {
            i += 1;
            while i < close && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
            continue;
        }
        // A character literal such as '}' (a lifetime has no closing quote two bytes on).
        if c == b'\'' && bytes.get(i + 2) == Some(&b'\'') {
            start.get_or_insert(i);
            i += 3;
            continue;
        }
        if !c.is_ascii_whitespace() {
            start.get_or_insert(i);
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth -= 1,
            b'}' => {
                depth -= 1;
                if depth == 0
                    && let Some(s) = start.take()
                {
                    out.push(item_at(text, s, i + 1));
                }
            }
            b';' if depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(item_at(text, s, i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn item_at(text: &str, start: usize, end: usize) -> Item {
    let decl = declaration(&text[start..end]).1;
    let name = decl.find("fn ").and_then(|i| {
        let before = &decl[..i];
        let qualifiers_only = before.split_whitespace().all(|w| {
            w.starts_with("pub")
                || matches!(w, "const" | "async" | "unsafe" | "extern")
                || w.starts_with('"')
        });
        let name: String = decl[i + 3..]
            .trim_start()
            .chars()
            .take_while(|c| is_ident(*c))
            .collect();
        (qualifiers_only && !name.is_empty()).then_some(name)
    });
    Item { start, end, name }
}

/// An item's text split into its leading lines of comments, docs and attributes, and the
/// declaration that follows them.
pub fn declaration(item: &str) -> (Vec<&str>, &str) {
    let mut leading = Vec::new();
    let mut rest = item;
    loop {
        let t = rest.trim_start();
        if t.starts_with("//") || t.starts_with("#[") {
            let end = t.find('\n').map_or(t.len(), |n| n + 1);
            leading.push(t[..end].trim_end());
            rest = &t[end..];
        } else {
            return (leading, t);
        }
    }
}

/// The visibility a declaration starts with (`pub`, `pub(crate)`, …; empty when private) and
/// the declaration without it.
pub fn visibility(decl: &str) -> (&str, &str) {
    if let Some(rest) = decl.strip_prefix("pub(")
        && let Some(close) = rest.find(')')
    {
        let end = 4 + close + 1;
        return (&decl[..end], decl[end..].trim_start());
    }
    match decl.strip_prefix("pub ") {
        Some(rest) => ("pub", rest.trim_start()),
        None => ("", decl),
    }
}

/// A function's signature: its declaration up to the body's opening brace.
pub fn signature(decl: &str) -> Option<&str> {
    let name_at = decl.find("fn ")? + 3;
    let name_at = name_at + (decl[name_at..].len() - decl[name_at..].trim_start().len());
    let (_, _, close) = crate::signature::param_span(decl, name_at)?;
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut body = None;
    for (offset, c) in decl[close..].char_indices() {
        match c {
            '<' => angle += 1,
            '>' if angle > 0 && !decl[..close + offset].ends_with('-') => angle -= 1,
            '(' => paren += 1,
            ')' if paren > 0 => paren -= 1,
            '[' => square += 1,
            ']' if square > 0 => square -= 1,
            '{' if angle == 0 && paren == 0 && square == 0 => {
                body = Some(close + offset);
                break;
            }
            _ => {}
        }
    }
    let body = body?;
    Some(decl[..body].trim_end())
}

/// An item's text at `indent`. The text starts at its first character, so only the first line
/// lacks the indentation; the others keep theirs, which is right because the new blocks sit at
/// the same depth as the old one.
fn indented(item: &str, indent: &str) -> String {
    format!("{indent}{}", item.trim())
}

/// The block rewritten: the methods named move out of `imp` into `trait {name}` and its
/// `impl {name} for Type`, placed right after the block; the block itself goes when nothing is
/// left in it. Returns the new text and the names of the methods left behind.
pub fn rewrite(
    text: &str,
    imp: &ImplBlock,
    methods: &[String],
    name: &str,
) -> Result<(String, Vec<String>)> {
    anyhow::ensure!(valid_ident(name), "`{name}` is not a valid Rust identifier");
    for method in methods {
        anyhow::ensure!(
            valid_ident(method),
            "`{method}` is not a valid Rust method identifier"
        );
    }
    let available: Vec<&str> = imp.items.iter().filter_map(|i| i.name.as_deref()).collect();
    for m in methods {
        anyhow::ensure!(
            available.contains(&m.as_str()),
            "`{}` has no method `{m}`; it has {}",
            imp.self_ty,
            available.join(", ")
        );
    }
    let line_start = text[..imp.start].rfind('\n').map_or(0, |i| i + 1);
    let before_impl = &text[line_start..imp.start];
    let outer = if before_impl.chars().all(char::is_whitespace) {
        before_impl
    } else {
        ""
    };
    let replacement_start = if outer.is_empty() {
        imp.start
    } else {
        line_start
    };
    let inner = format!("{outer}    ");
    let mut decls = Vec::new();
    let mut bodies = Vec::new();
    let mut kept = Vec::new();
    let mut kept_names = Vec::new();
    let mut widest = "";
    for item in &imp.items {
        let chunk = &text[item.start..item.end];
        let Some(chosen) = item.name.as_ref().filter(|n| methods.contains(n)) else {
            kept.push(indented(chunk, &inner));
            kept_names.extend(item.name.clone());
            continue;
        };
        let (leading, decl) = declaration(chunk);
        anyhow::ensure!(
            !leading
                .iter()
                .any(|line| line.starts_with("#[cfg") || line.starts_with("#[cfg_attr")),
            "method `{chosen}` is conditional; conditional methods cannot be extracted safely"
        );
        let (vis, bare) = visibility(decl);
        // `pub` wins; otherwise the first restricted visibility; otherwise private.
        if vis == "pub" || widest.is_empty() {
            widest = vis;
        }
        let sig =
            signature(bare).with_context(|| format!("cannot read the signature of `{chosen}`"))?;
        let docs: Vec<&str> = leading
            .iter()
            .copied()
            .filter(|l| l.starts_with("///"))
            .collect();
        let attrs: Vec<&str> = leading
            .iter()
            .copied()
            .filter(|l| !l.starts_with("///"))
            .collect();
        let mut decl_text = docs
            .iter()
            .map(|l| format!("{inner}{l}\n"))
            .collect::<String>();
        decl_text.push_str(&indented(&format!("{sig};"), &inner));
        decls.push(decl_text);
        let mut body = attrs
            .iter()
            .map(|l| format!("{inner}{l}\n"))
            .collect::<String>();
        body.push_str(&indented(bare, &inner));
        bodies.push(body);
    }
    let vis = if widest.is_empty() {
        String::new()
    } else {
        format!("{widest} ")
    };
    let block = |header: String, items: &[String]| {
        format!("{outer}{header} {{\n{}\n{outer}}}", items.join("\n\n"))
    };
    let where_clause = if imp.where_clause.is_empty() {
        String::new()
    } else if imp.where_on_newline {
        format!("\n{}", imp.where_clause)
    } else {
        format!(" {}", imp.where_clause)
    };
    let generic_args = if imp.generic_args.is_empty() {
        String::new()
    } else {
        format!("<{}>", imp.generic_args.join(", "))
    };
    let mut replacement = String::new();
    if !kept.is_empty() {
        let inherent = if imp.header.starts_with('<') {
            format!("impl{}", imp.header)
        } else {
            format!("impl {}", imp.header)
        };
        replacement.push_str(&block(inherent, &kept));
        replacement.push_str("\n\n");
    }
    replacement.push_str(&block(
        format!("{vis}trait {name}{}{where_clause}", imp.generics),
        &decls,
    ));
    replacement.push_str("\n\n");
    replacement.push_str(&block(
        format!(
            "impl{} {name}{generic_args} for {}{where_clause}",
            imp.generics, imp.self_ty
        ),
        &bodies,
    ));
    let mut out = String::with_capacity(text.len() + replacement.len());
    out.push_str(&text[..replacement_start]);
    out.push_str(&replacement);
    out.push_str(&text[imp.close + 1..]);
    Ok((out, kept_names))
}

/// Extracts `trait {name}` from the methods `methods` of the inherent `impl` block at
/// `line`:`col` of `file`.
#[allow(clippy::too_many_arguments)]
pub async fn extract_trait(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    methods: &[String],
    name: &str,
    apply: bool,
    force: bool,
) -> Result<Extracted> {
    anyhow::ensure!(valid_ident(name), "`{name}` is not a valid Rust identifier");
    anyhow::ensure!(!methods.is_empty(), "name at least one method");
    for method in methods {
        anyhow::ensure!(
            valid_ident(method),
            "`{method}` is not a valid Rust method identifier"
        );
    }
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at =
        crate::signature::offset_of(&text, line, col).context("the position is not in the file")?;
    let imp = impl_block(&text, at)?;
    let (new_text, kept) = rewrite(&text, &imp, methods, name)?;

    // Every file outside this one that calls a moved method needs the trait in scope.
    let (_, module) = crate::move_item::module_of(file)?;
    let mut files: BTreeMap<PathBuf, String> = BTreeMap::new();
    files.insert(file.to_path_buf(), new_text);
    let mut imports = Vec::new();
    for item in imp
        .items
        .iter()
        .filter(|i| i.name.as_ref().is_some_and(|n| methods.contains(n)))
    {
        let fn_name = item.name.as_deref().unwrap_or_default();
        let name_at = item.start
            + text[item.start..item.end]
                .find(&format!("fn {fn_name}"))
                .map_or(0, |i| i + 3);
        let (l, c) = crate::signature::position_at(&text, name_at)?;
        for (path, _, _) in crate::signature::references(remote, root, file, l, c).await? {
            if path == file {
                continue;
            }
            let caller_crate = crate::move_item::module_of(&path)
                .map(|(_, m)| m.krate)
                .unwrap_or_else(|_| module.krate.clone());
            let use_line = format!("use {}::{name};", module.spelled_from(&caller_crate));
            let current = match files.get(&path) {
                Some(t) => t.clone(),
                None => std::fs::read_to_string(&path)
                    .with_context(|| format!("cannot read {}", path.display()))?,
            };
            let updated = crate::move_item::add_import(&current, &use_line);
            if updated != current {
                imports.push((path.clone(), use_line));
                files.insert(path, updated);
            }
        }
    }

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
    let rel = |p: &Path| p.strip_prefix(root).unwrap_or(p).display().to_string();
    Ok(Extracted {
        trait_name: name.to_string(),
        type_name: imp.self_ty.clone(),
        root: root.to_path_buf(),
        file: rel(file),
        methods: methods.to_vec(),
        kept,
        imports: imports.iter().map(|(p, l)| (rel(p), l.clone())).collect(),
        rewritten: files
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

    const SHAPES: &str = "pub struct Rect {\n    pub w: f64,\n}\n\nimpl Rect {\n    pub fn new(w: f64) -> Self {\n        Rect { w }\n    }\n\n    /// The area.\n    #[inline]\n    pub fn area(&self) -> f64 {\n        // a stray } here\n        self.w * self.w\n    }\n\n    pub(crate) fn half(&self) -> f64 {\n        let _ = '}';\n        self.w / 2.0\n    }\n}\n\nfn other() {}\n";

    #[test]
    fn the_block_and_its_items_are_found_with_their_names() {
        let imp = impl_block(SHAPES, SHAPES.find("impl Rect").unwrap() + 2).unwrap();
        assert_eq!(imp.self_ty, "Rect");
        let names: Vec<_> = imp.items.iter().map(|i| i.name.clone().unwrap()).collect();
        assert_eq!(names, ["new", "area", "half"]);
        assert!(SHAPES[imp.items[1].start..].starts_with("/// The area."));
        assert!(SHAPES[..imp.items[1].end].ends_with("self.w * self.w\n    }"));
        assert_eq!(
            impl_block("impl<T> Foo<T> {}", 0).unwrap().generic_args,
            ["T"]
        );
        assert_eq!(
            impl_block("impl<T: Iterator<Item = u8>> Foo<T> {}", 0)
                .unwrap()
                .generic_args,
            ["T"]
        );
        assert!(impl_block("impl Display for Foo {}", 0).is_err());
    }

    #[test]
    fn a_generic_impl_header_is_preserved_and_reused() {
        let text = "impl<'a, T: Clone, const N: usize> Name<'a, T, N>\nwhere\n    T: Default,\n{\n    fn selected<U: Copy>(&self, value: U) -> Self\n    where\n        U: Into<T>,\n    {\n        let _ = value;\n        todo!()\n    }\n\n    fn kept(&self) -> usize { N }\n}\n";
        let imp = impl_block(text, text.find("impl").unwrap()).expect("generic impl");
        let (out, kept) = rewrite(text, &imp, &["selected".into()], "Selected").unwrap();
        assert_eq!(kept, ["kept"]);
        assert!(
            out.contains("trait Selected<'a, T: Clone, const N: usize>\nwhere\n    T: Default,"),
            "{out}"
        );
        assert!(
            out.contains(
                "impl<'a, T: Clone, const N: usize> Selected<'a, T, N> for Name<'a, T, N>\nwhere\n    T: Default,"
            ),
            "{out}"
        );
        assert!(
            out.contains("fn selected<U: Copy>(&self, value: U) -> Self"),
            "{out}"
        );
        assert!(
            out.contains(
                "impl<'a, T: Clone, const N: usize> Name<'a, T, N>\nwhere\n    T: Default,"
            ),
            "{out}"
        );

        let all = ["selected".into(), "kept".into()];
        let (all_out, kept) = rewrite(text, &imp, &all, "Everything").unwrap();
        assert!(kept.is_empty());
        assert!(!all_out.contains("impl<'a, T: Clone, const N: usize> Name"));
        assert!(all_out.contains("impl<'a, T: Clone, const N: usize> Everything<'a, T, N>"));
    }

    #[test]
    fn generic_refusals_and_exact_replacement_are_explicit() {
        for (source, expected) in [
            ("impl<T = u8> Name<T> {}", "defaults"),
            (
                "impl<T> Trait<T> for Name<T> {}",
                "already implements a trait",
            ),
            ("#[cfg(test)]\nimpl Name {}", "attributes on impl blocks"),
            ("impl<T> Name<make!{T}> {}", "macros in an impl header"),
            (
                "macro_rules! make { () => { impl<T> Name<T> {} } }",
                "generated inside macros",
            ),
            ("default impl<T> Name<T> {}", "specialized"),
        ] {
            let err = impl_block(source, source.find("impl").unwrap()).unwrap_err();
            assert!(format!("{err:#}").contains(expected), "{source}: {err:#}");
        }

        let inline =
            "fn before() -> u8 { 1 } impl<T> Name<T> { fn value(&self) -> Self { todo!() } }\n";
        let imp = impl_block(inline, inline.find("impl").unwrap()).unwrap();
        let (out, _) = rewrite(inline, &imp, &["value".into()], "Value").unwrap();
        assert!(
            out.starts_with("fn before() -> u8 { 1 } trait Value<T>"),
            "{out}"
        );
        assert!(out.contains("fn value(&self) -> Self;"), "{out}");

        let err = rewrite(inline, &imp, &["value".into()], "2Value").unwrap_err();
        assert!(format!("{err:#}").contains("valid Rust identifier"));

        let conditional = "impl Name { #[cfg(test)]\nfn value(&self) {} }";
        let imp = impl_block(conditional, 0).unwrap();
        let err = rewrite(conditional, &imp, &["value".into()], "Value").unwrap_err();
        assert!(format!("{err:#}").contains("conditional"), "{err:#}");
    }

    #[test]
    fn a_signature_loses_its_visibility_and_body() {
        assert_eq!(visibility("pub(crate) fn f()"), ("pub(crate)", "fn f()"));
        assert_eq!(visibility("pub fn f()"), ("pub", "fn f()"));
        assert_eq!(visibility("fn f()"), ("", "fn f()"));
        assert_eq!(
            signature("fn get<T: Into<u8>>(&self, t: T) -> Option<u8> where T: Copy {"),
            Some("fn get<T: Into<u8>>(&self, t: T) -> Option<u8> where T: Copy")
        );
        let (leading, decl) = declaration("/// Doc.\n    #[inline]\n    fn f() {}");
        assert_eq!(
            (leading, decl),
            (vec!["/// Doc.", "#[inline]"], "fn f() {}")
        );
    }

    #[test]
    fn only_the_named_methods_move_and_the_trait_is_as_visible_as_the_widest() {
        let imp = impl_block(SHAPES, SHAPES.find("impl Rect").unwrap()).unwrap();
        let (out, kept) =
            rewrite(SHAPES, &imp, &["area".into(), "half".into()], "Measure").unwrap();
        assert_eq!(kept, ["new"]);
        assert!(out.contains("impl Rect {\n    pub fn new(w: f64) -> Self {\n        Rect { w }\n    }\n}\n\npub trait Measure {\n    /// The area.\n    fn area(&self) -> f64;\n\n    fn half(&self) -> f64;\n}\n\nimpl Measure for Rect {\n    #[inline]\n    fn area(&self) -> f64 {\n        // a stray } here\n        self.w * self.w\n    }\n\n    fn half(&self) -> f64 {\n        let _ = '}';\n        self.w / 2.0\n    }\n}\n\nfn other() {}\n"), "{out}");

        // Every method taken: no empty `impl Rect {}` is left behind.
        let all: Vec<String> = ["new", "area", "half"].map(String::from).to_vec();
        let (out, kept) = rewrite(SHAPES, &imp, &all, "Measure").unwrap();
        assert!(kept.is_empty() && !out.contains("impl Rect {"), "{out}");
        let (out, _) = rewrite(SHAPES, &imp, &["half".into()], "Halve").unwrap();
        assert!(out.contains("pub(crate) trait Halve {"), "{out}");

        let err = rewrite(SHAPES, &imp, &["volume".into()], "M").unwrap_err();
        assert!(format!("{err}").contains("has no method `volume`; it has new, area, half"));
    }
}
