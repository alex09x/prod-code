/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn python_scope_uses_definition_indent_and_ignores_blank_comment_lines() {
    let text = "\ndef compute():\n    value = 1\n\n# comment\n    return value\noutside()\n";
    let start = text.find("value").unwrap();
    assert_eq!(
        find_enclosing_scope(text, Language::Python, start, start + 5),
        (
            1,
            text.find("outside").unwrap(),
            Some("compute".into()),
            None,
            false,
            "".into()
        )
    );
    let text = "class Sample:\n    def compute(self):\n        value = 1\n        return value\n    def next(self):\n        pass\n";
    let start = text.find("value").unwrap();
    assert_eq!(
        find_enclosing_scope(text, Language::Python, start, start + 5),
        (
            text.find("    def").unwrap(),
            text.find("    def next").unwrap(),
            Some("compute".into()),
            None,
            true,
            "    ".into()
        )
    );
    for text in ["value", "def broken\n    value", "def same():\nvalue"] {
        let start = text.find("value").unwrap();
        let name = if text.starts_with("def same") {
            Some("same".into())
        } else {
            None
        };
        assert_eq!(
            find_enclosing_scope(text, Language::Python, start, start + 5),
            (0, text.len(), name, None, false, "".into())
        );
    }
}

#[test]
fn python_scope_end_is_exact_at_file_start_and_with_crlf() {
    for newline in ["\n", "\r\n"] {
        for prefix in ["", newline] {
            let text = format!(
                "{prefix}def compute():{newline}    value = 1{newline}    return value{newline}outside(){newline}"
            );
            let start = text.find("value").unwrap();
            assert_eq!(
                find_enclosing_scope(&text, Language::Python, start, start + 5),
                (
                    prefix.len(),
                    text.find("outside").unwrap(),
                    Some("compute".into()),
                    None,
                    false,
                    "".into()
                ),
                "{text:?}"
            );
        }
    }
}
