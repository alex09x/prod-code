/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::util::{closes, indent_of, raw_excerpt};
use crate::dossier::types::AssertionEvidence;

/// Node's `assert` names its operands: the first argument is `actual`, the second `expected`.
fn node_evidence(
    format: &str,
    expression: Option<String>,
    actual: String,
    expected: String,
    excerpt: String,
) -> AssertionEvidence {
    AssertionEvidence {
        format: format.to_string(),
        expression,
        actual: Some(actual.clone()),
        expected: Some(expected.clone()),
        left: Some(actual.clone()),
        right: Some(expected.clone()),
        operands: vec![actual, expected],
        excerpt,
    }
}

/// Whether Node's `util.inspect` shortened a value: objects past its depth, and the tails of
/// long arrays and strings.
fn inspect_elided(value: &str) -> bool {
    value.contains("[Object]")
        || value.contains("[Array]")
        || (value.contains("... ")
            && (value.contains(" more item") || value.contains(" more character")))
}

/// The error's own properties as Node prints an uncaught or test-runner `AssertionError`: the
/// last stack frame opens `{`, top-level fields sit two columns in from the error header, and
/// `}` at the header's column closes the block. Only those top-level fields are bound, so an
/// `expected:` or `operator:` key nested in a value, or printed in the diff above, is never
/// taken for the error's own. Without the closing brace the block is incomplete, and the short
/// message above it is not taken instead.
pub(crate) fn parse_node_error_fields(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let header = stripped.iter().position(|l| {
        l.trim_start()
            .starts_with("AssertionError [ERR_ASSERTION]: ")
    })?;
    let base = indent_of(&stripped[header]);
    let open = (header + 1..stripped.len()).find(|&i| {
        let line = stripped[i].trim_end();
        indent_of(line) == base + 4 && line.trim_start().starts_with("at ") && line.ends_with(" {")
    })?;
    Some(node_error_fields(raw, stripped, header, base, open))
}

fn node_error_fields(
    raw: &[&str],
    stripped: &[String],
    header: usize,
    base: usize,
    open: usize,
) -> Option<AssertionEvidence> {
    let mut fields: Vec<(String, Vec<String>)> = Vec::new();
    let mut close = None;
    for (i, line) in stripped.iter().enumerate().skip(open + 1) {
        let line = line.trim_end();
        let indent = indent_of(line);
        let text = line.trim_start_matches(' ');
        if text.is_empty() {
            return None;
        }
        if indent == base && text == "}" {
            close = Some(i);
            break;
        }
        if indent < base + 2 {
            return None;
        }
        if indent == base + 2 && !text.starts_with(['}', ']', ')']) {
            let (key, value) = text.split_once(": ")?;
            fields.push((key.to_string(), vec![value.to_string()]));
        } else {
            fields.last_mut()?.1.push(line.to_string());
        }
    }
    let close = close?;
    let value_of = |name: &str| -> Option<String> {
        let mut found = fields
            .iter()
            .enumerate()
            .filter(|(_, (key, _))| key == name);
        let (at, (_, lines)) = found.next()?;
        if found.next().is_some() {
            return None;
        }
        let joined = lines.join("\n");
        // Every field but the last ends with a comma.
        let value = if at + 1 < fields.len() {
            joined.strip_suffix(',')?
        } else if joined.ends_with(',') {
            return None;
        } else {
            joined.as_str()
        };
        (!value.trim().is_empty() && !inspect_elided(value)).then(|| value.to_string())
    };
    let format = match value_of("operator")?.as_str() {
        "'strictEqual'" => "strictEqual",
        "'deepStrictEqual'" => "deepStrictEqual",
        _ => return None,
    };
    let actual = value_of("actual")?;
    let expected = value_of("expected")?;
    let expression = node_short_pair(stripped, header).map(|(_, line, _, _)| line);
    Some(node_evidence(
        format,
        expression,
        actual,
        expected,
        raw_excerpt(raw, header, close),
    ))
}

