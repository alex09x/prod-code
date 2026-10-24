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
fn coverage_escapes_and_balancing() {
    for (text, expected) in [
        ("α\x1b[31mβ\x1b[0m", "αβ"),
        ("\x1b]0;title\x07body", "body"),
        ("\x1b]link\x1b\\body", "body"),
        ("x\x1b[31", "x"), ("x\x1b]unfinished", "x"),
        ("x\x1b", "x"), ("x\x1b?y", "x?y"),
    ] { assert_eq!(strip_ansi(text), expected); }
    for (text, rust, expected) in [
        ("", false, true), ("([{}])", false, true), ("([)]", false, false),
        ("\"[\\\"]\"", false, true), ("\"unfinished", false, false),
        ("'{'", true, true), ("'\\''", true, true), ("'", true, true),
        ("'['", false, false), ("'ab", true, true), ("}", false, false),
    ] { assert_eq!(closes(text, rust), expected, "{text:?}"); }
    assert_eq!(indent_of("  α"), 2);
    assert_eq!(indent_of("\tα"), 0);
    assert_eq!(raw_excerpt(&["first", "α", "last"], 1, 2), "α\nlast");
}
