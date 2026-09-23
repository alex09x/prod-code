//! Inverting a boolean field or local variable: `enabled` becomes `disabled`, and every place that
//! reads it or writes it keeps doing what it did.
//!
//! A read gains a `!` (or loses the one it had); a write stores the negation of what it stored,
//! `x.enabled = v` becoming `x.disabled = !(v)`; a struct literal initialises the new field with the
//! negation. What cannot be rewritten that way is reported and blocks the write: a borrow of the
//! value (`&mut x.enabled` hands out the old meaning), a compound assignment (`x.enabled |= v`
//! is not the negation of `x.disabled |= v`), a pattern that binds the field, a use inside a
//! format string, and — for a field — a `#[derive(Default)]` (the default `false` would now mean
//! the opposite) or a serde derive (the serialised name and meaning would change).

use crate::invert_boolean::Inverted;
use anyhow::Result;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What a boolean name at a declaration is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueKind {
    /// A field of the struct whose header (`… struct Name …`) starts at `header`.
    Field { header: usize },
    /// A `let` binding; `annotated` when it is written `name: bool`.
    Local { annotated: bool },
}

/// The kind of declaration the name starting at `start` is, when it is one this can invert.
pub fn value_kind(text: &str, start: usize, name: &str) -> Option<ValueKind> {
    let after = text[start + name.len()..].trim_start();
    let before = text[..start].trim_end();
    let before_let = before
        .strip_suffix("mut")
        .map(str::trim_end)
        .unwrap_or(before);
    if before_let.ends_with("let")
        && !before_let[..before_let.len() - 3]
            .chars()
            .next_back()
            .is_some_and(is_ident)
    {
        let annotated = after.strip_prefix(':').is_some_and(|t| {
            t.trim_start().starts_with("bool") && !t.trim_start()[4..].starts_with(is_ident)
        });
        return Some(ValueKind::Local { annotated });
    }
    // `name: bool` inside the braces of a `struct`.
    let ty = after.strip_prefix(':').filter(|t| !t.starts_with(':'))?;
    let ty_end = ty.find([',', '}', '\n']).unwrap_or(ty.len());
    if ty[..ty_end].trim() != "bool" {
        return None;
    }
    let open = enclosing_open_brace(text, start)?;
    let header_start = text[..open].rfind(['\n', ';', '}']).map_or(0, |i| i + 1);
    let header = &text[header_start..open];
    header
        .split(|c: char| !is_ident(c))
        .any(|word| word == "struct")
        .then_some(ValueKind::Field {
            header: header_start,
        })
}

