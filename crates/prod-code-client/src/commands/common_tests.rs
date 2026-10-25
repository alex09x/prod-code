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
fn argument_shapes_and_position_refusals() {
    let path = PathBuf::from("missing-source.rs");
    assert_eq!(
        position(Some(path.clone()), Some(0), Some(u32::MAX)).unwrap(),
        (path.clone(), 0, u32::MAX)
    );
    for (file, line, col) in [
        (None, Some(1), Some(2)),
        (Some(path.clone()), None, Some(2)),
        (Some(path.clone()), Some(1), None),
        (None, None, None),
    ] {
        assert_eq!(
            position(file, line, col).unwrap_err().to_string(),
            "give <file> <line> <col>, or --symbol NAME"
        );
    }
    assert_eq!(symbol_args("λ", None), serde_json::json!({"symbol":"λ"}));
    assert_eq!(
        symbol_args("A", Some(path.clone())),
        serde_json::json!({"symbol":"A", "path":"missing-source.rs"})
    );
    for names in [vec![], vec!["A".into()], vec!["A".into(), "B".into()]] {
        let expected = if names.len() == 1 {
            serde_json::json!({"symbol":"A"})
        } else {
            serde_json::json!({"symbols":names})
        };
        assert_eq!(symbols_args(&names, None), expected);
        let mut with_path = expected;
        with_path["path"] = serde_json::json!("missing-source.rs");
        assert_eq!(symbols_args(&names, Some(path.clone())), with_path);
    }
}
#[test]
fn line_col_and_unicode_truncation_table() {
    for (input, expected) in [("1:2", (1, 2)), (" 0 : 4294967295 ", (0, u32::MAX))] {
        assert_eq!(parse_line_col(input).unwrap(), expected);
    }
    for (input, expected) in [
        ("12", "expected LINE:COL, got 12"),
        ("x:1", "invalid digit found in string"),
        ("1:", "cannot parse integer from empty string"),
        ("1:2:3", "invalid digit found in string"),
    ] {
        assert_eq!(parse_line_col(input).unwrap_err().to_string(), expected);
    }
    for (input, n, expected) in [
        ("", 0, ""),
        ("λ猫ab", 3, "λ猫…"),
        ("λ猫", 2, "λ猫"),
        ("abc", 1, "…"),
        ("abc", 0, "…"),
    ] {
        assert_eq!(truncate(input, n), expected);
    }
}
