/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Rust source bytes that are code, and bytes that are whitespace or comments between tokens.
/// The signature refactoring lexer already has the repository's handling for nested comments,
/// raw strings, characters, and lifetimes; build the two masks from that rather than teaching
/// test selection a second comment grammar.
struct RustLex {
    code: Vec<bool>,
    trivia: Vec<bool>,
}

fn rust_literal_end(text: &str, i: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let raw = match bytes[i] {
        b'r' if i == 0 || !ident(bytes[i - 1]) => Some(i),
        b'b' | b'c' if bytes.get(i + 1) == Some(&b'r') && (i == 0 || !ident(bytes[i - 1])) => {
            Some(i + 1)
        }
        _ => None,
    };
    if let Some(raw) = raw {
        let mut quote = raw + 1;
        while bytes.get(quote) == Some(&b'#') {
            quote += 1;
        }
        if bytes.get(quote) == Some(&b'"') {
            let hashes = quote - raw - 1;
            let mut j = quote + 1;
            while j < bytes.len() {
                if bytes[j] == b'"'
                    && bytes
                        .get(j + 1..j + 1 + hashes)
                        .is_some_and(|tail| tail.iter().all(|b| *b == b'#'))
                {
                    return Some(j + 1 + hashes);
                }
                j += 1;
            }
            return None;
        }
    }
    match bytes[i] {
        b'"' => {
            let mut j = i + 1;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' => j += 2,
                    b'"' => return Some(j + 1),
                    _ => j += 1,
                }
            }
            None
        }
        // One scalar or one escape followed immediately by a quote is a character literal.
        // Otherwise this is a lifetime or label, and its apostrophe remains code.
        b'\'' => {
            if bytes.get(i + 1) == Some(&b'\\') {
                return text.get(i + 2..)?.find('\'').map(|n| i + 3 + n);
            }
            let len = text.get(i + 1..)?.chars().next()?.len_utf8();
            (bytes.get(i + 1 + len) == Some(&b'\'')).then_some(i + 2 + len)
        }
        _ => None,
    }
}

fn rust_code(text: &str) -> std::result::Result<RustLex, String> {
    let without_comments = crate::signature::blank_comments(text).ok_or_else(|| {
        "the Rust source has an unterminated string, character, or comment".to_string()
    })?;
    let bytes = text.as_bytes();
    let blanked = without_comments.as_bytes();
    let mut code: Vec<bool> = bytes
        .iter()
        .zip(blanked)
        .map(|(original, blank)| original == blank)
        .collect();
    let trivia: Vec<bool> = bytes
        .iter()
        .zip(blanked)
        .map(|(original, blank)| original.is_ascii_whitespace() || original != blank)
        .collect();
    let mut i = 0usize;
    while i < bytes.len() {
        if code[i]
            && let Some(end) = rust_literal_end(&without_comments, i)
        {
            code[i..end].fill(false);
            i = end;
        } else {
            i += 1;
        }
    }
    Ok(RustLex { code, trivia })
}

fn rust_skip_trivia(lex: &RustLex, mut i: usize, end: usize) -> usize {
    while i < end && lex.trivia[i] {
        i += 1;
    }
    i
}

/// The identifier starting at `at`, without an optional raw `r#` prefix, its end, and
/// whether it had that prefix.
fn rust_identifier(text: &str, at: usize, end: usize) -> Option<(&str, usize, bool)> {
    let mut start = at;
    let raw = text.get(at..end)?.starts_with("r#");
    if raw {
        start += 2;
    }
    let first = text.get(start..end)?.chars().next()?;
    if first != '_' && !unicode_ident::is_xid_start(first) {
        return None;
    }
    let mut finish = start + first.len_utf8();
    for ch in text[finish..end].chars() {
        if ch != '_' && !unicode_ident::is_xid_continue(ch) {
            break;
        }
        finish += ch.len_utf8();
    }
    Some((&text[start..finish], finish, raw))
}

fn rust_test_attribute(text: &str, lex: &RustLex, open: usize, close: usize) -> bool {
    rust_test_meta_attribute(text, lex, open + 1, close, false)
}

/// The test marker inside cfg_attr is conditional; without resolving Cargo's active cfg state,
/// selecting it could treat a test-context helper as runnable.
fn rust_test_attribute_is_conditional(
    text: &str,
    lex: &RustLex,
    open: usize,
    close: usize,
) -> bool {
    let start = open + 1;
    rust_test_meta_attribute(text, lex, start, close, true)
        && !rust_test_meta_attribute(text, lex, start, close, false)
}

/// Recognizes direct test attributes and optionally searches cfg_attr's attributes for tests.
fn rust_test_meta_attribute(
    text: &str,
    lex: &RustLex,
    start: usize,
    end: usize,
    allow_cfg_attr: bool,
) -> bool {
    let mut i = rust_skip_trivia(lex, start, end);
    let Some((mut last, mut cursor, _)) = rust_identifier(text, i, end) else {
        return false;
    };
    loop {
        i = rust_skip_trivia(lex, cursor, end);
        if text.as_bytes().get(i..i + 2) != Some(b"::") {
            break;
        }
        i = rust_skip_trivia(lex, i + 2, end);
        let Some((segment, next, _)) = rust_identifier(text, i, end) else {
            return false;
        };
        last = segment;
        cursor = next;
    }
    if matches!(last, "test" | "rstest" | "test_case") {
        return true;
    }
    if !allow_cfg_attr || last != "cfg_attr" {
        return false;
    }
    let args_open = rust_skip_trivia(lex, cursor, end);
    if text.as_bytes().get(args_open) != Some(&b'(')
        || !lex.code.get(args_open).copied().unwrap_or(false)
    {
        return false;
    }
    rust_cfg_attr_contains_test_attribute(text, lex, args_open, end)
}

