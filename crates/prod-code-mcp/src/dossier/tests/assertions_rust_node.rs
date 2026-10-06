/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::dossier::assertions::parse_assertion_evidence;

#[test]
fn parses_rust_assert_eq_simple() {
    let output = "thread 'tests::it_fails' panicked at src/lib.rs:13:9:\nassertion `left == right` failed\n  left: 4\n right: 5\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";
    let ev = parse_assertion_evidence(output).expect("parsed assertion");
    assert_eq!(ev.format, "assert_eq");
    assert_eq!(ev.expression.as_deref(), Some("left == right"));
    assert_eq!(ev.left.as_deref(), Some("4"));
    assert_eq!(ev.right.as_deref(), Some("5"));
    // `assert_eq!` accepts either order: no operand is claimed to be the expected one.
    assert_eq!(ev.actual, None);
    assert_eq!(ev.expected, None);
    assert_eq!(ev.operands, vec!["4", "5"]);
    assert_eq!(
        ev.excerpt,
        "assertion `left == right` failed\n  left: 4\n right: 5"
    );
    assert_eq!(
        ev.render_compact(),
        "assertion [assert_eq (left == right)]: left: 4, right: 5\n"
    );
}

#[test]
fn parses_rust_assert_eq_multiline_debug() {
    let output = "thread 'tests::it_fails' panicked at src/lib.rs:13:9:\nassertion `left == right` failed\n  left: Foo {\n    a: 1,\n    b: 2,\n}\n right: Foo {\n    a: 1,\n    b: 3,\n}\nnote: run with `RUST_BACKTRACE=1`\n";
    let ev = parse_assertion_evidence(output).expect("parsed assertion");
    assert_eq!(ev.format, "assert_eq");
    assert_eq!(ev.left.as_deref(), Some("Foo {\n    a: 1,\n    b: 2,\n}"));
    assert_eq!(ev.right.as_deref(), Some("Foo {\n    a: 1,\n    b: 3,\n}"));
    assert_eq!(ev.operands.len(), 2);
}

#[test]
fn parses_rust_assert_eq_colored() {
    let output = "\x1b[1m\x1b[31mthread 'tests::it_fails' panicked at \x1b[0msrc/lib.rs:13:9:\n\x1b[1m\x1b[31massertion `left == right` failed\x1b[0m\n\x1b[1m\x1b[31m  left: \x1b[0m\x1b[32m4\x1b[0m\n\x1b[1m\x1b[31m right: \x1b[0m\x1b[32m5\x1b[0m\nnote: run with RUST_BACKTRACE=1\n";
    let ev = parse_assertion_evidence(output).expect("parsed assertion");
    assert_eq!(ev.format, "assert_eq");
    assert_eq!(ev.left.as_deref(), Some("4"));
    assert_eq!(ev.right.as_deref(), Some("5"));
    assert_eq!(ev.actual, None);
    assert_eq!(ev.expected, None);
    // The excerpt keeps the escape sequences exactly as the runner printed them.
    assert!(
        ev.excerpt
            .starts_with("\x1b[1m\x1b[31massertion `left == right` failed\x1b[0m\n")
    );
    assert!(
        ev.excerpt
            .ends_with("\x1b[1m\x1b[31m right: \x1b[0m\x1b[32m5\x1b[0m")
    );
}

#[test]
fn parses_rust_assert_ne() {
    let output = "thread 'main' panicked at src/lib.rs:14:9:\nassertion `left != right` failed\n  left: 4\n right: 4\n\n";
    let ev = parse_assertion_evidence(output).expect("parsed assertion");
    assert_eq!(ev.format, "assert_ne");
    assert_eq!(ev.expression.as_deref(), Some("left != right"));
    assert_eq!(ev.left.as_deref(), Some("4"));
    assert_eq!(ev.right.as_deref(), Some("4"));
    assert_eq!(ev.actual, None);
    assert_eq!(ev.expected, None);
    assert_eq!(ev.operands, vec!["4", "4"]);
}

#[test]
fn parses_rust_assert_eq_with_custom_message() {
    let output = "thread 'test_msg' panicked at src/lib.rs:20:9:\nassertion `left == right` failed: expected matching user IDs\n  left: \"usr_1\"\n right: \"usr_2\"\n\n";
    let ev = parse_assertion_evidence(output).expect("parsed assertion");
    assert_eq!(ev.format, "assert_eq");
    assert_eq!(ev.left.as_deref(), Some("\"usr_1\""));
    assert_eq!(ev.right.as_deref(), Some("\"usr_2\""));
}

