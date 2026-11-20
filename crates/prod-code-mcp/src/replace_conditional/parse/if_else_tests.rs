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
fn coverage_b2_if_else_python_cascades() {
    let text = "before\n  if kind == 'a':\n    first()\n  elif kind == 'b':\n    second()\n  else:\n    fallback()\nafter()\n";
    let result = parse_if_else_block(text, text.find("if ").unwrap()).unwrap();
    assert_eq!(result.start_offset, 7);
    assert_eq!(result.end_offset, text.find("after()").unwrap());
    assert_eq!(result.indent, "  ");
    assert_eq!(result.discriminator, "kind");
    let branches: Vec<_> = result
        .branches
        .iter()
        .map(|b| (b.tag.as_str(), b.body.as_str(), b.is_default))
        .collect();
    assert_eq!(
        branches,
        vec![
            ("'a'", "    first()", false),
            ("'b'", "    second()", false),
            ("default", "    fallback()", true)
        ]
    );
    let text = "if ready:\n    go()\n\nelse:\n    stop()";
    let result = parse_if_else_block(text, text.len()).unwrap();
    assert_eq!(result.end_offset, text.len());
    assert_eq!(result.discriminator, "");
    assert_eq!(result.branches[0].tag, "ready");
    assert_eq!(result.branches[1].body, "    stop()");
}

#[test]
fn coverage_b2_if_else_curly_and_refusals() {
    for (text, discriminator, tags, bodies) in [
        (
            "if (kind === 'a') { one(); } else if(kind == 'b') { two(); } else { three(); } tail",
            "kind",
            vec!["'a'", "'b'", "default"],
            vec!["one();", "two();", "three();"],
        ),
        (
            "if ready { go(); } else { stop(); }",
            "",
            vec!["ready", "default"],
            vec!["go();", "stop();"],
        ),
        (
            "if x == 1 { a(); } else if x == 2 { b(); } tail",
            "x",
            vec!["1", "2"],
            vec!["a();", "b();"],
        ),
    ] {
        let result = parse_if_else_block(text, 0).unwrap();
        assert_eq!(result.discriminator, discriminator);
        assert_eq!(result.start_offset, 0);
        assert_eq!(&text[result.end_offset - 1..result.end_offset], "}");
        assert_eq!(
            result
                .branches
                .iter()
                .map(|b| b.tag.as_str())
                .collect::<Vec<_>>(),
            tags
        );
        assert_eq!(
            result
                .branches
                .iter()
                .map(|b| b.body.as_str())
                .collect::<Vec<_>>(),
            bodies
        );
    }
    for text in [
        "",
        "nothing",
        "if ready",
        "if x { a(); }",
        "if x {",
        "if x {} else {",
        "if x:\n    a()",
        "if x:\n    a()\nafter()",
    ] {
        assert!(parse_if_else_block(text, 0).is_none(), "{text}");
    }
}

#[test]
fn coverage_b2_python_crlf_offsets_preserve_the_following_statement() {
    let text = "if x == 1:\r\n    a()\r\nelse:\r\n    b()\r\nafter()\r\n";
    let result = parse_if_else_block(text, 0).unwrap();
    assert_eq!(result.end_offset, text.find("after()").unwrap());
    assert_eq!(result.branches[0].body, "    a()");
    assert_eq!(result.branches[1].body, "    b()");
}

/// An inline suite's body sat on the header line: `f()` became part of the tag and `g()` was
/// dropped.
#[test]
fn an_inline_python_suite_is_refused() {
    let text = "if kind == 'a': f()\nelse: g()\n";
    assert!(parse_if_else_block(text, 0).is_none());
    // A `#` inside a string is not a comment that the colon could end before.
    let text = "if kind == 'a':\n    f()\nelse: log(\":#\")\n";
    assert!(parse_if_else_block(text, 0).is_none());
    // An f-string may nest quotes (Python 3.12), which the quote scan cannot follow.
    for header in [
        "else: log(f\"{\"a:#\"}\")",
        "elif kind == f\"{\"a:#\"}\": g()",
    ] {
        let text = format!("if kind == 'a':\n    f()\n{header}\n");
        assert!(parse_if_else_block(&text, 0).is_none(), "{header}");
    }
    // A real trailing comment after the colon is fine.
    let text = "if kind == 'a':  # first\n    f()\nelse:  # rest\n    g()\n";
    let tags: Vec<_> = parse_if_else_block(text, 0)
        .unwrap()
        .branches
        .into_iter()
        .map(|b| b.tag)
        .collect();
    assert_eq!(tags, ["'a'", "default"]);
}