/// cfg_attr has one condition followed by one or more attributes. Search the latter for a test
/// marker without assuming the condition is active for this build.
fn rust_cfg_attr_contains_test_attribute(
    text: &str,
    lex: &RustLex,
    open: usize,
    end: usize,
) -> bool {
    let mut depth = 1usize;
    let mut condition_seen = false;
    let mut attribute_start = None;
    let mut i = open + 1;
    while i < end {
        if lex.code[i] {
            match text.as_bytes()[i] {
                b'(' => depth += 1,
                b')' if depth == 1 => {
                    return attribute_start
                        .is_some_and(|start| rust_test_meta_attribute(text, lex, start, i, true));
                }
                b')' => depth -= 1,
                b',' if depth == 1 => {
                    if !condition_seen {
                        condition_seen = true;
                        attribute_start = Some(i + 1);
                    } else {
                        if attribute_start.is_some_and(|start| {
                            rust_test_meta_attribute(text, lex, start, i, true)
                        }) {
                            return true;
                        }
                        attribute_start = Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    false
}

pub(crate) fn rust_test_marker(
    text: &str,
    line: u32,
    name: &str,
) -> std::result::Result<Option<()>, String> {
    let lex = rust_code(text)?;
    if line == 0 {
        return Err(format!(
            "the Rust declaration for {name} has no source line"
        ));
    }
    let mut line_start = 0usize;
    for _ in 1..line {
        let Some(newline) = text[line_start..].find('\n') else {
            return Err(format!(
                "the Rust declaration for {name} at line {line} is outside the source"
            ));
        };
        line_start += newline + 1;
    }
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |n| line_start + n);
    let bare = name
        .split('(')
        .next()
        .unwrap_or(name)
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(name)
        .trim_start_matches("r#");
    let mut cursor = line_start;
    let mut declarations = Vec::new();
    while cursor < line_end {
        if !lex.code[cursor] {
            cursor += 1;
            continue;
        }
        if let Some((token, end, raw)) = rust_identifier(text, cursor, line_end) {
            if token == "fn" && !raw {
                let name_at = rust_skip_trivia(&lex, end, line_end);
                if let Some((declared, _, _)) = rust_identifier(text, name_at, line_end)
                    && declared == bare
                {
                    declarations.push(cursor);
                }
            }
            cursor = end;
        } else {
            cursor += text[cursor..].chars().next().map_or(1, char::len_utf8);
        }
    }
    let declaration = match declarations.as_slice() {
        [declaration] => *declaration,
        [] => {
            return Err(format!(
                "the Rust declaration for {name} at line {line} cannot be classified as a runnable test"
            ));
        }
        declarations => {
            return Err(format!(
                "the Rust declaration for {name} at line {line} is ambiguous: {} matching declarations share the line",
                declarations.len()
            ));
        }
    };
    let mut starts = vec![0usize];
    let mut braces = 0usize;
    let (mut squares, mut parens) = (0usize, 0usize);
    for i in 0..declaration {
        if !lex.code[i] {
            continue;
        }
        match text.as_bytes()[i] {
            b'[' => squares += 1,
            b']' if squares > 0 => squares -= 1,
            b']' => {
                return Err(format!(
                    "the Rust delimiters before {name} at line {line} do not balance"
                ));
            }
            b'(' => parens += 1,
            b')' if parens > 0 => parens -= 1,
            b')' => {
                return Err(format!(
                    "the Rust delimiters before {name} at line {line} do not balance"
                ));
            }
            b'{' if squares == 0 && parens == 0 => {
                braces += 1;
                if starts.len() <= braces {
                    starts.push(i + 1);
                } else {
                    starts[braces] = i + 1;
                }
            }
            b'}' if squares == 0 && parens == 0 && braces > 0 => {
                braces -= 1;
                starts[braces] = i + 1;
            }
            b'}' if squares == 0 && parens == 0 => {
                return Err(format!(
                    "the Rust braces before {name} at line {line} do not balance"
                ));
            }
            b';' if squares == 0 && parens == 0 => starts[braces] = i + 1,
            _ => {}
        }
    }
    if squares != 0 || parens != 0 {
        return Err(format!(
            "the Rust delimiters before {name} at line {line} do not balance"
        ));
    }
    let mut conditional_test = false;
    let mut i = starts[braces];
    while i + 1 < declaration {
        if lex.code[i] && text.as_bytes()[i] == b'#' {
            let open = rust_skip_trivia(&lex, i + 1, declaration);
            if text.as_bytes().get(open) != Some(&b'[') || !lex.code[open] {
                i += 1;
                continue;
            }
            let mut depth = 1usize;
            let mut j = open + 1;
            while j < declaration && depth > 0 {
                if lex.code[j] {
                    match text.as_bytes()[j] {
                        b'[' => depth += 1,
                        b']' => depth -= 1,
                        _ => {}
                    }
                }
                j += 1;
            }
            if depth != 0 {
                return Err(format!(
                    "the Rust attributes before {name} at line {line} do not close"
                ));
            }
            if rust_test_attribute(text, &lex, open, j - 1) {
                return Ok(Some(()));
            }
            if rust_test_attribute_is_conditional(text, &lex, open, j - 1) {
                conditional_test = true;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    if conditional_test {
        return Err(format!(
            "the Rust test attribute for {name} at line {line} is conditional and cannot be proven active"
        ));
    }
    Ok(None)
}
