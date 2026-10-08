/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::codemod::{TokenKind, matcher::newline_separates_expressions, tokenize_source_for_lang};
use crate::parameter_object::Language;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Associativity {
    Left,
    Right,
    None,
}

#[derive(Clone, Copy)]
struct Precedence {
    level: u8,
    associativity: Associativity,
}

pub(super) fn requires_complete_expression_boundaries(expression: &str, lang: Language) -> bool {
    let tokens = tokenize_source_for_lang(expression, Some(lang));
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::OpenDelim(_) => depth += 1,
            TokenKind::CloseDelim(_) => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ if operator_info(
                &token.kind,
                tokens.get(index.wrapping_sub(1)).map(|t| &t.kind),
                lang,
            )
            .is_some() =>
            {
                return true;
            }
            TokenKind::Punct(ref punctuation)
                if depth == 0 && !matches!(punctuation.as_str(), "." | "?." | "::" | "->") =>
            {
                // Unknown operators need delimiters to prove that a textual hit is complete.
                // Note: comma (",") at depth 0 defines compound tuple/sequence boundaries.
                return true;
            }
            _ => {}
        }
    }
    false
}

fn expression_has_top_level_comma(expression: &str, lang: Language) -> bool {
    let tokens = tokenize_source_for_lang(expression, Some(lang));
    let mut depth = 0usize;
    for token in &tokens {
        match token.kind {
            TokenKind::OpenDelim(_) => depth += 1,
            TokenKind::CloseDelim(_) => depth = depth.saturating_sub(1),
            TokenKind::Punct(ref p) if depth == 0 && p == "," => return true,
            _ => {}
        }
    }
    false
}

pub(super) fn has_complete_expression_boundaries(
    text: &str,
    start: usize,
    end: usize,
    expression: &str,
    lang: Language,
) -> bool {
    if start > end
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return false;
    }

    let has_top_level_comma = expression_has_top_level_comma(expression, lang);

    let before = text[..start].trim_end();
    let left_char = before.chars().next_back();
    let left_delimited = left_char.is_none_or(|c| {
        matches!(c, '(' | '[' | '{' | ':' | '=' | ';') || (c == ',' && !has_top_level_comma)
    }) || before.ends_with("=>")
        || ends_with_keyword(before, "return")
        || ends_with_keyword(before, "yield")
        || ends_with_keyword(before, "if")
        || ends_with_keyword(before, "elif")
        || ends_with_keyword(before, "while")
        || ends_with_keyword(before, "assert")
        || ends_with_keyword(before, "with");

    let after = text[end..].trim_start();
    let right_char = after.chars().next();
    let right_delimited = right_char.is_none_or(|c| {
        matches!(c, ')' | ']' | '}' | ';' | ':') || (c == ',' && !has_top_level_comma)
    });

    let tokens = tokenize_source_for_lang(text, Some(lang));
    let Some(start_token) = tokens.iter().position(|token| token.start_byte == start) else {
        return false;
    };
    let Some(end_token) = tokens
        .iter()
        .position(|token| token.end_byte == end)
        .map(|index| index + 1)
    else {
        return false;
    };

    let root = expression_root_precedence(expression, lang);
    let left_operator = start_token.checked_sub(1).and_then(|index| {
        operator_info(
            &tokens[index].kind,
            index.checked_sub(1).map(|previous| &tokens[previous].kind),
            lang,
        )
    });
    let right_operator = if end_token < tokens.len() {
        let token = &tokens[end_token];
        if matches!(token.kind, TokenKind::OpenDelim('(' | '[')) {
            Some(Precedence {
                level: 16,
                associativity: Associativity::Left,
            })
        } else {
            operator_info(
                &token.kind,
                end_token
                    .checked_sub(1)
                    .map(|previous| &tokens[previous].kind),
                lang,
            )
        }
    } else {
        None
    };
    let left_newline = left_operator.is_none()
        && (start_token == 0 || newline_separates_expressions(text, &tokens, start_token));
    let right_newline = right_operator.is_none()
        && (end_token >= tokens.len() || newline_separates_expressions(text, &tokens, end_token));

    let right_is_python_power = lang == Language::Python
        && tokens
            .get(end_token)
            .is_some_and(|token| matches!(&token.kind, TokenKind::Punct(p) if p == "**"));

    let left_by_precedence = root.zip(left_operator).is_some_and(|(root, context)| {
        context.level < root.level
            || (context.level == root.level && root.associativity == Associativity::Right)
    });
    let right_by_precedence = root.zip(right_operator).is_some_and(|(root, context)| {
        if right_is_python_power && root.level < 16 {
            // In Python, ** binds tighter to its left than unary prefix operators (-(a**b))
            // and binary operators (level <= 15). But primary/postfix expressions (level >= 16,
            // such as obj.value or fn()) are valid complete left operands of **.
            false
        } else {
            context.level < root.level
                || (context.level == root.level && root.associativity == Associativity::Left)
        }
    });

    (left_delimited || left_newline || left_by_precedence)
        && (right_delimited || right_newline || right_by_precedence)
}