#[test]
fn a_left_line_inside_a_custom_message_is_not_an_operand() {
    let output = "thread 't' panicked at src/lib.rs:3:5:\nassertion `left == right` failed: first line\n  left: from the message\n  left: 7\n right: 8\n\n";
    let ev = parse_assertion_evidence(output).expect("parsed assertion");
    assert_eq!(ev.left.as_deref(), Some("7"));
    assert_eq!(ev.right.as_deref(), Some("8"));
}

#[test]
fn a_rust_block_without_both_operands_gives_nothing() {
    let output = "thread 't' panicked at src/lib.rs:3:5:\nassertion `left == right` failed\n  left: 7\nnote: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n";
    assert_eq!(parse_assertion_evidence(output), None);
}

#[test]
fn rust_operands_keep_their_blank_lines_up_to_the_panic_hooks_trailer() {
    let head = "\nthread 't' panicked at src/lib.rs:3:5:\nassertion `left == right` failed\n  left: A {\n\n    x\n}\n right: B {\n\n    y\n}\n";
    for trailer in [
        "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n\n",
        "stack backtrace:\n   0: rust_begin_unwind\n\n",
        // The hook starts every panic message with a blank line of its own.
        "\nthread 'other' panicked at src/lib.rs:9:1:\nboom\n\n",
        // The end of a libtest block: its separator, then the next block or the list.
        "\n",
        "\n\n",
    ] {
        let ev = parse_assertion_evidence(&format!("{head}{trailer}"))
            .unwrap_or_else(|| panic!("no evidence before {trailer:?}"));
        assert_eq!(ev.left.as_deref(), Some("A {\n\n    x\n}"), "{trailer:?}");
        assert_eq!(ev.right.as_deref(), Some("B {\n\n    y\n}"), "{trailer:?}");
        assert!(
            ev.excerpt.ends_with(" right: B {\n\n    y\n}"),
            "{trailer:?}"
        );
    }
    // Before the hook's trailer a value's own trailing line break is printed as is.
    let own = head.replace("    y\n}\n", "    y\n}\n\n")
        + "note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace\n";
    let ev = parse_assertion_evidence(&own).expect("evidence");
    assert_eq!(ev.right.as_deref(), Some("B {\n\n    y\n}\n"));
}

#[test]
fn a_rust_block_cut_short_or_split_ambiguously_gives_nothing() {
    let block = "\nthread 't' panicked at src/lib.rs:3:5:\nassertion `left == right` failed\n  left: A {\n\n    x\n}\n right: B {\n\n    y\n}\n\n";
    // Cut at every line: without libtest's separator, or with a value's brackets still
    // open after one of its blank lines, nothing is claimed.
    let lines: Vec<&str> = block.split_inclusive('\n').collect();
    for cut in 1..lines.len() {
        let prefix = lines[..cut].concat();
        assert_eq!(parse_assertion_evidence(&prefix), None, "{prefix:?}");
    }
    // More blank lines than libtest's separators belong to the value, which then has an
    // unknown number of trailing line breaks.
    assert_eq!(parse_assertion_evidence(&format!("{block}\n\n")), None);
    // A second `right:` after the last `left:` could start either operand.
    let two_rights = block.replace("    x\n", "    x\n right: inside\n");
    assert_eq!(parse_assertion_evidence(&two_rights), None);
    // The Rust header claims the block: a Node message in it is not taken instead.
    let with_node = block.replace(
        "}\n\n",
        "\nAssertionError: Expected values to be strictly equal:\n\n1 !== 2\n\n",
    );
    assert_eq!(parse_assertion_evidence(&with_node), None);
}

#[test]
fn rust_char_operands_do_not_count_as_brackets() {
    let output =
        "assertion `left == right` failed\n  left: '{'\n right: ['\"', '\\'', '\\u{7b}', ')']\n\n";
    let ev = parse_assertion_evidence(output).expect("evidence");
    assert_eq!(ev.left.as_deref(), Some("'{'"));
    assert_eq!(ev.right.as_deref(), Some("['\"', '\\'', '\\u{7b}', ')']"));
}

