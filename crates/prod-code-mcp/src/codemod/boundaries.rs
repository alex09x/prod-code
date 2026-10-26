/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Where a codemod placeholder may stop: statement boundaries and the end of a receiver.

use super::types::{SourceToken, TokenKind};

/// `<` right after `::`: the start of a turbofish's generic arguments.
pub(super) fn opens_turbofish(tokens: &[SourceToken], at: usize) -> bool {
    at > 0
        && matches!(&tokens[at].kind, TokenKind::Punct(p) if p == "<")
        && matches!(&tokens[at - 1].kind, TokenKind::Punct(q) if q == "::")
}

/// Whether `tokens[at]`, outside every delimiter, is a binary operator, which no receiver of
/// a member access contains. The angles of a turbofish (`::<A, B>`) are not operators.
pub(super) fn ends_receiver(tokens: &[SourceToken], at: usize, turbofish: &mut usize) -> bool {
    let TokenKind::Punct(p) = &tokens[at].kind else {
        return false;
    };
    match p.as_str() {
        "<" if opens_turbofish(tokens, at) || *turbofish > 0 => {
            *turbofish += 1;
            false
        }
        ">" if *turbofish > 0 => {
            *turbofish -= 1;
            false
        }
        _ if *turbofish > 0 => false,
        op => matches!(
            op,
            "+" | "-"
                | "*"
                | "/"
                | "%"
                | "=="
                | "!="
                | "<"
                | ">"
                | "<="
                | ">="
                | "&&"
                | "||"
                | "&"
                | "|"
                | "^"
                | "<<"
                | ">>"
                | "**"
                | "??"
        ),
    }
}

pub fn is_boundary_token(kind: &TokenKind, next_expected: &TokenKind) -> bool {
    if kind == next_expected {
        return false;
    }
    match kind {
        TokenKind::Punct(p) if p == ";" => true,
        TokenKind::Punct(p) if p == ":" => true,
        TokenKind::Punct(p) if matches!(p.as_str(), "=" | ":=" | "+=" | "-=" | "*=" | "/=") => true,
        TokenKind::Punct(p) if p == "," => !matches!(next_expected, TokenKind::CloseDelim(_)),
        TokenKind::Ident(id) => matches!(
            id.as_str(),
            "let"
                | "var"
                | "const"
                | "return"
                | "fn"
                | "func"
                | "function"
                | "def"
                | "class"
                | "struct"
                | "enum"
                | "interface"
                | "import"
                | "export"
                | "package"
                | "if"
                | "else"
                | "for"
                | "while"
                | "switch"
                | "case"
                | "catch"
                | "throw"
                | "yield"
                | "defer"
                | "break"
                | "continue"
        ),
        _ => false,
    }
}
