/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use super::syntax::{close_in, is_ident_byte, line_end, walk_code};
use super::types::Language;

/// The byte range of a function's body, from the end of its parameter list to the end of the
/// function. A reference outside it is not a use in the body: basedpyright lists a keyword
/// argument at a call site (`height=2`) among the references to the parameter `height`.
pub(crate) fn body_span(
    text: &str,
    decl: usize,
    close: usize,
    language: Language,
) -> (usize, usize) {
    if language != Language::Python {
        return match body_open(text, close, language).and_then(|o| close_in(text, o, language)) {
            Some(end) => (close, end),
            None => (close, close),
        };
    }
    // The signature ends at the first top-level colon after the parameter list; the return
    // annotation before it has none.
    let mut depth = 0i32;
    let mut colon = None;
    walk_code(text, close + 1, language, |i, c| {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 => {
                colon = Some(i);
                return false;
            }
            _ => {}
        }
        true
    });
    let Some(colon) = colon else {
        return (close, close);
    };
    let eol = line_end(text.as_bytes(), colon);
    let same_line = text[colon + 1..eol].trim();
    if !same_line.is_empty() && !same_line.starts_with('#') {
        return (close, eol);
    }
    let def_line = text[..decl].rfind('\n').map_or(0, |i| i + 1);
    let indent = text[def_line..].len() - text[def_line..].trim_start_matches([' ', '\t']).len();
    let mut at = eol + 1;
    while at < text.len() {
        let end = line_end(text.as_bytes(), at);
        let line = &text[at..end];
        let code = line.trim_start();
        if !code.is_empty() && !code.starts_with('#') && line.len() - code.len() <= indent {
            break;
        }
        at = end + 1;
    }
    (close, at.min(text.len()))
}

/// The brace that opens a TypeScript or Go function's body, past its return type — which can
/// be an object type in braces itself (`): { a: number } {`), told apart by what precedes it.
/// `None` for a declaration without a body, such as a TypeScript overload.
pub(crate) fn body_open(text: &str, close: usize, language: Language) -> Option<usize> {
    let mut prev = b')';
    let mut skip_until = 0usize;
    let mut found = None;
    walk_code(text, close + 1, language, |i, c| {
        if i < skip_until || c.is_ascii_whitespace() {
            return true;
        }
        match c {
            b'{' if !matches!(prev, b':' | b'|' | b'&' | b'<' | b',') => {
                found = Some(i);
                return false;
            }
            b'{' | b'(' | b'[' => match close_in(text, i, language) {
                Some(end) => {
                    skip_until = end + 1;
                    prev = b')';
                }
                None => return false,
            },
            b';' => return false,
            _ => prev = c,
        }
        true
    });
    found
}

/// The start of the line of the top-level declaration the function belongs to: the class of a
/// TypeScript or Python method, the function itself otherwise (a Go method is top-level).
pub(crate) fn top_level_line(text: &str, decl: usize) -> usize {
    let mut start = text[..decl].rfind('\n').map_or(0, |i| i + 1);
    while start > 0 {
        let line = &text[start..line_end(text.as_bytes(), start)];
        if !line.trim().is_empty() && !line.starts_with([' ', '\t']) {
            break;
        }
        start = text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
    }
    start
}

/// Where the new type goes: above the top-level declaration, and above the comments and
/// decorators that belong to it, so it does not come between a declaration and its doc.
pub(crate) fn item_start_in(text: &str, top: usize, language: Language) -> usize {
    let mut start = top;
    while start > 0 {
        let prev_start = text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        let prev = text[prev_start..start - 1].trim_start();
        let belongs = match language {
            Language::Python => prev.starts_with('#') || prev.starts_with('@'),
            // A C++ template header and an attribute on a line of its own are part of the
            // declaration below them.
            Language::C | Language::Cpp => ["//", "/*", "*", "template", "[["]
                .iter()
                .any(|p| prev.starts_with(p)),
            _ => ["//", "/*", "*", "@"].iter().any(|p| prev.starts_with(p)),
        };
        if !belongs {
            break;
        }
        start = prev_start;
    }
    start
}

