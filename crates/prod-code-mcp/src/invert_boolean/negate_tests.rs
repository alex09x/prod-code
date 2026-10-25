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
fn coverage_negate_expression_tables() {
    for (input, expected) in [
        (" True ", "False"),
        ("False", "True"),
        ("not ready", "ready"),
        ("not ( α )", "α"),
        ("not(ready)", "ready"),
        ("not (a) and (b)", "(a) and (b)"),
        ("not (a", "(a"),
        ("not(a) or (b)", "not (not(a) or (b))"),
        ("a == b", "not (a == b)"),
        ("", "not ()"),
    ] {
        assert_eq!(negate_expr_python(input), expected, "{input}");
    }
    for (input, expected) in [
        (" true ", "false"),
        ("false", "true"),
        ("! ready", "ready"),
        ("!( α )", "α"),
        ("!(a) && (b)", "(a) && (b)"),
        ("!(a", "(a"),
        ("!= x", "!(!= x)"),
        ("!== x", "!(!== x)"),
        ("a == b", "!(a == b)"),
        ("", "!()"),
    ] {
        assert_eq!(negate_expr_c_like(input), expected, "{input}");
    }
}

#[test]
fn coverage_negate_bodies_preserve_nested_functions() {
    let python = "    def nested():\n        return True\n\n    async def later():\n        return False\n    return not (ready)\n    return False";
    assert_eq!(
        negate_python_body(python),
        "    def nested():\n        return True\n\n    async def later():\n        return False\n    return ready\n    return True"
    );
    assert_eq!(
        negate_python_body("return True\r\nreturn False\r\n"),
        "return False\nreturn True"
    );
    for (input, expected) in [
        ("", ""),
        ("  α  ", "\n    !(α)\n"),
        ("return true;", "return false;"),
        (
            "if (x) { return false; } return !ready;",
            "if (x) { return true; } return ready;",
        ),
        ("return;", "return;"),
        ("return true", "return false"),
    ] {
        assert_eq!(negate_c_like_body(input), expected, "{input}");
    }
}
