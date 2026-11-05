/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */
use super::missing_module_dependencies;

#[test]
fn reports_module_globals_used_by_the_moved_function() {
    let source = "LIMIT = 10\ndef helper(x):\n    return x + 1\ndef entry(x):\n    return helper(x) + LIMIT\n";
    let item = "def entry(x):\n    return helper(x) + LIMIT\n";
    assert_eq!(
        missing_module_dependencies(source, item, "", "entry"),
        ["LIMIT", "helper"]
    );
}

#[test]
fn ignores_names_already_bound_in_target_or_imported() {
    let source = "LIMIT = 10\ndef helper(x):\n    return x + 1\ndef entry(x):\n    return helper(x) + LIMIT\n";
    let item = "def entry(x):\n    return helper(x) + LIMIT\n";
    let target = "from example import helper\nLIMIT = 20\n";
    assert!(missing_module_dependencies(source, item, target, "entry").is_empty());
}
