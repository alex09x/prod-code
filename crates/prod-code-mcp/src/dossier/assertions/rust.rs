/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::util::{closes, raw_excerpt};
use crate::dossier::types::AssertionEvidence;

/// `assert_eq!` / `assert_ne!` since Rust 1.73:
/// "assertion `left == right` failed[: message]\n  left: {:?}\n right: {:?}". The operands keep
/// the macro's names: either one may be the expected value, so no actual/expected is claimed.
pub(crate) fn parse_rust_assertion(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let (header, is_ne) = stripped.iter().enumerate().find_map(|(i, line)| {
        let (is_ne, rest) =
            if let Some(rest) = line.strip_prefix("assertion `left == right` failed") {
                (false, rest)
            } else {
                (true, line.strip_prefix("assertion `left != right` failed")?)
            };
        (rest.trim_end().is_empty() || rest.starts_with(": ")).then_some((i, is_ne))
    })?;
    Some(rust_operands(raw, stripped, header, is_ne))
}

/// The operands after a Rust assertion header. A `Debug` value may hold blank lines, so only
/// the panic hook's own trailer (the backtrace note, a backtrace, the next panic, which the
/// hook starts with a blank line) or the end of the test's libtest block ends the message.
/// libtest closes a block with one blank line, two before its `failures:` list: a block
/// without one was cut short, and a third belongs to the value, whose own trailing line breaks
/// are then unknown.
fn rust_operands(
    raw: &[&str],
    stripped: &[String],
    header: usize,
    is_ne: bool,
) -> Option<AssertionEvidence> {
    let trailer = |line: &str| {
        line.starts_with("note: run with ")
            || line.starts_with("stack backtrace:")
            || line.starts_with("thread '")
    };
    let next_block = |line: &str| {
        line == "failures:" || (line.starts_with("---- ") && line.ends_with(" stdout ----"))
    };
    let stop =
        (header + 1..stripped.len()).find(|&i| trailer(&stripped[i]) || next_block(&stripped[i]));
    let end = match stop {
        Some(at) if stripped[at].starts_with("thread '") && stripped[at - 1].is_empty() => at - 1,
        Some(at) if trailer(&stripped[at]) => at,
        stop => {
            let stop = stop.unwrap_or(stripped.len());
            let content = (header + 1..stop).rfind(|&i| !stripped[i].trim().is_empty())? + 1;
            if !(1..=2).contains(&(stop - content)) {
                return None;
            }
            content
        }
    };
    // A custom message may hold "  left: " lines of its own: the operands start at the last
    // one. Two " right: " lines after it leave the split between the operands unknown.
    let (left_at, right_at) = (header + 1..end)
        .rev()
        .filter(|&j| stripped[j].starts_with("  left: "))
        .find_map(|j| {
            let mut rights = (j + 1..end).filter(|&k| stripped[k].starts_with(" right: "));
            let first = rights.next()?;
            Some(rights.next().is_none().then_some((j, first)))
        })??;
    let operand = |first: &str, rest: &[String]| {
        std::iter::once(first)
            .chain(rest.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let left = operand(
        &stripped[left_at]["  left: ".len()..],
        &stripped[left_at + 1..right_at],
    );
    let right = operand(
        &stripped[right_at][" right: ".len()..],
        &stripped[right_at + 1..end],
    );
    // A block cut after one of a value's blank lines still leaves its brackets open.
    if [&left, &right]
        .iter()
        .any(|v| v.trim().is_empty() || !closes(v, true))
    {
        return None;
    }
    Some(AssertionEvidence {
        format: if is_ne { "assert_ne" } else { "assert_eq" }.to_string(),
        expression: Some(
            if is_ne {
                "left != right"
            } else {
                "left == right"
            }
            .to_string(),
        ),
        actual: None,
        expected: None,
        left: Some(left.clone()),
        right: Some(right.clone()),
        operands: vec![left, right],
        excerpt: raw_excerpt(raw, header, end - 1),
    })
}
