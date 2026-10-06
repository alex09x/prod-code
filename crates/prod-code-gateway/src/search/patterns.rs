/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// A declaration this line starts: its kind and its name, or `None`.
///
/// Deliberately shallow. It recognises the shapes that carry a doc comment in the six
/// languages the gateway serves, and ignores everything else; a false negative costs a
/// missing hit, and the analyzer remains the authority on what a symbol actually is.
pub(crate) fn declaration_on(line: &str, language: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    fn strip<'a>(t: &'a str, prefixes: &[&str]) -> &'a str {
        let mut cur = t;
        loop {
            let mut moved = false;
            for p in prefixes {
                if let Some(rest) = cur.strip_prefix(p) {
                    cur = rest.trim_start();
                    moved = true;
                }
            }
            if !moved {
                return cur;
            }
        }
    }
    let ident = |s: &str| -> Option<String> {
        let name: String = s
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty() && !name.chars().next()?.is_numeric()).then_some(name)
    };
    let keyword = |t: &str, words: &[(&str, &str)]| -> Option<(String, String)> {
        for (word, kind) in words {
            if let Some(rest) = t.strip_prefix(*word)
                && rest.starts_with(|c: char| c.is_whitespace())
                && let Some(name) = ident(rest.trim_start())
            {
                return Some((kind.to_string(), name));
            }
        }
        None
    };
    match language {
        "rust" => {
            let t = strip(
                t,
                &[
                    "pub(crate)",
                    "pub(super)",
                    "pub",
                    "async",
                    "unsafe",
                    "const",
                    "default",
                ],
            );
            keyword(
                t,
                &[
                    ("fn", "function"),
                    ("struct", "struct"),
                    ("enum", "enum"),
                    ("trait", "trait"),
                    ("type", "type"),
                    ("static", "constant"),
                    ("macro_rules!", "macro"),
                    ("mod", "module"),
                ],
            )
            .or_else(|| {
                t.strip_prefix("impl").and_then(|rest| {
                    let body = rest.split_once(" for ").map(|(_, b)| b).unwrap_or(rest);
                    ident(body.trim_start().trim_start_matches('<'))
                        .map(|n| ("impl".to_string(), n))
                })
            })
        }
        "go" => {
            if let Some(rest) = t.strip_prefix("func")
                && rest.starts_with(|c: char| c.is_whitespace())
            {
                let rest_trim = rest.trim_start();
                if rest_trim.starts_with('(') {
                    if let Some((_, after)) = rest_trim.split_once(')')
                        && let Some(real) = ident(after.trim_start())
                    {
                        Some(("method".to_string(), real))
                    } else {
                        None
                    }
                } else {
                    ident(rest_trim).map(|name| ("function".to_string(), name))
                }
            } else {
                keyword(
                    t,
                    &[("type", "type"), ("const", "constant"), ("var", "variable")],
                )
            }
        }
        "python" => keyword(
            t,
            &[
                ("def", "function"),
                ("async def", "function"),
                ("class", "class"),
            ],
        )
        .or_else(|| {
            let t = strip(t, &["async"]);
            keyword(t, &[("def", "function")])
        }),
        "typescript" => {
            let t = strip(
                t,
                &[
                    "export",
                    "default",
                    "declare",
                    "abstract",
                    "async",
                    "public",
                    "private",
                    "protected",
                    "static",
                    "readonly",
                ],
            );
            keyword(
                t,
                &[
                    ("function", "function"),
                    ("class", "class"),
                    ("interface", "interface"),
                    ("type", "type"),
                    ("enum", "enum"),
                    ("const", "constant"),
                ],
            )
            .or_else(|| {
                let open = t.find('(')?;
                let before = t[..open].trim_end();
                let mut depth = 0usize;
                let mut close_idx = None;
                let mut in_single = false;
                let mut in_double = false;
                let mut in_backtick = false;
                let mut escaped = false;
                for (idx, ch) in t[open..].char_indices() {
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    if ch == '\\' {
                        escaped = true;
                        continue;
                    }
                    if in_single {
                        if ch == '\'' {
                            in_single = false;
                        }
                        continue;
                    }
                    if in_double {
                        if ch == '"' {
                            in_double = false;
                        }
                        continue;
                    }
                    if in_backtick {
                        if ch == '`' {
                            in_backtick = false;
                        }
                        continue;
                    }
                    match ch {
                        '\'' => in_single = true,
                        '"' => in_double = true,
                        '`' => in_backtick = true,
                        '(' => depth += 1,
                        ')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                close_idx = Some(open + idx);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let close = close_idx?;
                let after = t[close + 1..].trim_start();
                let has_return_type = after.strip_prefix(':').is_some_and(|ty| {
                    let ty = ty.trim();
                    !ty.is_empty() && (ty.ends_with(';') || ty.contains('{'))
                });
                let declaration_tail = after.starts_with('{') || has_return_type;
                (!before.is_empty()
                    && declaration_tail
                    && !before.contains(' ')
                    && !before.contains('.')
                    && !t.starts_with("//")
                    && !t.starts_with('*')
                    && !matches!(
                        before,
                        "if" | "for" | "while" | "switch" | "catch" | "return"
                    ))
                .then(|| ident(before))
                .flatten()
                .map(|n| ("method".to_string(), n))
            })
        }
        "swift" => {
            let t = strip(
                t,
                &[
                    "public",
                    "private",
                    "internal",
                    "fileprivate",
                    "open",
                    "final",
                    "static",
                    "override",
                    "@objc",
                ],
            );
            keyword(
                t,
                &[
                    ("func", "function"),
                    ("struct", "struct"),
                    ("class", "class"),
                    ("enum", "enum"),
                    ("protocol", "protocol"),
                    ("extension", "extension"),
                    ("let", "constant"),
                    ("var", "variable"),
                ],
            )
        }
        "cpp" => keyword(
            t,
            &[
                ("class", "class"),
                ("struct", "struct"),
                ("namespace", "module"),
                ("enum", "enum"),
            ],
        )
        .or_else(|| {
            // A definition line ending in `{` with a parameter list: `Type name(args) {`.
            let before = t.split_once('(')?.0.trim_end();
            (!before.is_empty() && t.contains('(') && !t.starts_with('#'))
                .then(|| ident(before.rsplit([' ', ':', '*', '&']).next()?))
                .flatten()
                .map(|n| ("function".to_string(), n))
        }),
        _ => None,
    }
}
