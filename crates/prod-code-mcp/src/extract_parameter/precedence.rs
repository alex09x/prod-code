/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::Syntax;

pub(super) fn unary(s: &str, syntax: Syntax) -> Option<u8> {
    if syntax == Syntax::Python {
        return match s {
            "not" => Some(4),
            "+" | "-" | "~" => Some(12),
            "await" => Some(14),
            _ => None,
        };
    }
    // C++'s alternative spelling has the same unary binding power as `!`.
    if syntax == Syntax::Cpp && s == "not" {
        return Some(14);
    }
    matches!(s, "!" | "~" | "-" | "+" | "&" | "*" | "await").then_some(14)
}

/// Binding powers follow each language's grammar, not surface token similarity.
pub(super) fn precedence(s: &str, syntax: Syntax) -> Option<(u8, u8)> {
    let n = match syntax {
        // Go groups shifts/AND with multiplication and OR/XOR with addition.
        Syntax::Go => match s {
            "=" | ":=" | "+=" | "-=" | "*=" | "/=" => return Some((1, 1)),
            "||" => 2,
            "&&" => 3,
            "==" | "!=" | "<" | ">" | "<=" | ">=" => 4,
            "+" | "-" | "|" | "^" => 5,
            "*" | "/" | "%" | "<<" | ">>" | "&" | "&^" => 6,
            _ => return None,
        },
        // Rust bitwise operators bind more tightly than all comparisons.
        Syntax::Rust => match s {
            "=" | "+=" | "-=" | "*=" | "/=" => return Some((1, 1)),
            ".." | "..=" => 2,
            "||" => 3,
            "&&" => 4,
            "==" | "!=" | "<" | ">" | "<=" | ">=" => 5,
            "|" => 6,
            "^" => 7,
            "&" => 8,
            "<<" | ">>" => 9,
            "+" | "-" => 10,
            "*" | "/" | "%" => 11,
            _ => return None,
        },
        // Python comparisons share one chain level below bitwise operators.
        // `not` here is only the first token of `not in`; unary not binds below comparisons.
        Syntax::Python => match s {
            "or" => 2,
            "and" => 3,
            "==" | "!=" | "<" | ">" | "<=" | ">=" | "in" | "is" | "not" => 5,
            "|" => 6,
            "^" => 7,
            "&" => 8,
            "<<" | ">>" => 9,
            "+" | "-" => 10,
            "*" | "/" | "//" | "%" | "@" => 11,
            "**" => return Some((13, 13)),
            _ => return None,
        },
        Syntax::JavaScript | Syntax::TypeScript | Syntax::C | Syntax::Cpp | Syntax::Swift => {
            match s {
                "=" | ":=" | "+=" | "-=" | "*=" | "/=" => return Some((1, 1)),
                "||" | "or" | "??" => 2,
                "&&" | "and" => 3,
                "|" => 4,
                "^" => 5,
                "&" => 6,
                "==" | "!=" | "===" | "!==" => 7,
                "<" | ">" | "<=" | ">=" | "in" | "is" => 8,
                ".." | "..=" => 9,
                "<<" | ">>" => 10,
                "+" | "-" => 11,
                "*" | "/" | "%" | "//" => 12,
                "**" => return Some((13, 13)),
                _ => return None,
            }
        }
    };
    Some((n, n + 1))
}
