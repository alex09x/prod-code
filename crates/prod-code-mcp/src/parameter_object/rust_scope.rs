/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::drop::rust_code;

pub(super) struct Scope {
    pub insertion: usize,
    pub modules: Vec<String>,
}

struct Frame {
    delimiter: u8,
    boundary: usize,
    module: Option<String>,
    header: Header,
    angles: usize,
    completes_item: bool,
    inner_attribute: bool,
    header_macro: bool,
}

/// Only declaration introducers select a body. In particular, `mod` inside a type path is
/// not a module declaration, and const/static/type initializers end at their semicolon.
#[derive(Default)]
enum Header {
    #[default]
    Start,
    Const,
    Extern,
    Module,
    NamedModule(String),
    Braced,
    BlockExpression,
    AfterExpressionBlock,
    MacroPath,
    Semicolon,
}

impl Header {
    fn word(&mut self, word: &str) {
        if matches!(self, Self::AfterExpressionBlock) {
            // An expression-with-block can be a statement without `;`. Its next
            // token may instead continue a condition or `else` chain, so keep
            // expression context unless an item declaration actually starts.
            *self = match word {
                "pub" | "unsafe" | "safe" | "async" | "default" | "auto" | "const" | "extern"
                | "mod" | "fn" | "impl" | "trait" | "struct" | "enum" | "union" | "macro"
                | "type" | "use" | "static" => Self::Start,
                _ => Self::BlockExpression,
            };
        }
        *self = match (&*self, word) {
            (Self::Start, "pub" | "unsafe" | "safe" | "async" | "default" | "auto") => Self::Start,
            (Self::Start, "const") => Self::Const,
            (Self::Const, "unsafe" | "async") => Self::Const,
            (Self::Start | Self::Const, "extern") => Self::Extern,
            (Self::Start, "mod") => Self::Module,
            (Self::Module, name) => Self::NamedModule(name.to_owned()),
            (Self::Start | Self::Const | Self::Extern, "fn")
            | (Self::Start, "impl" | "trait" | "struct" | "enum" | "union" | "macro") => {
                Self::Braced
            }
            (Self::Extern, "crate")
            | (Self::Start, "type" | "use" | "static")
            | (Self::Const, _) => Self::Semicolon,
            (Self::Start, "if" | "else" | "match" | "while" | "for" | "loop") => {
                Self::BlockExpression
            }
            (Self::Start, _) => Self::MacroPath,
            _ => return,
        };
    }

    fn body(&self) -> bool {
        matches!(self, Self::Braced | Self::NamedModule(_) | Self::Extern)
    }
}

impl Frame {
    fn new(delimiter: u8, boundary: usize, module: Option<String>) -> Self {
        Self {
            delimiter,
            boundary,
            module,
            header: Header::Start,
            angles: 0,
            completes_item: false,
            inner_attribute: false,
            header_macro: false,
        }
    }

    fn finish(&mut self, end: usize) {
        self.boundary = end;
        self.header = Header::Start;
        self.angles = 0;
        self.header_macro = false;
    }
}

/// Consume a complete Rust identifier, retaining raw prefixes and Unicode XID characters.
fn identifier(text: &str, at: usize) -> Option<usize> {
    let start = at + usize::from(text[at..].starts_with("r#")) * 2;
    let mut chars = text[start..].char_indices();
    let (_, first) = chars.next()?;
    if first != '_' && !unicode_ident::is_xid_start(first) {
        return None;
    }
    Some(
        chars
            .find(|(_, ch)| *ch != '_' && !unicode_ident::is_xid_continue(*ch))
            .map_or(text.len(), |(offset, _)| start + offset),
    )
}

/// Rust permits a BOM followed by a shebang, but `#!` followed by an attribute's
/// opening bracket (ignoring trivia) is Rust syntax. Skip interpreter text before
/// masking literals too: its quotes and braces need not be valid Rust tokens.
fn preamble_end(text: &str) -> usize {
    let bom = usize::from(text.starts_with('\u{feff}')) * '\u{feff}'.len_utf8();
    let rest = &text[bom..];
    if !rest.starts_with("#!") {
        return bom;
    }
    let masked = rust_code(&rest[2..]);
    let after_bang = std::str::from_utf8(&masked).expect("masking preserves UTF-8");
    if after_bang
        .trim_start_matches(rust_whitespace)
        .starts_with('[')
    {
        return bom;
    }
    bom + rest.find('\n').map_or(rest.len(), |end| end + 1)
}

/// Rust's lexical Pattern_White_Space includes directional marks that
/// `char::is_whitespace` excludes, and excludes other Unicode space characters.
fn rust_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\u{9}'..='\u{d}' | ' ' | '\u{85}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}'
    )
}

