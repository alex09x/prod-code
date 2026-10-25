/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::yield_context::{expression_arrow, function_parameters, tokens};

/// The lexer may mask a regex after `await`, but Script grammar also permits a
/// value named `await`. Retain possible division callers unless an async body
/// or an explicit Module goal proves the operator role. Unknown is not proof.
pub(super) fn ambiguous_regex_prefix(text: &[u8], code: &[bool]) -> bool {
    let tokens = tokens(text, code);
    let Some(&(start, word)) = tokens.last() else {
        return false;
    };
    word == b"await"
        && !tokens
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|(_, token)| matches!(*token, b"." | b"#"))
        && is_operator(&text[..start], &tokens[..tokens.len() - 1]) != Some(true)
}

fn is_operator(text: &[u8], tokens: &[(usize, &[u8])]) -> Option<bool> {
    let arrow = expression_arrow(text, tokens);
    let arrow_grammar = |arrow: usize| {
        text[tokens[arrow].0 + 1..]
            .iter()
            .all(|b| b.is_ascii_whitespace() || *b == b'(')
            .then(|| async_arrow(text, &tokens[..arrow - 1]))
    };
    let mut scopes = Vec::new();
    for (i, (_, token)) in tokens.iter().enumerate() {
        match *token {
            b"{" | b"(" | b"[" => scopes.push(i),
            b"}" | b")" | b"]" => {
                let open = scopes.pop()?;
                if !matches!(
                    (tokens[open].1, *token),
                    (b"{", b"}") | (b"(", b")") | (b"[", b"]")
                ) {
                    return None;
                }
            }
            _ => {}
        }
    }
    for boundary in scopes.into_iter().rev() {
        if let Some(arrow) = arrow.filter(|&arrow| arrow > boundary) {
            return arrow_grammar(arrow);
        }
        if tokens[boundary].1 == b"(" && function_parameters(&tokens[..boundary]) {
            return None; // Parameter initializers do not inherit the outer body.
        }
        if tokens[boundary].1 != b"{" {
            continue;
        }
        let before = &tokens[..boundary];
        let &(_, last) = before.last()?;
        if last == b">" && before.iter().rev().nth(1).is_some_and(|(_, t)| *t == b"=") {
            return Some(async_arrow(text, &before[..before.len() - 2]));
        }
        if last != b")" {
            if matches!(
                last,
                b"{" | b"}" | b";" | b"else" | b"try" | b"finally" | b"do"
            ) {
                continue;
            }
            return None;
        }
        let open = matching_open(before)?;
        let header = &before[..open];
        let mut function = header.len().checked_sub(1)?;
        // Named/anonymous ordinary and generator functions have the same await
        // grammar: only an async modifier enables the operator in their body.
        if header[function].1 != b"function" {
            function = function.checked_sub(1)?;
            if header[function].1 == b"*" {
                function = function.checked_sub(1)?;
            }
        }
        if header[function].1 == b"function"
            && !function
                .checked_sub(1)
                .is_some_and(|i| matches!(header[i].1, b"." | b"#"))
        {
            return Some(async_modifier(text, header, function));
        }
        let last = header.last()?.1;
        let previous = header.iter().rev().nth(1).map(|(_, t)| *t);
        if matches!(
            last,
            b"if" | b"for" | b"while" | b"switch" | b"catch" | b"with"
        ) && !previous.is_some_and(|t| matches!(t, b"." | b"#"))
        {
            continue;
        }
        return None; // Unproven method, computed header, or call plus ASI block.
    }
    if let Some(arrow) = arrow {
        return arrow_grammar(arrow);
    }
    // An explicit top-level export proves Module grammar. A package/extension
    // goal is not available in this lexical API, so other top-level cases stay
    // ambiguous instead of assuming that every file is a Module or a Script.
    let mut depth = 0usize;
    for (i, (_, token)) in tokens.iter().enumerate() {
        match *token {
            b"export"
                if depth == 0
                    && !i
                        .checked_sub(1)
                        .is_some_and(|p| matches!(tokens[p].1, b"." | b"#"))
                    && tokens.get(i + 1).is_some_and(|(_, next)| {
                        matches!(
                            *next,
                            b"function"
                                | b"async"
                                | b"const"
                                | b"let"
                                | b"var"
                                | b"class"
                                | b"default"
                                | b"{"
                                | b"*"
                        )
                    }) =>
            {
                return Some(true);
            }
            b"{" | b"(" | b"[" => depth += 1,
            b"}" | b")" | b"]" => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn matching_open(tokens: &[(usize, &[u8])]) -> Option<usize> {
    let mut depth = 0usize;
    tokens.iter().rposition(|(_, token)| match *token {
        b")" => {
            depth += 1;
            false
        }
        b"(" => {
            depth = depth.saturating_sub(1);
            depth == 0
        }
        _ => false,
    })
}

fn async_arrow(text: &[u8], header: &[(usize, &[u8])]) -> bool {
    let Some((_, last)) = header.last() else {
        return false;
    };
    let parameter = if *last == b")" {
        matching_open(header)
    } else {
        header.len().checked_sub(1)
    };
    parameter.is_some_and(|at| async_modifier(text, header, at))
}

fn async_modifier(text: &[u8], header: &[(usize, &[u8])], at: usize) -> bool {
    at.checked_sub(1).is_some_and(|modifier| {
        header[modifier].1 == b"async"
            && std::str::from_utf8(&text[header[modifier].0 + 5..header[at].0])
                .is_ok_and(|gap| !gap.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
            && !modifier
                .checked_sub(1)
                .is_some_and(|i| matches!(header[i].1, b"." | b"#"))
    })
}
