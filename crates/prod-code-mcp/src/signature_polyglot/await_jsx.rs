/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// JSX tags and text are opaque to the JavaScript operand lexer. Expressions
/// inside braces return to that lexer, including nested JSX in an expression.
#[derive(Default)]
pub struct Context {
    frames: Vec<Frame>,
}

#[derive(Default)]
struct Frame {
    elements: usize,
    tag: bool,
    closing: bool,
    expression: Option<usize>,
}

impl Context {
    pub fn expression_start(&self, depth: usize) -> bool {
        self.frames
            .last()
            .is_some_and(|frame| frame.expression == Some(depth + 1))
    }

    pub fn start(&mut self, rest: &str) -> bool {
        let next = rest.chars().nth(1);
        if !next.is_some_and(|c| c == '>' || c.is_alphabetic() || matches!(c, '_' | '$')) {
            return false;
        }
        if generic_arrow(rest) {
            return false;
        }
        self.frames.push(Frame::default());
        true
    }

    pub fn skip(&mut self, content: &str, at: usize, depth: usize) -> Option<usize> {
        let frame = self.frames.last_mut()?;
        if let Some(expression) = frame.expression {
            if depth >= expression {
                return None;
            }
            frame.expression = None;
        }
        let bytes = content.as_bytes();
        if bytes[at] == b'{' {
            frame.expression = Some(depth + 1);
            return None;
        }
        let mut end = at;
        if !frame.tag {
            if bytes[at] != b'<' {
                while end < bytes.len() && !matches!(bytes[end], b'<' | b'{') {
                    end += 1;
                }
                return Some(end);
            }
            frame.tag = true;
            frame.closing = bytes.get(at + 1) == Some(&b'/');
        }
        while end < bytes.len() {
            match bytes[end] {
                b'{' => break,
                b'\'' | b'"' => {
                    let quote = bytes[end];
                    end += 1;
                    while end < bytes.len() && bytes[end] != quote {
                        end += 1;
                    }
                    end = (end + 1).min(bytes.len());
                }
                b'>' => {
                    let self_closing = end > 0 && bytes[end - 1] == b'/';
                    if frame.closing {
                        frame.elements = frame.elements.saturating_sub(1);
                    } else if !self_closing {
                        frame.elements += 1;
                    }
                    frame.tag = false;
                    end += 1;
                    if frame.elements == 0 {
                        self.frames.pop();
                    }
                    break;
                }
                _ => end += 1,
            }
        }
        Some(end)
    }
}

/// TSX generic arrows share JSX's opening `<`. A comma or default after the
/// first type parameter cannot be a tag. Both `extends` and a plain closing `>`
/// need a callable head: the former can be a JSX attribute, while the latter
/// can introduce an unconstrained generic function type. Leave these generics
/// in the JS lexer without consuming actual JSX tags or text.
fn generic_arrow(rest: &str) -> bool {
    let Some(rest) = rest.strip_prefix('<').and_then(trivia) else {
        return false;
    };
    let (first, mut tail) = word(rest);
    if first == "const" {
        let Some(rest) = trivia(tail) else {
            return false;
        };
        (_, tail) = word(rest);
    }
    let Some(tail) = trivia(tail) else {
        return false;
    };
    if tail.starts_with([',', '=']) {
        return true;
    }
    let (constraint, _) = word(tail);
    if constraint != "extends" && !tail.starts_with('>') {
        return false;
    }
    constrained_arrow(rest)
}

fn constrained_arrow(parameters: &str) -> bool {
    let Some(after_types) = balanced(parameters, b'>').and_then(trivia) else {
        return false;
    };
    let Some(parameters) = after_types.strip_prefix('(') else {
        return false;
    };
    let Some(mut tail) = balanced(parameters, b')').and_then(trivia) else {
        return false;
    };
    if let Some(result) = tail.strip_prefix(':') {
        tail = result;
        loop {
            let Some(rest) = trivia(tail) else {
                return false;
            };
            tail = rest;
            if tail.starts_with("=>") {
                break;
            }
            if tail.is_empty() || tail.starts_with(';') || tail.starts_with("</") {
                return false;
            }
            let byte = tail.as_bytes()[0];
            if matches!(byte, b'(' | b'[' | b'{' | b'<') {
                let close = match byte {
                    b'(' => b')',
                    b'[' => b']',
                    b'{' => b'}',
                    _ => b'>',
                };
                let Some(rest) = balanced(&tail[1..], close) else {
                    return false;
                };
                tail = rest;
            } else {
                tail = &tail[tail.chars().next().unwrap().len_utf8()..];
            }
        }
    }
    tail.starts_with("=>")
}

fn word(rest: &str) -> (&str, &str) {
    let end = rest
        .find(|c| !super::syntax::is_ident(c) && c != '$')
        .unwrap_or(rest.len());
    rest.split_at(end)
}

fn trivia(mut rest: &str) -> Option<&str> {
    loop {
        rest = rest.trim_start();
        if let Some(comment) = rest.strip_prefix("/*") {
            rest = &comment[comment.find("*/")? + 2..];
        } else if let Some(comment) = rest.strip_prefix("//") {
            rest = &comment[comment.find('\n')?..];
        } else {
            return Some(rest);
        }
    }
}

/// Scan a type/parameter group, without interpreting delimiters in literal
/// types or comments. Angle brackets nest only at the type-list level; arrows
/// in a function constraint do not close the type list.
fn balanced(mut rest: &str, close: u8) -> Option<&str> {
    let mut stack = vec![close];
    loop {
        rest = trivia(rest)?;
        let byte = *rest.as_bytes().first()?;
        if matches!(byte, b'\'' | b'"' | b'`') {
            let mut end = 1;
            while *rest.as_bytes().get(end)? != byte {
                end += if rest.as_bytes()[end] == b'\\' { 2 } else { 1 };
            }
            rest = &rest[end + 1..];
            continue;
        }
        if rest.starts_with("=>") {
            rest = &rest[2..];
            continue;
        }
        if Some(&byte) == stack.last() {
            stack.pop();
            if stack.is_empty() {
                return Some(&rest[1..]);
            }
        } else {
            match byte {
                b'(' => stack.push(b')'),
                b'[' => stack.push(b']'),
                b'{' => stack.push(b'}'),
                b'<' if stack.last() == Some(&b'>') => stack.push(b'>'),
                b';' if stack.len() == 1 && close == b'>' => return None,
                _ => {}
            }
        }
        rest = &rest[rest.chars().next()?.len_utf8()..];
    }
}