/// Walk item headers separately from balanced expression/attribute/parameter token trees.
/// A body only completes its owning item; a brace within `<...>` cannot complete that item.
fn frames_at(text: &str, at: usize) -> Vec<Frame> {
    let preamble = preamble_end(text);
    let mut masked = vec![b' '; preamble];
    masked.extend(rust_code(&text[preamble..]));
    let code = std::str::from_utf8(&masked).expect("masking preserves UTF-8");
    let bytes = code.as_bytes();
    let mut frames = vec![Frame::new(b'{', preamble, Some(String::new()))];
    let mut i = preamble;
    while i < at {
        let nested = frames.len() > 1;
        let parent = frames.last_mut().unwrap();
        if let Some(end) = identifier(code, i) {
            if parent.delimiter == b'{' && parent.angles == 0 {
                parent.header.word(&code[i..end]);
            }
            i = end;
            continue;
        }
        match bytes[i] {
            b'(' | b'[' | b'{' => {
                // A type macro owns its next token tree, regardless of delimiter.
                // Keep the surrounding item header until its actual body closes.
                let header_macro = std::mem::take(&mut parent.header_macro);
                if bytes[i] == b'{' && matches!(parent.header, Header::Const) {
                    // `const { ... }` is an expression, whereas `const NAME = ...`
                    // already selected Semicolon and `const fn` selected Braced.
                    parent.header = Header::BlockExpression;
                }
                let completes_item = bytes[i] == b'{'
                    && !header_macro
                    && parent.delimiter == b'{'
                    && parent.angles == 0
                    && parent.header.body();
                let module = if completes_item {
                    match &parent.header {
                        Header::NamedModule(name) => Some(name.clone()),
                        _ => None,
                    }
                } else {
                    None
                };
                let inner_attribute = bytes[i] == b'['
                    && parent.delimiter == b'{'
                    && matches!(parent.header, Header::Start)
                    && code[parent.boundary..i]
                        .chars()
                        .filter(|ch| !rust_whitespace(*ch))
                        .eq("#!".chars());
                let mut frame = Frame::new(bytes[i], i + 1, module);
                frame.completes_item = completes_item;
                frame.inner_attribute = inner_attribute;
                frames.push(frame);
            }
            b')' | b']' | b'}' if nested => {
                let closed = frames.pop().unwrap();
                let parent = frames.last_mut().unwrap();
                if closed.completes_item || closed.inner_attribute {
                    parent.finish(i + 1);
                } else if closed.delimiter == b'{'
                    && parent.delimiter == b'{'
                    && matches!(
                        parent.header,
                        Header::BlockExpression | Header::AfterExpressionBlock
                    )
                {
                    parent.header = Header::AfterExpressionBlock;
                }
            }
            b'<' if parent.delimiter == b'{' && matches!(parent.header, Header::Braced) => {
                parent.angles += 1;
            }
            b'>' if parent.delimiter == b'{' && bytes.get(i.wrapping_sub(1)) != Some(&b'-') => {
                parent.angles = parent.angles.saturating_sub(1);
            }
            b'!' if parent.delimiter == b'{' => {
                if matches!(parent.header, Header::MacroPath) {
                    // A braced macro invocation at item level completes that item.
                    parent.header = Header::Braced;
                } else if matches!(parent.header, Header::Braced) {
                    // A macro bang follows its path's identifier. The never type
                    // in `-> !` must leave the next brace as the function body.
                    parent.header_macro = code[..i]
                        .trim_end_matches(rust_whitespace)
                        .chars()
                        .next_back()
                        .is_some_and(unicode_ident::is_xid_continue);
                }
            }
            b'\'' if parent.delimiter == b'{' && matches!(parent.header, Header::Start) => {
                // A label starts a loop or block expression. Function lifetimes
                // are inside their item's header or a parenthesized token tree.
                parent.header = Header::BlockExpression;
            }
            b';' if parent.delimiter == b'{' && parent.angles == 0 => {
                parent.finish(i + 1);
            }
            _ => {}
        }
        // Identifiers and masked literals preserve byte offsets. Unrecognized
        // Unicode (including Rust whitespace and a BOM) still spans whole chars.
        i += code[i..].chars().next().unwrap().len_utf8();
    }
    frames
}

/// Find the item in the nearest enclosing module, keeping its outer docs/attributes attached.
pub(super) fn scope(text: &str, at: usize) -> Scope {
    let frames = frames_at(text, at);
    let module = frames.iter().rev().find(|f| f.module.is_some()).unwrap();
    let mut insertion = module.boundary;
    let mut scan = insertion;
    // Inner docs belong to the module itself, even after ordinary preamble comments.
    // Keep outer docs with their item; only an inner doc advances the insertion boundary.
    loop {
        let rest = &text[scan..];
        let start = scan + rest.len() - rest.trim_start_matches(rust_whitespace).len();
        let inner = text[start..].starts_with("//!") || text[start..].starts_with("/*!");
        if text[start..].starts_with("//") {
            scan = text[start..]
                .find('\n')
                .map_or(text.len(), |n| start + n + 1);
        } else if text[start..].starts_with("/*") {
            let (mut end, mut depth) = (start + 2, 1);
            while end < text.len() && depth > 0 {
                if text.as_bytes()[end..].starts_with(b"/*") {
                    depth += 1;
                    end += 2;
                } else if text.as_bytes()[end..].starts_with(b"*/") {
                    depth -= 1;
                    end += 2;
                } else {
                    end += 1;
                }
            }
            scan = end;
        } else {
            break;
        }
        if inner {
            insertion = scan;
        }
    }
    Scope {
        insertion,
        modules: frames
            .iter()
            .filter_map(|f| f.module.as_ref())
            .filter(|m| !m.is_empty())
            .cloned()
            .collect(),
    }
}

/// A same-file caller may be outside the inline module that owns the new type.
pub(super) fn spelling(home: &[String], caller: &[String], name: &str) -> String {
    let common = home.iter().zip(caller).take_while(|(a, b)| a == b).count();
    std::iter::repeat_n("super", caller.len() - common)
        .chain(home[common..].iter().map(String::as_str))
        .chain(std::iter::once(name))
        .collect::<Vec<_>>()
        .join("::")
}
