/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::dossier::{locations_in, parse_assertion_evidence};

#[test]
fn python_traceback_keeps_the_deepest_project_frame_first() {
    let temp = tempfile::tempdir().unwrap();
    for file in ["test_case.py", "helper.py"] {
        std::fs::write(temp.path().join(file), "# project frame\n").unwrap();
    }
    let trace = "Traceback (most recent call last):\n  File \"test_case.py\", line 4, in test_case\n    helper()\n  File \"helper.py\", line 8, in helper\n    assert False\nAssertionError\n";
    assert_eq!(
        locations_in(temp.path(), trace),
        [("helper.py".into(), 8), ("test_case.py".into(), 4)]
    );
}

#[test]
fn pytest_custom_message_does_not_hide_the_rewritten_assertion() {
    let output =
        "E   AssertionError: custom message\nE   assert 1 == 2\n\ntest_case.py:4: AssertionError\n";
    let evidence = parse_assertion_evidence(output).unwrap();
    assert_eq!(evidence.left.as_deref(), Some("1"));
    assert_eq!(evidence.right.as_deref(), Some("2"));
    assert_eq!(evidence.expression.as_deref(), Some("assert 1 == 2"));
}