#[test]
fn parses_node_strict_equal() {
    let output = "node:assert:95\n  throw new AssertionError(obj);\n  ^\n\nAssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\n\n1 !== 2\n\n    at [eval]:1:42 {\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: 1,\n  expected: 2,\n  operator: 'strictEqual'\n}\n\nNode.js v24.3.0\n";
    let ev = parse_assertion_evidence(output).expect("parsed node assertion");
    assert_eq!(ev.format, "strictEqual");
    assert_eq!(ev.expression.as_deref(), Some("1 !== 2"));
    assert_eq!(ev.actual.as_deref(), Some("1"));
    assert_eq!(ev.expected.as_deref(), Some("2"));
    assert_eq!(ev.left.as_deref(), Some("1"));
    assert_eq!(ev.right.as_deref(), Some("2"));
    assert_eq!(ev.operands, vec!["1", "2"]);
    assert!(ev.excerpt.contains("AssertionError"));
    assert!(ev.excerpt.contains("operator: 'strictEqual'"));
}

#[test]
fn parses_node_deep_strict_equal_multiline() {
    let output = "AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:\n+ actual - expected\n\n  {\n+   a: 1\n-   a: 2\n  }\n\n    at [eval]:1:42 {\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: {\n    a: 'foo',\n    b: [ 1, 2 ]\n  },\n  expected: {\n    a: 'bar',\n    b: [ 1, 3 ]\n  },\n  operator: 'deepStrictEqual'\n}\n";
    let ev = parse_assertion_evidence(output).expect("parsed node deep assertion");
    assert_eq!(ev.format, "deepStrictEqual");
    assert_eq!(ev.expression, None);
    assert_eq!(
        ev.actual.as_deref(),
        Some("{\n    a: 'foo',\n    b: [ 1, 2 ]\n  }")
    );
    assert_eq!(
        ev.expected.as_deref(),
        Some("{\n    a: 'bar',\n    b: [ 1, 3 ]\n  }")
    );
    assert_eq!(ev.operands.len(), 2);
}

#[test]
fn parses_node_with_ansi_colors() {
    let output = "\x1b[31mAssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\x1b[0m\n\n1 !== 2\n\n    at test.js:1:1 {\n  actual: \x1b[32m1\x1b[0m,\n  expected: \x1b[31m2\x1b[0m,\n  operator: 'strictEqual'\n}\n";
    let ev = parse_assertion_evidence(output).expect("parsed colored node assertion");
    assert_eq!(ev.format, "strictEqual");
    assert_eq!(ev.actual.as_deref(), Some("1"));
    assert_eq!(ev.expected.as_deref(), Some("2"));
}

const NESTED_FIELDS: &str = "AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:\n+ actual - expected\n\n  {\n+   expected: 'inner-a',\n-   expected: 'inner-b',\n    operator: 'strictEqual'\n  }\n\n    at run (file.js:1:1) {\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: {\n    expected: 'inner-a',\n    operator: 'strictEqual'\n  },\n  expected: {\n    expected: 'inner-b',\n    operator: 'strictEqual'\n  },\n  operator: 'deepStrictEqual'\n}\n";

#[test]
fn node_fields_nested_in_a_value_or_the_diff_are_not_the_errors_own() {
    let ev = parse_assertion_evidence(NESTED_FIELDS).expect("parsed node fields");
    assert_eq!(ev.format, "deepStrictEqual");
    assert_eq!(ev.expression, None);
    assert_eq!(
        ev.actual.as_deref(),
        Some("{\n    expected: 'inner-a',\n    operator: 'strictEqual'\n  }")
    );
    assert_eq!(
        ev.expected.as_deref(),
        Some("{\n    expected: 'inner-b',\n    operator: 'strictEqual'\n  }")
    );
    assert!(ev.excerpt.ends_with("operator: 'deepStrictEqual'\n}"));
}

#[test]
fn incomplete_or_elided_node_fields_give_nothing() {
    // Cut before the closing brace.
    let cut = NESTED_FIELDS.trim_end().strip_suffix("\n}").unwrap();
    assert_eq!(parse_assertion_evidence(cut), None);
    // Cut inside the expected value.
    let cut = &NESTED_FIELDS[..NESTED_FIELDS.find("    expected: 'inner-b'").unwrap()];
    assert_eq!(parse_assertion_evidence(cut), None);
    // A top-level field missing.
    let no_operator = NESTED_FIELDS.replace("  },\n  operator: 'deepStrictEqual'\n", "  }\n");
    assert_eq!(parse_assertion_evidence(&no_operator), None);
    // util.inspect elided a nested object or the tail of an array.
    let depth = NESTED_FIELDS.replace("operator: 'strictEqual'\n  },", "deeper: [Object]\n  },");
    assert_eq!(parse_assertion_evidence(&depth), None);
    let items = NESTED_FIELDS.replace("expected: 'inner-a',", "list: [ 1, ... 99 more items ],");
    assert_eq!(parse_assertion_evidence(&items), None);
}

