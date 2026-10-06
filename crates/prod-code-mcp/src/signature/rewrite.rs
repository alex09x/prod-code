/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::call_sites::{blank_comments, call_open, opaque_at, split_arguments};
use crate::signature::parse::offset_of;
use crate::signature::plan::{
    AWAIT, argument_list_end, call_start, common_prefix, in_use_or_comment, names_at,
};
use crate::signature::types::OldCall;

/// The call a rewrite made where the call at `from` was: the callee may be respelled (`m::f` as
/// `f`, `T::f(x, …)` as `x.f(…)`), so its name is the first one at the call's own depth before
/// the expression around it ends. `(where its argument list opens, just past it or its
/// `.await`)`.
pub fn rewritten_call(text: &str, from: usize, name: &str) -> Option<(usize, usize)> {
    let s = text.as_bytes();
    let mut depth = 0i32;
    let mut i = from;
    while i < s.len() {
        if let Some((end, _)) = opaque_at(text, i, from).ok()? {
            i = end;
            continue;
        }
        match s[i] {
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            b';' | b'{' | b'}' => return None,
            b',' if depth == 0 => return None,
            _ if depth == 0 && names_at(text, i, name) => {
                if let Ok(Some(open)) = call_open(text, i) {
                    let close = argument_list_end(text, open)?;
                    let end = close
                        + if text[close..].starts_with(AWAIT) {
                            AWAIT.len()
                        } else {
                            0
                        };
                    return Some((open, end));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Comments and significant source bytes of a call. Literal contents remain byte-for-byte:
/// whitespace inside a string is code, while whitespace between tokens is trivia.
pub fn call_tokens(text: &str) -> Option<(Vec<&str>, Vec<u8>)> {
    let mut comments = Vec::new();
    let mut code = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if let Some((end, comment)) = opaque_at(text, i, 0).ok()? {
            if comment {
                comments.push(&text[i..end]);
            } else {
                code.extend_from_slice(&text.as_bytes()[i..end]);
            }
            i = end;
        } else {
            let byte = text.as_bytes()[i];
            if !byte.is_ascii_whitespace() {
                code.push(byte);
            }
            i += 1;
        }
    }
    Some((comments, code))
}

/// rust-analyzer's structural replacement can put an argument's block comment immediately
/// after the rewritten call. Account for exactly those missing comment tokens, with their
/// multiplicity, before comparing the surrounding source. Arbitrary trailing comments, changed
/// or dropped comments, and unrelated edits still fail reconciliation. Relocated line or doc
/// comments are refused because moving them can change which code they cover or annotate.
pub fn attributed_call_end(
    old: &str,
    new: &str,
    full: &str,
    mut end: usize,
) -> Option<(usize, bool)> {
    let (mut missing, old_code) = call_tokens(old)?;
    let (retained, new_code) = call_tokens(new)?;
    for comment in retained {
        let i = missing.iter().position(|old| *old == comment)?;
        missing.remove(i);
    }
    while !missing.is_empty() {
        let start = end
            + full[end..]
                .bytes()
                .take_while(u8::is_ascii_whitespace)
                .count();
        if start == full.len() {
            return None;
        }
        let (after, true) = opaque_at(full, start, start).ok()?? else {
            return None;
        };
        let comment = &full[start..after];
        if !comment.starts_with("/*") || comment.starts_with("/**") || comment.starts_with("/*!") {
            return None;
        }
        let i = missing.iter().position(|old| *old == comment)?;
        missing.remove(i);
        end = after;
    }
    Some((end, old_code == new_code))
}

/// What the rewrite of one file (`old` into `new`) did with the references `refs` in it:
/// `(unmatched, unexpected)`.
///
/// Occurrence by occurrence, not line by line: a function used as a value on the line of a call
/// that was rewritten was taken for rewritten, and its callers would get the old order through
/// a pointer of the same type (#446). Every reference is a call the rewrite changed (or one that
/// needs no change: equal arguments reordered, an `.await` already there), an import, or a
/// comment; anything else is unmatched. Aside from unchanged block comments relocated from a
/// call to immediately after it, `new` has to be `old` byte for byte outside the calls;
/// the first place it is not is unexpected, and no call after it is taken as rewritten.
/// `needs` says, from a call's arguments and whether it is awaited, whether it must change.
pub fn attribute(
    place: &str,
    old: &str,
    new: &str,
    name: &str,
    refs: &[(u32, u32)],
    needs: &dyn Fn(&[String], bool) -> bool,
) -> (Vec<String>, Vec<String>) {
    let mut unmatched = Vec::new();
    let mut calls: Vec<OldCall> = Vec::new();
    let code = blank_comments(old);
    for &(l, c) in refs {
        let at = format!("{place}:{l}:{c}");
        let Some(o) = offset_of(old, l, c) else {
            unmatched.push(format!("{at} (no such position in the file)"));
            continue;
        };
        if !names_at(old, o, name) {
            unmatched.push(format!(
                "{at} (the analyzer places `{name}` here, but the file says otherwise)"
            ));
            continue;
        }
        if in_use_or_comment(old, code.as_deref(), o) {
            continue;
        }
        let open = match call_open(old, o) {
            Ok(Some(open)) => open,
            Ok(None) => {
                unmatched.push(format!(
                    "{at} (not a call: the function used as a value, which the rewrite of its \
                     calls does not reach)"
                ));
                continue;
            }
            Err(_) => {
                unmatched.push(format!("{at} (a call whose generics do not close)"));
                continue;
            }
        };
        let Some(close) = argument_list_end(old, open) else {
            unmatched.push(format!("{at} (a call whose argument list does not close)"));
            continue;
        };
        let awaited = old[close..].starts_with(AWAIT);
        let args = split_arguments(old, open).unwrap_or_default();
        calls.push(OldCall {
            place: at,
            start: call_start(old, o),
            open,
            end: close + if awaited { AWAIT.len() } else { 0 },
            needs: needs(&args, awaited),
        });
    }
    calls.sort_by_key(|c| c.start);
    // A call in another's arguments goes with it: the outer one's text holds both.
    let mut outer: Vec<(usize, Vec<usize>)> = Vec::new();
    for i in 0..calls.len() {
        match outer.last_mut() {
            Some((p, nested)) if calls[i].start < calls[*p].end => nested.push(i),
            _ => outer.push((i, Vec::new())),
        }
    }

    let (mut po, mut pn) = (0usize, 0usize);
    let mut stop = None;
    let mut verified = outer.len();
    for (k, (i, nested)) in outer.iter().enumerate() {
        let call = &calls[*i];
        let gap = &old[po..call.start];
        if !new[pn..].starts_with(gap) {
            stop = Some(po + common_prefix(gap, &new[pn..]));
            verified = k;
            break;
        }
        pn += gap.len();
        let Some((open, end)) = rewritten_call(new, pn, name) else {
            stop = Some(call.start);
            verified = k;
            break;
        };
        let Some((attributed_end, same_code)) =
            attributed_call_end(&old[call.open..call.end], &new[open..end], new, end)
        else {
            stop = Some(call.start);
            verified = k;
            break;
        };
        if call.needs && same_code {
            unmatched.push(format!(
                "{} (a call the rewrite left as it was)",
                call.place
            ));
        }
        let now = &new[pn..end];
        for j in nested {
            let inner = &calls[*j];
            if inner.needs && now.contains(&old[inner.start..inner.end]) {
                unmatched.push(format!(
                    "{} (a call inside another call, left as it was)",
                    inner.place
                ));
            }
        }
        (po, pn) = (call.end, attributed_end);
    }
    if stop.is_none() && old[po..] != new[pn..] {
        stop = Some(po + common_prefix(&old[po..], &new[pn..]));
    }
    let mut unexpected = Vec::new();
    if let Some(at) = stop {
        // Only the line is named: a change can start between the `\r` and `\n` of a break.
        unexpected.push(format!("{place}:{}", old[..at].matches('\n').count() + 1));
        for (i, nested) in &outer[verified..] {
            for j in std::iter::once(i).chain(nested) {
                if calls[*j].needs {
                    unmatched.push(format!(
                        "{} (after a change that is not a reference; not verified)",
                        calls[*j].place
                    ));
                }
            }
        }
    }
    (unmatched, unexpected)
}
