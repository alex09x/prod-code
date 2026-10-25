/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

fn ident_continue(c: char) -> bool {
    unicode_ident::is_xid_continue(c) || matches!(c, '$' | '\u{200c}' | '\u{200d}')
}

/// Await the selected result before a postfix consumes it. An existing outer
/// await still resolves the postfix result; keep it and its trivia unchanged.
pub(super) fn await_call(
    code: &str,
    call_start: usize,
    callee_end: usize,
    args_end: usize,
    edits: &mut Vec<(usize, usize, String)>,
) -> Result<(), &'static str> {
    // An await may be separated from the call by transparent grouping. Only use
    // the wider span if it leads to an await; an argument list is not grouping.
    let (mut start, mut end) = (call_start, args_end + 1);
    while let Some(prefix) = code[..start].trim_end().strip_suffix('(') {
        let preceding = prefix.trim_end();
        // A value followed by '(' is a call's argument list, not grouping.
        // In particular, a property named `await` is still a value.
        if await_token(preceding).is_none()
            && preceding.chars().next_back().is_some_and(|c| {
                ident_continue(c) || matches!(c, ')' | ']' | '!' | '\'' | '"' | '`' | '~')
            })
        {
            break;
        }
        let after = code[end..].trim_start();
        let close = code.len() - after.len();
        if !after.starts_with(')')
            || crate::parameter_object::matching_bracket(code, prefix.len()) != Some(close)
        {
            break;
        }
        start = prefix.len();
        end = close + 1;
    }
    let await_start = await_token(&code[..start]);
    if await_start.is_none() {
        end = args_end + 1;
    }
    let after = code[end..].trim_start();
    let has_postfix = after.starts_with('.')
        || after.starts_with("?.")
        || after.starts_with('[')
        || after.starts_with('(')
        || (after.starts_with('!') && !after.starts_with("!="));
    // Grouping a continuous optional receiver chain would make its result
    // postfix run even when the receiver is null. Existing explicit grouping
    // already ends that chain and may safely keep its original behavior.
    if has_postfix && end == args_end + 1 && code[call_start..callee_end].contains("?.") {
        return Err(
            "optional receiver short-circuit cannot be preserved before the result postfix",
        );
    }
    match (await_start, has_postfix) {
        (Some(_), true) => {
            edits.push((start, 0, "(await ".to_string()));
            edits.push((end, 0, ")".to_string()));
        }
        (None, true) => {
            edits.push((call_start, 0, "(await ".to_string()));
            edits.push((args_end + 1, 0, ")".to_string()));
        }
        (None, false) => edits.push((call_start, 0, "await ".to_string())),
        (Some(_), false) => {}
    }
    Ok(())
}

fn await_token(before: &str) -> Option<usize> {
    before.trim_end().strip_suffix("await").and_then(|prefix| {
        (!prefix.chars().next_back().is_some_and(ident_continue)
            && !prefix.trim_end().ends_with(['.', '#']))
        .then_some(prefix.len())
    })
}
