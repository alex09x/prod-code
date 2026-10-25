/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The conditional-to-polymorphism model: when a conditional's branches can be split, how tags
//! become class names, how positions map to offsets, and how the outcome is reported.

use prod_code_mcp::replace_conditional::types::{
    ConditionalBlock, ConditionalBranch, ConditionalKind, ReplaceConditionalResult, contains_word,
    line_col_to_offset, line_indentation, returns_from_conditional, tag_to_variant_name,
};

fn block(branches: &[(&str, bool)]) -> ConditionalBlock {
    ConditionalBlock {
        kind: ConditionalKind::Switch,
        start_offset: 0,
        end_offset: 0,
        discriminator: "kind".to_string(),
        indent: String::new(),
        branches: branches
            .iter()
            .enumerate()
            .map(|(i, (body, is_default))| ConditionalBranch {
                tag: format!("T{i}"),
                variant_name: format!("T{i}"),
                body: body.to_string(),
                is_default: *is_default,
            })
            .collect(),
    }
}

#[test]
fn branches_may_all_return_or_none_but_never_a_mix() {
    let complex = "branch control flow is too complex to preserve safely; each branch must return or terminate directly, or none may return";
    let fallthrough = "a returning conditional without a default branch cannot be converted without changing fallthrough behavior";
    let mixed =
        "conditional branches mix returns and statements; control flow cannot be preserved safely";
    for (branches, expected) in [
        (vec![], Err("conditional has no branches")),
        (vec![("return 1;", false), ("return 2;", true)], Ok(true)),
        (vec![("return;", false), ("return", true)], Ok(true)),
        (
            vec![("return 1;", false), ("return 2;", false)],
            Err(fallthrough),
        ),
        (vec![("x = 1;", false), ("x = 2;", true)], Ok(false)),
        // A word that merely starts like `return` is not one.
        (vec![("returned = 1;", false), ("x = 2;", false)], Ok(false)),
        (vec![("return 1;", false), ("x = 2;", true)], Err(mixed)),
        (
            vec![("if a { return 1; }", true), ("return 2;", false)],
            Err(complex),
        ),
        (vec![("x = 1; return x;", false)], Err(complex)),
        // Leaving by throwing, raising or aborting counts as returning.
        (
            vec![("throw new Error(\"x\")", false), ("return 1;", true)],
            Ok(true),
        ),
        (
            vec![("throw;", false), ("raise ValueError()", true)],
            Ok(true),
        ),
        (
            vec![("panic(\"x\")", false), ("panic!(\"y\")", true)],
            Ok(true),
        ),
        (
            vec![("fatalError(\"x\")", false), ("fatalError \"y\"", true)],
            Ok(true),
        ),
        (vec![("raise\n", false), ("raise\r\n", true)], Ok(true)),
        (vec![("raise_x()", false), ("y = 1", false)], Ok(false)),
    ] {
        let got = returns_from_conditional(&block(&branches)).map_err(|e| e.to_string());
        assert_eq!(got, expected.map_err(str::to_string), "{branches:?}");
    }
    assert!(contains_word("a return b", "return"));
    assert!(contains_word("return", "return"));
    assert!(!contains_word("returned", "return"));
    assert!(!contains_word("do_return", "return"));
    assert!(!contains_word("_return", "return"));
}

#[test]
fn tags_become_class_names() {
    for (tag, expected) in [
        ("\"EUROPEAN\"", "European"),
        ("'african'", "African"),
        ("`x_y-z`", "XYZ"),
        (".circle", "Circle"),
        ("Shape::Circle", "Circle"),
        ("HTTP_ERROR", "HttpError"),
        ("camelCase", "CamelCase"),
        ("ünï", "Ünï"),
        ("default", "Default"),
        ("_", "Default"),
        ("", "Default"),
        ("  ", "Default"),
        ("\"\"", "Default"),
        ("42", "Case42"),
        ("2d_mode", "Case2dMode"),
        ("!!!", "Variant"),
    ] {
        assert_eq!(tag_to_variant_name(tag), expected, "{tag:?}");
    }
}

#[test]
fn indentation_and_positions_are_measured_in_the_editors_units() {
    let text = "    foo\n\t\tbar baz\nlast";
    assert_eq!(line_indentation(text, 4), "    ");
    assert_eq!(line_indentation(text, text.find("baz").unwrap()), "\t\t");
    assert_eq!(line_indentation(text, 8), "");
    assert_eq!(line_indentation(text, text.len()), "");

    for (text, line, col, expected) in [
        ("ab\ncd", 1, 1, Some(0)),
        ("ab\ncd", 2, 2, Some(4)),
        ("ab\ncd", 1, 3, Some(2)),
        ("ab\ncd", 1, 4, None),
        ("ab\ncd", 3, 1, None),
        ("ab\ncd", 2, 3, Some(5)),
        ("ab\ncd", 0, 1, None),
        ("ab\ncd", 1, 0, None),
        ("", 1, 1, None),
        // A carriage return is not a column.
        ("ab\r\ncd", 1, 3, Some(2)),
        ("ab\r\ncd", 1, 4, None),
        ("ab\r\ncd", 2, 1, Some(4)),
        // An astral character is two columns; a position inside it is no position.
        ("a😀b", 1, 2, Some(1)),
        ("a😀b", 1, 3, None),
        ("a😀b", 1, 4, Some(5)),
        ("a😀b", 1, 5, Some(6)),
    ] {
        assert_eq!(
            line_col_to_offset(text, line, col),
            expected,
            "{text:?} {line}:{col}"
        );
    }
}

#[test]
fn the_outcome_lists_variants_files_diagnostics_and_a_bounded_diff() {
    let result = ReplaceConditionalResult {
        base_name: "Shape".to_string(),
        method_name: "area".to_string(),
        variants: vec!["Circle".to_string(), "Square".to_string()],
        files_modified: vec!["a.ts".to_string(), "b.ts".to_string()],
        overlays: Vec::new(),
        diff: "-old\n+new\n".to_string(),
        applied: true,
        verified: false,
        diagnostics: vec!["boom".to_string(), "bang".to_string()],
    };
    let head = "`Shape` — replaced conditional with polymorphism (method: `area`)\n- variants (2): Circle, Square\n- files modified (2): a.ts, b.ts\n- applied: true\n- verified: false\n";
    assert_eq!(
        result.render(100),
        format!(
            "{head}- diagnostics (2):\n  • boom\n  • bang\n\nDiff:\n```diff\n-old\n+new\n```\n"
        )
    );
    assert_eq!(
        result.render(4),
        format!(
            "{head}- diagnostics (2):\n  • boom\n  • bang\n\nDiff:\n```diff\n-old\n... [truncated]\n```\n"
        )
    );
    let bare = ReplaceConditionalResult {
        variants: Vec::new(),
        diagnostics: Vec::new(),
        diff: String::new(),
        applied: false,
        ..result
    };
    assert_eq!(
        bare.render(100),
        "`Shape` — replaced conditional with polymorphism (method: `area`)\n- variants (0): none\n- files modified (2): a.ts, b.ts\n- applied: false\n- verified: false\n"
    );
}