#[test]
fn a_node_short_message_splits_only_on_a_single_operator() {
    let output = "AssertionError: Expected values to be strictly equal:\n\n'a' !== 'b'\n\n";
    let ev = parse_assertion_evidence(output).expect("parsed short message");
    assert_eq!(ev.format, "strictEqual");
    assert_eq!(ev.actual.as_deref(), Some("'a'"));
    assert_eq!(ev.expected.as_deref(), Some("'b'"));
    assert_eq!(ev.expression.as_deref(), Some("'a' !== 'b'"));
    let ambiguous =
        "AssertionError: Expected values to be strictly equal:\n\n'x !== y' !== 'z'\n\n";
    assert_eq!(parse_assertion_evidence(ambiguous), None);
    // Output that stops at the pair may have cut its second value.
    let cut = "AssertionError: Expected values to be strictly equal:\n\n'a' !== 'b";
    assert_eq!(parse_assertion_evidence(cut), None);
}

#[test]
fn a_cut_error_fields_block_is_not_read_from_its_short_message() {
    let fields = "AssertionError [ERR_ASSERTION]: Expected values to be strictly equal:\n\n1 !== 2\n\n    at [eval]:1:42 {\n  generatedMessage: true,\n  code: 'ERR_ASSERTION',\n  actual: 1,\n  expected: 2,\n  operator: 'strictEqual'\n}\n";
    assert!(parse_assertion_evidence(fields).is_some());
    let cut = fields.strip_suffix("}\n").unwrap();
    assert_eq!(parse_assertion_evidence(cut), None);
    let cut = &fields[..fields.find("  expected: 2").unwrap()];
    assert_eq!(parse_assertion_evidence(cut), None);
}

const JEST_STRICT: &str = "    assert.strictEqual(received, expected)\n\n    Expected value to strictly be equal to:\n      \"line one\n    line 2\"\n    Received:\n      \"line one\n    line two\"\n\n    Difference:\n";

#[test]
fn jest_values_are_bound_by_their_labels_and_columns() {
    let ev = parse_assertion_evidence(JEST_STRICT).expect("parsed jest reprint");
    assert_eq!(ev.format, "strictEqual");
    assert_eq!(ev.expected.as_deref(), Some("\"line one\nline 2\""));
    assert_eq!(ev.actual.as_deref(), Some("\"line one\nline two\""));
    assert_eq!(ev.left, ev.actual);
    assert_eq!(ev.right, ev.expected);
    assert!(
        ev.excerpt
            .starts_with("    assert.strictEqual(received, expected)")
    );
    assert!(ev.excerpt.ends_with("    line two\""));
}

#[test]
fn truncated_or_elided_jest_values_give_nothing() {
    // Output cut inside the received value: no blank line ends it.
    let cut = &JEST_STRICT[..JEST_STRICT.find("\n\n    Difference").unwrap()];
    assert_eq!(parse_assertion_evidence(cut), None);
    // A string that is still open when the next label comes.
    let open = JEST_STRICT.replace("    line 2\"\n", "    Received:\n    line 2\"\n");
    assert_eq!(parse_assertion_evidence(&open), None);
    // jest's maxWidth and maxDepth elisions.
    let wide = "    assert.deepStrictEqual(received, expected)\n\n    Expected value to deeply and strictly equal to:\n      [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, …]\n    Received:\n      [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, …]\n\n";
    assert_eq!(parse_assertion_evidence(wide), None);
    let deep = wide.replace("[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, …]", "{\"a\": [Object]}");
    assert_eq!(parse_assertion_evidence(&deep), None);
    // A label that does not match the hint's operator.
    let mismatched = JEST_STRICT.replace("strictly be equal to:", "deeply and strictly equal to:");
    assert_eq!(parse_assertion_evidence(&mismatched), None);
}