/// Node's message for two short unequal primitives: "Expected values to be strictly equal:",
/// then `actual !== expected` on the next non-blank line. Exactly one ` !== ` makes the split
/// unambiguous; a quoted value that holds that text gives two and is refused. Output that ends
/// at the pair may have cut it: a line after it shows it was printed whole.
fn node_short_pair(stripped: &[String], header: usize) -> Option<(usize, String, String, String)> {
    if !stripped[header]
        .trim_end()
        .ends_with("Expected values to be strictly equal:")
    {
        return None;
    }
    let at = (header + 1..stripped.len()).find(|&i| !stripped[i].trim().is_empty())?;
    if at + 1 == stripped.len() {
        return None;
    }
    let line = stripped[at].trim();
    if line.matches(" !== ").count() != 1 || inspect_elided(line) {
        return None;
    }
    let (actual, expected) = line.split_once(" !== ")?;
    if actual.is_empty() || expected.is_empty() {
        return None;
    }
    Some((
        at,
        line.to_string(),
        actual.to_string(),
        expected.to_string(),
    ))
}

/// The short message alone, as vitest prints a Node assertion error.
pub(crate) fn parse_node_short_message(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let header = stripped.iter().position(|l| {
        let l = l.trim();
        l.starts_with("AssertionError") && l.ends_with("Expected values to be strictly equal:")
    })?;
    Some(
        node_short_pair(stripped, header).map(|(at, line, actual, expected)| {
            node_evidence(
                "strictEqual",
                Some(line),
                actual,
                expected,
                raw_excerpt(raw, header, at),
            )
        }),
    )
}

/// jest's reprint of a Node assertion error: "assert.strictEqual(received, expected)", then
/// "Expected value to strictly be equal to:" and "Received:" at the hint's column, each value
/// two columns further in (further lines of a multi-line string at the hint's column), and a
/// blank line after the received value. Output cut before that blank line is refused.
pub(crate) fn parse_jest_node_assert(
    raw: &[&str],
    stripped: &[String],
) -> Option<Option<AssertionEvidence>> {
    let (hint, format, label) =
        stripped
            .iter()
            .enumerate()
            .find_map(|(i, line)| match line.trim() {
                "assert.strictEqual(received, expected)" => {
                    Some((i, "strictEqual", "Expected value to strictly be equal to:"))
                }
                "assert.deepStrictEqual(received, expected)" => Some((
                    i,
                    "deepStrictEqual",
                    "Expected value to deeply and strictly equal to:",
                )),
                _ => None,
            })?;
    let base = indent_of(&stripped[hint]);
    let at_base =
        |i: usize, text: &str| indent_of(&stripped[i]) == base && stripped[i].trim() == text;
    let values = || {
        let expected_label =
            (hint + 1..stripped.len()).find(|&i| !stripped[i].trim().is_empty())?;
        if !at_base(expected_label, label) {
            return None;
        }
        let received_label =
            (expected_label + 1..stripped.len()).find(|&i| at_base(i, "Received:"))?;
        let end = (received_label + 1..stripped.len()).find(|&i| stripped[i].trim().is_empty())?;
        let expected = jest_value(&stripped[expected_label + 1..received_label], base)?;
        let actual = jest_value(&stripped[received_label + 1..end], base)?;
        Some(node_evidence(
            format,
            None,
            actual,
            expected,
            raw_excerpt(raw, hint, end - 1),
        ))
    };
    Some(values())
}

/// One value as jest prints it: the first line two columns in from `base`, any further lines
/// (a multi-line string) at `base`. Refused unless its strings and brackets all close and
/// nothing was elided (`…` past jest's `maxWidth`, `[Object]` / `[Array]` past `maxDepth`).
fn jest_value(lines: &[String], base: usize) -> Option<String> {
    let (first, rest) = lines.split_first()?;
    let first = first.trim_end().strip_prefix(&" ".repeat(base + 2))?;
    if first.is_empty() || first.starts_with(' ') {
        return None;
    }
    let mut value = first.to_string();
    for line in rest {
        let line = line.trim_end();
        value.push('\n');
        if !line.is_empty() {
            value.push_str(line.strip_prefix(&" ".repeat(base))?);
        }
    }
    (!value.contains('…')
        && !value.contains("[Object]")
        && !value.contains("[Array]")
        && closes(&value, false))
    .then_some(value)
}
