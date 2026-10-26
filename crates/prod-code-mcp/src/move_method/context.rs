/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::syntax::{is_ident, opaque_token};

/// A raw identifier is one token, including when its spelling is an owner keyword.
#[derive(Clone, Copy)]
pub(super) struct Word<'a> {
    pub start: usize,
    pub end: usize,
    pub name: &'a str,
    pub raw: bool,
}

pub(super) fn word_at(text: &str, start: usize) -> Option<Word<'_>> {
    let c = text[start..].chars().next()?;
    if !is_ident(c) {
        return None;
    }
    let raw = text[start..].starts_with("r#");
    let name_at = start + if raw { 2 } else { 0 };
    let mut end = name_at;
    while end < text.len() && text[end..].starts_with(is_ident) {
        end += text[end..].chars().next()?.len_utf8();
    }
    Some(Word {
        start,
        end,
        name: &text[name_at..end],
        raw,
    })
}

impl Word<'_> {
    /// All rewrite/capture/ownership consumers make the same owner decision.
    pub(super) fn owner_end(self, text: &str) -> Option<usize> {
        if self.raw
            || !matches!(self.name, "impl" | "trait" | "mod")
            || type_header(text, self.start)
        {
            return None;
        }
        super::syntax::nested_owner_end(text, self.end, self.name)
    }
}

#[derive(Default)]
pub(super) struct TypeDelimiters(Vec<TypeFrame>);

struct TypeFrame {
    open: char,
    resume: bool,
    length: bool,
}

impl TypeDelimiters {
    pub(super) fn top(&self) -> Option<char> {
        self.0.last().map(|f| f.open)
    }
    pub(super) fn contains(&self, open: char) -> bool {
        self.0.iter().any(|f| f.open == open)
    }
    pub(super) fn array_length(&self) -> bool {
        self.0.iter().any(|f| f.length)
    }
    pub(super) fn start_length(&mut self) {
        if let Some(f) = self.0.last_mut()
            && f.open == '['
        {
            f.length = true;
        }
    }
    pub(super) fn push(&mut self, open: char, resume: bool) {
        self.0.push(TypeFrame {
            open,
            resume,
            length: false,
        });
    }
    pub(super) fn close(&mut self, open: char) -> Option<bool> {
        (self.top() == Some(open)).then(|| self.0.pop().unwrap().resume)
    }
}

/// Track a function's entire type/header region, including generic arguments,
/// references, tuples and where clauses. Braced const expressions temporarily
/// leave that region: items declared inside them still own their own Self.
pub(super) fn type_header(text: &str, at: usize) -> bool {
    let mut header = false;
    let mut delimiters = TypeDelimiters::default();
    let mut macros = MacroContext::default();
    let mut i = 0;
    while i < at {
        match opaque_token(text, i) {
            Ok(Some((end, comment))) => {
                if !comment {
                    macros.literal();
                }
                i = end;
                continue;
            }
            Err(()) => return false,
            Ok(None) => {}
        }
        let c = text[i..].chars().next().unwrap();
        if let Some(word) = word_at(text, i) {
            i = word.end;
            macros.word(word.name, word.raw);
            if !word.raw && !macros.in_macro() && word.name == "fn" {
                header = true;
            }
            continue;
        }
        let in_macro = macros.in_macro();
        macros.punctuation(c);
        // Macro tokens have no Rust header grammar before expansion. In particular,
        // a colon, arrow or `fn` there cannot turn a following impl into impl Trait.
        if in_macro || macros.in_macro() {
            i += c.len_utf8();
            continue;
        }
        match c {
            ':' if !text[..i].ends_with(':')
                && !text[i + 1..].starts_with(':')
                && (header || !delimiters.contains('{')) =>
            {
                // A field initializer's colon starts an expression. Only a
                // function header (or a standalone type fragment) supplies types.
                header = true;
            }
            '>' if text[..i].ends_with('-') => header = true,
            '(' => delimiters.push(c, false),
            // An array starts with a type, but its length (after the direct
            // semicolon) is an expression. Save the surrounding type context so
            // the closing bracket resumes tuples/generics containing impl Trait.
            '[' => delimiters.push(c, header),
            ';' if delimiters.top() == Some('[') => {
                delimiters.start_length();
                header = false;
            }
            '<' if header && !delimiters.array_length() => delimiters.push(c, false),
            '{' => {
                let resume = header && delimiters.top().is_some_and(|c| c != '{');
                delimiters.push('{', resume);
                header = false;
            }
            ')' | ']' | '>' | '}' => {
                let open = match c {
                    ')' => '(',
                    ']' => '[',
                    '>' => '<',
                    _ => '{',
                };
                if let Some(resume) = delimiters.close(open)
                    && (c == '}' || c == ']')
                {
                    header = resume;
                }
            }
            ';' | '=' if delimiters.top().is_none_or(|c| c == '{') => {
                header = false;
            }
            _ => {}
        }
        i += c.len_utf8();
    }
    header && !macros.in_macro()
}

#[derive(Clone, Copy)]
enum Pending {
    Invocation,
    DefinitionName,
    DefinitionBody,
}

/// A macro invocation requires a path immediately before ! and a token tree
/// immediately after it (comments/whitespace are trivia). macro_rules alone has
/// a declared name between ! and its body. Unary ! and the never type have neither.
#[derive(Default)]
pub(super) struct MacroContext {
    delimiters: Vec<bool>,
    path: bool,
    rules: bool,
    pending: Option<Pending>,
}

impl MacroContext {
    pub(super) fn in_macro(&self) -> bool {
        self.delimiters.last().copied().unwrap_or(false)
    }

    pub(super) fn word(&mut self, word: &str, raw: bool) {
        self.pending = match self.pending {
            Some(Pending::DefinitionName) => Some(Pending::DefinitionBody),
            _ => None,
        };
        self.rules = word == "macro_rules" && !raw;
        self.path = raw
            || !matches!(
                word,
                "as" | "async"
                    | "await"
                    | "break"
                    | "const"
                    | "continue"
                    | "else"
                    | "enum"
                    | "extern"
                    | "false"
                    | "fn"
                    | "for"
                    | "if"
                    | "impl"
                    | "in"
                    | "let"
                    | "loop"
                    | "match"
                    | "mod"
                    | "move"
                    | "mut"
                    | "pub"
                    | "ref"
                    | "return"
                    | "static"
                    | "struct"
                    | "trait"
                    | "true"
                    | "type"
                    | "unsafe"
                    | "use"
                    | "where"
                    | "while"
                    | "yield"
            );
    }

    pub(super) fn literal(&mut self) {
        self.path = false;
        self.rules = false;
        self.pending = None;
    }

    pub(super) fn punctuation(&mut self, c: char) {
        if c.is_whitespace() {
            return;
        }
        match c {
            '!' => {
                self.pending = if self.rules {
                    Some(Pending::DefinitionName)
                } else if self.path {
                    Some(Pending::Invocation)
                } else {
                    None
                };
            }
            '(' | '[' | '{' => {
                let invocation = matches!(
                    self.pending,
                    Some(Pending::Invocation | Pending::DefinitionBody)
                );
                self.delimiters.push(invocation || self.in_macro());
                self.pending = None;
            }
            ')' | ']' | '}' => {
                self.delimiters.pop();
                self.pending = None;
            }
            _ => self.pending = None,
        }
        self.path = false;
        self.rules = false;
    }
}