/// The offset of the `{` that encloses `at`.
fn enclosing_open_brace(text: &str, at: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in text[..at].char_indices().rev() {
        match c {
            '}' => depth += 1,
            '{' if depth == 0 => return Some(i),
            '{' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The derives on the item whose header starts at `header`: the words inside every
/// `#[derive(…)]` directly above it.
fn derives_above(text: &str, header: usize) -> Vec<String> {
    let mut out = Vec::new();
    for line in text[..header].lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with("#[") && !line.starts_with("///") && !line.starts_with("//") {
            break;
        }
        if let Some(rest) = line.strip_prefix("#[derive(") {
            out.extend(
                rest.trim_end_matches(")]")
                    .split(',')
                    .map(|w| w.trim().rsplit("::").next().unwrap_or("").to_string())
                    .filter(|w| !w.is_empty()),
            );
        }
    }
    out
}

/// The end of the expression that starts at `from`: the first `;`, or `,` / `)` / `}` / `]` that
/// closes nothing opened after `from`.
fn expression_end(text: &str, from: usize) -> usize {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut prev = '\0';
    for (i, c) in text[from..].char_indices() {
        if in_str {
            if c == '"' && prev != '\\' {
                in_str = false;
            }
            prev = c;
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return from + i,
            ')' | ']' | '}' => depth -= 1,
            ',' | ';' if depth == 0 => return from + i,
            _ => {}
        }
        prev = c;
    }
    text.len()
}

/// `!(value)`, or `value` without its `!` when it already had one around a simple operand.
fn negation_of(value: &str) -> String {
    let v = value.trim();
    if let Some(rest) = v.strip_prefix('!')
        && !rest.starts_with('=')
        && rest
            .chars()
            .all(|c| is_ident(c) || c == '.' || c == '(' || c == ')' || c == ':')
    {
        return rest.to_string();
    }
    match v {
        "true" => return "false".to_string(),
        "false" => return "true".to_string(),
        _ => {}
    }
    format!("!({v})")
}

/// Whether the offset `at` is inside a string literal on its line.
fn inside_string(text: &str, at: usize) -> bool {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let mut quotes = 0;
    let mut prev = '\0';
    for c in text[line_start..at].chars() {
        if c == '"' && prev != '\\' {
            quotes += 1;
        }
        prev = c;
    }
    quotes % 2 == 1
}

/// Whether the `{` at `open` opens a struct literal or pattern (`Flags {`, `Self {`,
/// `crate::m::Flags {`) rather than a block (`if flag {`, `else {`, `=> {`). A type is named in
/// CamelCase, a condition or a binding is not: text alone cannot tell `if flag { x }` from a
/// struct named `flag`, and Rust's naming convention can.
fn is_struct_brace(text: &str, open: usize) -> bool {
    let before = text[..open].trim_end();
    let segment: String = before
        .chars()
        .rev()
        .take_while(|c| is_ident(*c))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    segment.chars().next().is_some_and(char::is_uppercase)
}

/// Whether the struct braces that start at `open` are a pattern rather than an expression: a
/// pattern is followed by `=` (a `let`), `=>` or `|` (a match arm) or `if` (a guard).
fn braces_are_pattern(text: &str, open: usize) -> bool {
    let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
        return false;
    };
    let after = text[close + 1..].trim_start();
    (after.starts_with('=') && !after.starts_with("=="))
        || after.starts_with('|') && !after.starts_with("||")
        || after.starts_with("if ")
        || after.starts_with("=>")
}

/// Inverts the boolean field or local declared at `start` (its name) of `file`.
#[allow(clippy::too_many_arguments)]
pub async fn invert_value(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    start: usize,
    kind: ValueKind,
    new_name: &str,
    apply: bool,
    force: bool,
) -> Result<Inverted> {
    let name: String = text[start..].chars().take_while(|c| is_ident(*c)).collect();
    anyhow::ensure!(name != new_name, "the new name is the old one");
    let mut blocked = Vec::new();
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    texts.insert(file.to_path_buf(), text.to_string());
    let (l0, c0) = crate::signature::line_col_at(text, start);

    // The declaration.
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((start, name.len(), new_name.to_string()));
    let mut writes = 0usize;
    match &kind {
        ValueKind::Field { header } => {
            for derive in derives_above(text, *header) {
                match derive.as_str() {
                    "Default" => blocked.push(format!(
                        "{}:{l0}:{c0} the struct derives `Default`: a default `{name}` of `false` \
                         would be a default `{new_name}` of `false`, the opposite; write the \
                         `Default` impl by hand first",
                        display(root, file)
                    )),
                    "Serialize" | "Deserialize" => blocked.push(format!(
                        "{}:{l0}:{c0} the struct derives `{derive}`: the serialised field would \
                         change its name and its meaning; keep the name with `#[serde(rename)]` \
                         and invert the value in a custom (de)serializer, or do it by hand",
                        display(root, file)
                    )),
                    _ => {}
                }
            }
        }
        ValueKind::Local { annotated } => {
            if !annotated {
                let hover = local_type(remote, root, file, l0, c0).await;
                anyhow::ensure!(
                    hover.as_deref() == Some("bool"),
                    "`{name}` is {}, not `bool`: only a boolean can be inverted",
                    hover.map_or(
                        "of a type the analyzer does not say".to_string(),
                        |t| format!("`{t}`")
                    )
                );
            }
            // `let name = value;` stores the negation.
            let rest = &text[start + name.len()..];
            if let Some(eq) = rest
                .find('=')
                .filter(|&i| !rest[..i].contains(';') && !rest[i..].starts_with("=="))
            {
                let value_start = start + name.len() + eq + 1;
                let value_end = expression_end(text, value_start);
                own.push((
                    value_start,
                    value_end - value_start,
                    format!(" {}", negation_of(&text[value_start..value_end])),
                ));
                writes += 1;
            }
        }
    }

    let (mut negated, mut cancelled) = (0usize, 0usize);
    let mut unmatched = Vec::new();
    for (path, l, c) in crate::signature::references(remote, root, file, l0, c0)
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
        if inside_string(&body, at) {
            blocked.push(format!("{site} `{name}` is used inside a format string"));
            continue;
        }
        let end = at + name.len();
        let after = body[end..].trim_start();
        let before = body[..at].trim_end();
        let spot = edits.entry(path.clone()).or_default();

        // In struct braces: `name: value`, or the shorthand `name`.
        let struct_open = enclosing_open_brace(&body, at).filter(|&o| is_struct_brace(&body, o));
        if let Some(open) = struct_open
            && (before.ends_with('{') || before.ends_with(','))
        {
            let is_init = after.starts_with(':') && !after.starts_with("::");
            let is_shorthand = after.starts_with(',') || after.starts_with('}');
            if is_init || is_shorthand {
                if braces_are_pattern(&body, open) {
                    blocked.push(format!(
                        "{site} a pattern binds `{name}`: the binding would hold the opposite"
                    ));
                    continue;
                }
                let field_side = matches!(kind, ValueKind::Field { .. });
                if is_init && field_side {
                    let colon = end + body[end..].find(':').unwrap_or(0);
                    let value_end = expression_end(&body, colon + 1);
                    spot.push((at, name.len(), new_name.to_string()));
                    spot.push((
                        colon + 1,
                        value_end - colon - 1,
                        format!(" {}", negation_of(&body[colon + 1..value_end])),
                    ));
                    writes += 1;
                } else if is_shorthand && field_side {
                    // `S { enabled }` with a local `enabled`: `S { disabled: !enabled }`.
                    spot.push((at, name.len(), format!("{new_name}: !{name}")));
                    writes += 1;
                } else if is_shorthand {
                    // A local used as a field's shorthand: `S { flag }` → `S { flag: !not_flag }`.
                    spot.push((at, name.len(), format!("{name}: !{new_name}")));
                    negated += 1;
                } else {
                    unmatched.push(format!("{site} (neither a read nor a write)"));
                }
                continue;
            }
        }

        // Where the use begins: the receiver chain of a field, or the name of a local.
        let (begin, is_field_access) = match before.strip_suffix('.') {
            Some(rest) => (
                crate::encapsulate_field::chain_start(&body, rest.len()),
                true,
            ),
            None => (at, false),
        };
        let lead = body[..begin].trim_end();
        if lead.ends_with('&') || lead.ends_with("&mut") {
            blocked.push(format!(
                "{site} `{name}` is borrowed: the reference would read the opposite"
            ));
            continue;
        }
        let compound = ["|=", "&=", "^="];
        if compound.iter().any(|op| after.starts_with(op)) {
            blocked.push(format!(
                "{site} a compound assignment to `{name}` is not the negation of the same one"
            ));
            continue;
        }
        if after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>") {
            let eq = end + body[end..].find('=').unwrap_or(0);
            let value_end = expression_end(&body, eq + 1);
            spot.push((at, name.len(), new_name.to_string()));
            spot.push((
                eq + 1,
                value_end - eq - 1,
                format!(" {}", negation_of(&body[eq + 1..value_end])),
            ));
            writes += 1;
            continue;
        }
        if !is_field_access && matches!(kind, ValueKind::Field { .. }) {
            unmatched.push(format!("{site} (a field named without a receiver)"));
            continue;
        }
        let continues = after.starts_with('.') || after.starts_with('?') || after.starts_with('[');
        spot.push((at, name.len(), new_name.to_string()));
        if !continues && lead.ends_with('!') && !lead.ends_with("!=") {
            spot.push((lead.len() - 1, 1, String::new()));
            cancelled += 1;
        } else if continues {
            spot.push((begin, 0, "(!".to_string()));
            spot.push((end, 0, ")".to_string()));
            negated += 1;
        } else {
            spot.push((begin, 0, "!".to_string()));
            negated += 1;
        }
    }

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
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
            blocked.is_empty() || force,
            "{} use(s) of `{name}` cannot keep their meaning under the inversion; nothing was \
             written:\n  {}",
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

    Ok(Inverted {
        was: name,
        now: new_name.to_string(),
        root: root.to_path_buf(),
        file: display(root, file),
        kind: match kind {
            ValueKind::Field { .. } => "field",
            ValueKind::Local { .. } => "variable",
        }
        .to_string(),
        negated,
        cancelled,
        writes,
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

/// The type the analyzer's hover gives a local: `bool` for `let flag: bool`.
async fn local_type(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Option<String> {
    let uri = url::Url::from_file_path(file).ok()?.to_string();
    let res = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/hover",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
        }),
    )
    .await
    .ok()?;
    let value = res.pointer("/contents/value").and_then(|v| v.as_str())?;
    hover_type(value)
}

