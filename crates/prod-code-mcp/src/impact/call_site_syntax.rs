/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(super) fn call_site_end_line(
    text: &str,
    line: u32,
    col: u32,
    name: &str,
    language: &str,
) -> Option<u32> {
    match call_expression_end_line(text, line, col, name, language) {
        Some(end_line) => Some(end_line),
        // Imports, re-exports and function values are references, not calls (#1474, #1733).
        None if names_at(text, line, col, name) => None,
        // If the server's range does not start at the name, retain the uncertain call site.
        None => Some(line),
    }
}

pub(super) fn call_hierarchy_call_end_line(
    text: &str,
    line: u32,
    col: u32,
    reported_end_line: Option<u32>,
    name: &str,
    language: &str,
) -> Option<u32> {
    let verified_end_line = call_site_end_line(text, line, col, name, language)?;
    Some(
        reported_end_line
            .unwrap_or(verified_end_line)
            .max(verified_end_line),
    )
}

fn names_at(text: &str, line: u32, col: u32, name: &str) -> bool {
    lsp_position_to_byte_offset(text, line, col)
        .and_then(|at| text.get(at..))
        .is_some_and(|rest| rest.starts_with(name))
}

fn lsp_position_to_byte_offset(text: &str, line: u32, character: u32) -> Option<usize> {
    if line == 0 || character == 0 {
        return None;
    }
    let mut offset = 0usize;
    for (index, raw_line) in text.split_inclusive('\n').enumerate() {
        if index as u32 + 1 != line {
            offset += raw_line.len();
            continue;
        }
        let line_text = raw_line
            .strip_suffix('\n')
            .unwrap_or(raw_line)
            .strip_suffix('\r')
            .unwrap_or_else(|| raw_line.strip_suffix('\n').unwrap_or(raw_line));
        let target = (character - 1) as usize;
        let mut units = 0usize;
        for (byte, ch) in line_text.char_indices() {
            if units == target {
                return Some(offset + byte);
            }
            let next = units + ch.len_utf16();
            if target < next {
                return None;
            }
            units = next;
        }
        return (units == target).then_some(offset + line_text.len());
    }
    None
}

pub(crate) fn call_expression_end_line(
    text: &str,
    line: u32,
    col: u32,
    name: &str,
    language: &str,
) -> Option<u32> {
    let start = lsp_position_to_byte_offset(text, line, col)?;
    let rest = text.get(start..)?;
    if !rest.starts_with(name) {
        return None;
    }
    let mut cursor = start + name.len();
    while text[cursor..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        cursor += text[cursor..].chars().next()?.len_utf8();
    }
    if language == "rust" && text[cursor..].starts_with("::<") {
        cursor += 2;
    }
    if text[cursor..].starts_with('<') {
        let mut depth = 0usize;
        let mut close = None;
        for (offset, ch) in text[cursor..].char_indices() {
            match ch {
                '<' => depth += 1,
                '>' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        close = Some(cursor + offset + ch.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        cursor = close?;
    }
    while text[cursor..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        cursor += text[cursor..].chars().next()?.len_utf8();
    }
    if text.as_bytes().get(cursor) != Some(&b'(') {
        return None;
    }
    let close = crate::parameter_object::matching_bracket(text, cursor)?;
    Some(text[..=close].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::call_hierarchy_call_end_line;

    #[test]
    fn import_reference_is_not_a_call_but_invocation_is() {
        let source = "use crate::module::resolve_symbol_args;\n\
fn caller() {\n\
    let _ = resolve_symbol_args(\n\
        remote,\n\
        root,\n\
    );\n\
}\n";
        let import_col = source
            .lines()
            .next()
            .unwrap()
            .find("resolve_symbol_args")
            .unwrap()
            + 1;
        assert_eq!(
            call_hierarchy_call_end_line(
                source,
                1,
                import_col as u32,
                Some(1),
                "resolve_symbol_args",
                "rust",
            ),
            None,
            "the imported name must not be reported as a caller"
        );

        let call_line = 3;
        let call_col = source
            .lines()
            .nth((call_line - 1) as usize)
            .unwrap()
            .find("resolve_symbol_args")
            .unwrap()
            + 1;
        assert_eq!(
            call_hierarchy_call_end_line(
                source,
                call_line,
                call_col as u32,
                Some(6),
                "resolve_symbol_args",
                "rust",
            ),
            Some(6),
            "the real multiline call must remain a call site"
        );
    }
}