/// The qualifier a call spells the function with — `home.` in `home.build(…)` — which the new
/// type needs as well, since it is declared next to the function.
pub(crate) fn qualifier_before(text: &str, at: usize) -> &str {
    let bytes = text.as_bytes();
    if at == 0 || bytes[at - 1] != b'.' {
        return "";
    }
    let mut start = at;
    while start > 0 && (bytes[start - 1] == b'.' || is_ident_byte(bytes[start - 1])) {
        start -= 1;
    }
    &text[start..at]
}

/// Whether a reference is a name in an import statement. basedpyright lists the `build` of
/// `from app.home import build` among the references to `build`; it is neither a call to
/// rewrite nor a use to report.
pub(crate) fn in_import(text: &str, at: usize, language: Language) -> bool {
    let starts = |l: &str| match language {
        Language::Python => l.starts_with("from ") || l.starts_with("import "),
        // CommonJS imports with `require` and exports by assigning to `module.exports`.
        Language::JavaScript => {
            l.starts_with("import ")
                || l.starts_with("export {")
                || l.contains("require(")
                || l.starts_with("module.exports")
                || l.starts_with("exports.")
        }
        _ => {
            l.starts_with("import ") || l.starts_with("export {") || l.starts_with("export type {")
        }
    };
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    if starts(text[line_start..].trim_start()) {
        return true;
    }
    // A name on a line of its own inside an import's parenthesised or braced list: the list
    // opens right after the `import` keyword and has not closed yet.
    let Some(keyword) = text[..line_start].rfind("import") else {
        return false;
    };
    let statement = text[..keyword].rfind('\n').map_or(0, |i| i + 1);
    if !starts(text[statement..].trim_start()) {
        return false;
    }
    let (open, close) = if language == Language::Python {
        ('(', ')')
    } else {
        ('{', '}')
    };
    let list = text[keyword + "import".len()..at].trim_start();
    let list = list.strip_prefix("type ").unwrap_or(list).trim_start();
    list.starts_with(open) && !list.contains(close)
}

/// The edit that adds `name` to the import a Python file already has from the declaring module
/// — the one whose last component is `stem` — as (offset, text to insert). `Some` with an empty
/// text when the name is imported already; `None` when there is no such import to extend.
pub(crate) fn python_import_edit(text: &str, stem: &str, name: &str) -> Option<(usize, String)> {
    let mut line_start = 0usize;
    for line in text.split_inclusive('\n') {
        let at = line_start;
        line_start += line.len();
        let Some(rest) = line.strip_prefix("from ") else {
            continue;
        };
        let Some((module, names)) = rest.split_once(" import ") else {
            continue;
        };
        if module.trim().rsplit('.').next() != Some(stem) {
            continue;
        }
        let names_at = at + line.len() - names.len();
        if names.trim_start().starts_with('(') {
            let open = names_at + names.len() - names.trim_start().len();
            let close = open + 1 + text[open + 1..].find(')')?;
            if text[open + 1..close].split(',').any(|n| n.trim() == name) {
                return Some((close, String::new()));
            }
            // A list that ends in a trailing comma keeps one.
            let before = text[..close].trim_end();
            return Some(if before.ends_with(',') {
                (before.len(), format!(" {name},"))
            } else {
                (before.len(), format!(", {name}"))
            });
        }
        let list = names.split('#').next().unwrap_or("");
        if list.split(',').any(|n| n.trim() == name) {
            return Some((at, String::new()));
        }
        return Some((names_at + list.trim_end().len(), format!(", {name}")));
    }
    None
}