fn expression_root_precedence(expression: &str, lang: Language) -> Option<Precedence> {
    let tokens = tokenize_source_for_lang(expression, Some(lang));
    let mut depth = 0usize;
    let mut root = None;
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::OpenDelim(delimiter) => {
                if depth == 0
                    && index > 0
                    && matches!(delimiter, '(' | '[')
                    && token_can_end_expression(&tokens[index - 1].kind)
                {
                    choose_root(
                        &mut root,
                        Precedence {
                            level: 16,
                            associativity: Associativity::Left,
                        },
                    );
                }
                depth += 1;
            }
            TokenKind::CloseDelim(_) => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ => {
                if let Some(operator) = operator_info(
                    &token.kind,
                    index.checked_sub(1).map(|previous| &tokens[previous].kind),
                    lang,
                ) {
                    choose_root(&mut root, operator);
                }
            }
        }
    }
    root
}

fn choose_root(root: &mut Option<Precedence>, candidate: Precedence) {
    match root {
        None => *root = Some(candidate),
        Some(existing) if candidate.level < existing.level => *root = Some(candidate),
        Some(existing)
            if candidate.level == existing.level
                && candidate.associativity == Associativity::Right =>
        {
            *root = Some(candidate)
        }
        Some(_) => {}
    }
}

fn operator_info(
    token: &TokenKind,
    previous: Option<&TokenKind>,
    lang: Language,
) -> Option<Precedence> {
    let left = |level| Precedence {
        level,
        associativity: Associativity::Left,
    };
    let right = |level| Precedence {
        level,
        associativity: Associativity::Right,
    };
    let none = |level| Precedence {
        level,
        associativity: Associativity::None,
    };
    match token {
        TokenKind::Punct(operator) => Some(match operator.as_str() {
            "," => none(0),
            "." | "?." | "::" | "->" => left(16),
            "=" | ":=" | "+=" | "-=" | "*=" | "/=" => right(1),
            "?" | ":" => right(2),
            "??" => right(3),
            "||" => left(4),
            "&&" => left(5),
            "|" => left(6),
            "^" => left(7),
            "&" => {
                if unary_position(previous, lang) {
                    right(15)
                } else {
                    left(8)
                }
            }
            "==" | "!=" => {
                if lang == Language::Python {
                    none(10)
                } else {
                    left(9)
                }
            }
            "<" | ">" | "<=" | ">=" => {
                if lang == Language::Python {
                    none(10)
                } else {
                    left(10)
                }
            }
            "<<" | ">>" => left(11),
            "+" | "-" => {
                if unary_position(previous, lang) {
                    right(15)
                } else {
                    left(12)
                }
            }
            "*" => {
                if unary_position(previous, lang) {
                    right(15)
                } else {
                    left(13)
                }
            }
            "/" | "%" | "//" => left(13),
            "**" => right(14),
            "!" | "~" if unary_position(previous, lang) => right(15),
            _ => return None,
        }),
        TokenKind::Ident(operator) => Some(match (lang, operator.as_str()) {
            (Language::Python, "if") if !unary_position(previous, Language::Python) => right(3),
            (Language::Python, "else") if !unary_position(previous, Language::Python) => right(3),
            (Language::Python, "or") | (Language::Cpp | Language::C, "or") => left(4),
            (Language::Python, "and") | (Language::Cpp | Language::C, "and") => left(5),
            (Language::Python, "is" | "in") => none(10),
            (Language::TypeScript | Language::JavaScript, "in" | "instanceof")
            | (Language::Java, "instanceof")
            | (Language::Swift, "is" | "as") => left(10),
            (Language::TypeScript | Language::JavaScript, "as") => left(11),
            (Language::Python, "not") => right(6),
            (Language::Cpp | Language::C, "xor") => left(7),
            (Language::Cpp | Language::C, "bitand") => left(8),
            (Language::Cpp | Language::C, "bitor") => left(6),
            _ => return None,
        }),
        _ => None,
    }
}

fn unary_position(previous: Option<&TokenKind>, lang: Language) -> bool {
    match previous {
        None | Some(TokenKind::OpenDelim(_)) => true,
        Some(TokenKind::Punct(operator)) => matches!(
            operator.as_str(),
            "=" | ":="
                | "+="
                | "-="
                | "*="
                | "/="
                | "+"
                | "-"
                | "*"
                | "/"
                | "%"
                | "&&"
                | "||"
                | "!"
                | "~"
                | "&"
                | "|"
                | "^"
                | "?"
                | ":"
                | ","
                | ";"
        ),
        Some(TokenKind::Ident(word)) => match lang {
            Language::Python => matches!(
                word.as_str(),
                "return" | "yield" | "and" | "or" | "not" | "if" | "else"
            ),
            _ => matches!(word.as_str(), "return" | "yield" | "throw" | "case"),
        },
        _ => false,
    }
}

fn token_can_end_expression(token: &TokenKind) -> bool {
    matches!(
        token,
        TokenKind::Ident(_)
            | TokenKind::NumberLit(_)
            | TokenKind::StringLit(_)
            | TokenKind::CloseDelim(_)
    )
}

fn ends_with_keyword(text: &str, keyword: &str) -> bool {
    text.strip_suffix(keyword).is_some_and(|prefix| {
        prefix
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
    })
}
