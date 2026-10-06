/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::options::OutlineOptions;

/// A Protobuf file's package, services, messages, enums, methods, fields, and enum members as an outline.
pub fn protobuf_outline(text: &str, path: &str, options: &OutlineOptions) -> String {
    let mut sanitized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut escape = false;

    while let Some(c) = chars.next() {
        if in_line_comment {
            if c == '\n' {
                in_line_comment = false;
                sanitized.push('\n');
            } else {
                sanitized.push(' ');
            }
        } else if in_block_comment {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block_comment = false;
                sanitized.push(' ');
                sanitized.push(' ');
            } else if c == '\n' {
                sanitized.push('\n');
            } else {
                sanitized.push(' ');
            }
        } else if in_double_quote {
            if escape {
                escape = false;
                sanitized.push(if c == '\n' { '\n' } else { ' ' });
            } else if c == '\\' {
                escape = true;
                sanitized.push(' ');
            } else if c == '"' {
                in_double_quote = false;
                sanitized.push(' ');
            } else if c == '\n' {
                sanitized.push('\n');
            } else {
                sanitized.push(' ');
            }
        } else if in_single_quote {
            if escape {
                escape = false;
                sanitized.push(if c == '\n' { '\n' } else { ' ' });
            } else if c == '\\' {
                escape = true;
                sanitized.push(' ');
            } else if c == '\'' {
                in_single_quote = false;
                sanitized.push(' ');
            } else if c == '\n' {
                sanitized.push('\n');
            } else {
                sanitized.push(' ');
            }
        } else {
            if c == '/' && chars.peek() == Some(&'/') {
                chars.next();
                in_line_comment = true;
                sanitized.push(' ');
                sanitized.push(' ');
            } else if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                in_block_comment = true;
                sanitized.push(' ');
                sanitized.push(' ');
            } else if c == '"' {
                in_double_quote = true;
                sanitized.push(' ');
            } else if c == '\'' {
                in_single_quote = true;
                sanitized.push(' ');
            } else {
                sanitized.push(c);
            }
        }
    }

    struct Token {
        line: usize,
        text: String,
    }

    let mut tokens: Vec<Token> = Vec::new();
    for (line_idx, line) in sanitized.lines().enumerate() {
        let line_num = line_idx + 1;
        let mut char_indices = line.char_indices().peekable();
        while let Some((_, c)) = char_indices.next() {
            if c.is_whitespace() {
                continue;
            }
            if matches!(c, '{' | '}' | ';' | '=' | '(' | ')') {
                tokens.push(Token {
                    line: line_num,
                    text: c.to_string(),
                });
                continue;
            }
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                let mut tok = String::new();
                tok.push(c);
                while let Some(&(_, next_c)) = char_indices.peek() {
                    if next_c.is_ascii_alphanumeric() || next_c == '_' || next_c == '.' {
                        tok.push(next_c);
                        char_indices.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token {
                    line: line_num,
                    text: tok,
                });
            }
        }
    }

    #[derive(Copy, Clone, PartialEq, Eq)]
    enum Container {
        Message,
        Enum,
        Service,
        Other,
    }

    let mut stack: Vec<Container> = Vec::new();
    let mut declarations: Vec<(usize, &'static str, String, usize)> = Vec::new();

    let mut idx = 0;
    let n = tokens.len();
    while idx < n {
        let tok = &tokens[idx];
        let current_depth = stack.len() + 1;

        if matches!(
            tok.text.as_str(),
            "option" | "reserved" | "extensions" | "syntax" | "import"
        ) {
            idx += 1;
            let mut nested = 0usize;
            while idx < n {
                match tokens[idx].text.as_str() {
                    "{" | "(" => nested += 1,
                    "}" | ")" if nested > 0 => nested -= 1,
                    "}" => break,
                    ";" if nested == 0 => {
                        idx += 1;
                        break;
                    }
                    _ => {}
                }
                idx += 1;
            }
            continue;
        }

        if tok.text == "{" {
            stack.push(Container::Other);
            idx += 1;
            continue;
        } else if tok.text == "}" {
            stack.pop();
            idx += 1;
            continue;
        } else if tok.text == "package" {
            if idx + 1 < n {
                let pkg_name = &tokens[idx + 1].text;
                if pkg_name != ";" && pkg_name != "{" {
                    declarations.push((current_depth, "Package", pkg_name.clone(), tok.line));
                }
                idx += 2;
                while idx < n && tokens[idx].text != ";" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == ";" {
                    idx += 1;
                }
                continue;
            }
        } else if tok.text == "message" {
            if idx + 1 < n {
                let name = tokens[idx + 1].text.clone();
                declarations.push((current_depth, "Message", name, tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != "{" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == "{" {
                    stack.push(Container::Message);
                    idx += 1;
                }
                continue;
            }
        } else if tok.text == "enum" {
            if idx + 1 < n {
                let name = tokens[idx + 1].text.clone();
                declarations.push((current_depth, "Enum", name, tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != "{" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == "{" {
                    stack.push(Container::Enum);
                    idx += 1;
                }
                continue;
            }
        } else if tok.text == "service" {
            if idx + 1 < n {
                let name = tokens[idx + 1].text.clone();
                declarations.push((current_depth, "Service", name, tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != "{" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == "{" {
                    stack.push(Container::Service);
                    idx += 1;
                }
                continue;
            }
        } else if tok.text == "rpc" && stack.last() == Some(&Container::Service) {
            if idx + 1 < n {
                let name = tokens[idx + 1].text.clone();
                declarations.push((current_depth, "Method", name, tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != ";" && tokens[idx].text != "{" {
                    idx += 1;
                }
                if idx < n {
                    if tokens[idx].text == "{" {
                        stack.push(Container::Other);
                    }
                    idx += 1;
                }
                continue;
            }
        } else if stack.last() == Some(&Container::Enum) {
            if !matches!(tok.text.as_str(), "option" | "reserved")
                && idx + 1 < n
                && tokens[idx + 1].text == "="
            {
                declarations.push((current_depth, "EnumMember", tok.text.clone(), tok.line));
                idx += 2;
                while idx < n && tokens[idx].text != ";" && tokens[idx].text != "}" {
                    idx += 1;
                }
                if idx < n && tokens[idx].text == ";" {
                    idx += 1;
                }
                continue;
            }
        } else if stack.last() == Some(&Container::Message) {
            if tok.text == "oneof" {
                if idx + 1 < n {
                    let name = tokens[idx + 1].text.clone();
                    declarations.push((current_depth, "Field", name, tok.line));
                    idx += 2;
                    while idx < n && tokens[idx].text != "{" {
                        idx += 1;
                    }
                    if idx < n && tokens[idx].text == "{" {
                        stack.push(Container::Message);
                        idx += 1;
                    }
                    continue;
                }
            } else if !matches!(
                tok.text.as_str(),
                "option" | "reserved" | "extensions" | "syntax" | "import"
            ) {
                let mut scan = idx;
                let mut found_eq = false;
                while scan < n && !matches!(tokens[scan].text.as_str(), ";" | "{" | "}") {
                    if tokens[scan].text == "=" {
                        found_eq = true;
                        break;
                    }
                    scan += 1;
                }
                if found_eq && scan > idx {
                    let field_name = tokens[scan - 1].text.clone();
                    let field_line = tokens[scan - 1].line;
                    declarations.push((current_depth, "Field", field_name, field_line));
                    idx = scan + 1;
                    while idx < n && !matches!(tokens[idx].text.as_str(), ";" | "}") {
                        idx += 1;
                    }
                    if idx < n && tokens[idx].text == ";" {
                        idx += 1;
                    }
                    continue;
                }
            }
        }

        idx += 1;
    }

    let mut out = format!("Outline for {path}:\n");
    let mut count = 0usize;
    for (depth, kind, name, line) in declarations {
        if depth > options.max_depth {
            continue;
        }
        if let Some(kinds) = &options.kinds {
            let matched = kinds.iter().any(|k| {
                k.eq_ignore_ascii_case(kind)
                    || (kind == "Message"
                        && (k.eq_ignore_ascii_case("struct") || k.eq_ignore_ascii_case("class")))
                    || (kind == "Service" && k.eq_ignore_ascii_case("interface"))
                    || (kind == "Method"
                        && (k.eq_ignore_ascii_case("function") || k.eq_ignore_ascii_case("rpc")))
                    || (kind == "Package"
                        && (k.eq_ignore_ascii_case("module")
                            || k.eq_ignore_ascii_case("namespace")))
                    || (kind == "EnumMember"
                        && (k.eq_ignore_ascii_case("member") || k.eq_ignore_ascii_case("constant")))
                    || (kind == "Field"
                        && (k.eq_ignore_ascii_case("property")
                            || k.eq_ignore_ascii_case("variable")))
            });
            if !matched {
                continue;
            }
        }
        count += 1;
        out.push_str(&format!("  [{kind}] {name} (line {line})\n"));
    }
    if count == 0 {
        out.push_str("  (no declarations)");
    }
    out.trim_end().to_string()
}