/// Where `from dataclasses import dataclass` goes, and the text to insert there: after the last
/// top-level import, or after the module's docstring, or at the very top. `None` when the file
/// imports `dataclass` already.
pub(crate) fn dataclass_import(text: &str) -> Option<(usize, String)> {
    const LINE: &str = "from dataclasses import dataclass";
    let mut after_imports = None;
    let mut offset = 0usize;
    let mut open_paren = false;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if open_paren {
            if line.contains(')') {
                open_paren = false;
                after_imports = Some(offset);
            }
            continue;
        }
        if let Some(names) = line.strip_prefix("from dataclasses import ")
            && names
                .split([',', '(', ')', ' ', '\n'])
                .any(|n| n.trim() == "dataclass")
        {
            return None;
        }
        if line.starts_with("import ") || line.starts_with("from ") {
            open_paren = line.contains('(') && !line.contains(')');
            if !open_paren {
                after_imports = Some(offset);
            }
        }
    }
    if let Some(at) = after_imports {
        let tail = if text[..at].ends_with('\n') { "" } else { "\n" };
        return Some((at, format!("{tail}{LINE}\n")));
    }
    for quote in ["\"\"\"", "'''"] {
        if let Some(rest) = text.strip_prefix(quote)
            && let Some(n) = rest.find(quote)
        {
            let end = line_end(text.as_bytes(), quote.len() + n + quote.len());
            let at = (end + 1).min(text.len());
            return Some((at, format!("\n{LINE}\n")));
        }
    }
    Some((0, format!("{LINE}\n\n")))
}

/// The first line of the outermost declaration around a 0-based line that is not a namespace,
/// in a documentSymbol answer: the class of a method, the function itself otherwise. Lines are
/// enough to tell which declaration holds a name: two declarations do not share one.
pub(crate) fn outermost_container(symbols: &[serde_json::Value], line: u64) -> Option<u64> {
    for s in symbols {
        let Some(range) = s.get("range").or_else(|| s.pointer("/location/range")) else {
            continue;
        };
        let at = |p: &str| range.pointer(p).and_then(|v| v.as_u64());
        let (Some(start), Some(end)) = (at("/start/line"), at("/end/line")) else {
            continue;
        };
        if line < start || line > end {
            continue;
        }
        // Module, namespace and package: the new type belongs inside them, next to the function.
        if matches!(s.get("kind").and_then(|k| k.as_u64()), Some(2..=4)) {
            return s
                .get("children")
                .and_then(|c| c.as_array())
                .and_then(|c| outermost_container(c, line));
        }
        return Some(start);
    }
    None
}

/// The start of the line the new type goes above in a C or C++ file, for the declaration whose
/// name is at `at`: the outermost declaration around it that is not a namespace, as the
/// server's documentSymbol nests them.
///
/// The text's indentation is not enough here, as it is in the other languages: a class's
/// `public:` is written at the class's own indentation often enough, and would be taken for
/// the line the class starts on. Without an answer from the server, the lines that are only an
/// access specifier or a preprocessor directive are passed over.
pub(crate) async fn container_line(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    text: &str,
    at: usize,
) -> usize {
    // An offset on no position asks the server nothing and falls to the text below.
    let line = crate::signature::line_col_at(text, at).map(|(line, _)| line);
    let answer = match url::Url::from_file_path(path) {
        Ok(uri) => crate::tools::execute_lsp_query(
            remote,
            root,
            path,
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": uri.to_string() } }),
        )
        .await
        .ok(),
        Err(_) => None,
    };
    let found = answer
        .as_ref()
        .and_then(|a| a.as_array())
        .zip(line)
        .and_then(|(symbols, line)| outermost_container(symbols, u64::from(line - 1)))
        .and_then(|l| crate::signature::offset_of(text, l as u32 + 1, 1));
    if let Some(start) = found {
        return start;
    }
    let mut start = top_level_line(text, at);
    while start > 0 {
        let code = text[start..line_end(text.as_bytes(), start)].trim();
        let access = matches!(code, "public:" | "private:" | "protected:");
        if !access && !code.starts_with('#') {
            break;
        }
        start = top_level_line(text, start - 1);
    }
    start
}