/// `bool` out of a hover like "```rust\nlet flag: bool\n```".
pub fn hover_type(hover: &str) -> Option<String> {
    let line = hover.lines().find(|l| l.trim_start().starts_with("let "))?;
    let (_, ty) = line.split_once(':')?;
    Some(ty.trim().to_string())
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_of(text: &str, name: &str) -> Option<ValueKind> {
        value_kind(text, text.find(name).unwrap(), name)
    }

    #[test]
    fn a_bool_field_and_a_let_are_recognised_and_nothing_else() {
        let s = "#[derive(Debug)]\npub struct S {\n    pub enabled: bool,\n    pub n: u32,\n}\n";
        assert!(matches!(
            kind_of(s, "enabled"),
            Some(ValueKind::Field { .. })
        ));
        assert_eq!(kind_of(s, "n: u32"), None);
        assert_eq!(
            kind_of("fn f() { let flag: bool = true; }", "flag"),
            Some(ValueKind::Local { annotated: true })
        );
        assert_eq!(
            kind_of("fn f() { let mut flag = g(); }", "flag"),
            Some(ValueKind::Local { annotated: false })
        );
        assert_eq!(kind_of("fn f(outlet: bool) {}", "outlet"), None);
        assert_eq!(kind_of("fn f(x: bool) {}", "x"), None);
    }

    #[test]
    fn a_negation_cancels_and_a_literal_flips() {
        assert_eq!(negation_of(" v"), "!(v)");
        assert_eq!(negation_of(" !v"), "v");
        assert_eq!(negation_of("!x.ready()"), "x.ready()");
        assert_eq!(negation_of(" a && b"), "!(a && b)");
        assert_eq!(negation_of(" true"), "false");
        assert_eq!(negation_of("false"), "true");
        assert_eq!(negation_of("!a || b"), "!(!a || b)");
    }

    #[test]
    fn derives_and_patterns_and_strings_are_seen() {
        let s = "#[derive(Default, serde::Serialize)]\n/// doc\npub struct S {\n    x: bool,\n}\n";
        let header = s.find("pub struct").unwrap();
        assert_eq!(derives_above(s, header), ["Default", "Serialize"]);
        let p = "let S { x, .. } = s;\nlet t = S { x };\n";
        assert!(braces_are_pattern(p, p.find('{').unwrap()));
        assert!(!braces_are_pattern(p, p.rfind('{').unwrap()));
        let b = "if flag { x } else { y }; Flags { x }; Self { x }; m::Flags { x }";
        let opens: Vec<usize> = b.match_indices('{').map(|(i, _)| i).collect();
        let kinds: Vec<bool> = opens.iter().map(|&o| is_struct_brace(b, o)).collect();
        assert_eq!(kinds, [false, false, true, true, true]);
        let q = "println!(\"{x}\"); x";
        assert!(inside_string(q, q.find("x}").unwrap()));
        assert!(!inside_string(q, q.rfind('x').unwrap()));
        assert_eq!(expression_end("a = f(1, 2), b", 4), 11);
        assert_eq!(expression_end("x = y && z;", 4), 10);
        assert_eq!(
            hover_type("```rust\nlet flag: bool\n```").as_deref(),
            Some("bool")
        );
        assert_eq!(
            hover_type("```rust\nlet n: u32\n```").as_deref(),
            Some("u32")
        );
    }
}
