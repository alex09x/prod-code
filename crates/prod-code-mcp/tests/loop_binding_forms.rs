/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A name the extracted helper binds itself, in any pattern position, is not an enclosing local
//! it forgot to capture, though the enclosing function has a local of that name (#1934).

use prod_code_mcp::extract_function::binding::uncaptured_enclosing_locals;

/// The enclosing function has the locals `value`, `item`, `x` and `y`; `region` binds some of
/// those names itself.
fn flagged(region: &str) -> Vec<String> {
    let original = format!(
        "fn total(values: &[i32], pairs: &[(i32, i32)], opt: Option<i32>, value: i32, item: i32, x: i32, y: i32) -> i32 {{\n    let mut sum = value + item + x + y;\n{region}    sum\n}}\n"
    );
    let extracted = format!(
        "fn total(values: &[i32], pairs: &[(i32, i32)], opt: Option<i32>, value: i32, item: i32, x: i32, y: i32) -> i32 {{\n    let mut sum = value + item + x + y;\n    sum = fun_name(values, pairs, opt, sum);\n    sum\n}}\n\nfn fun_name(values: &[i32], pairs: &[(i32, i32)], opt: Option<i32>, mut sum: i32) -> i32 {{\n{region}    sum\n}}\n"
    );
    let start = original.find(region).unwrap();
    uncaptured_enclosing_locals(&original, start, &extracted, None)
}

#[test]
fn every_pattern_position_that_binds_a_name_inside_the_selection_is_its_own() {
    let regions = [
        "    while let Some(value) = Some(1) {\n        sum += value;\n        break;\n    }\n",
        "    if let Some(item) = opt {\n        sum += item;\n    }\n",
        "    match opt {\n        Some(item) => sum += item,\n        None => {}\n    }\n",
        "    sum += values.iter().map(|value| value + 1).sum::<i32>();\n",
        "    sum += values.iter().fold(0, |item, value| item + value);\n",
        "    for (x, y) in pairs {\n        sum += x * y;\n    }\n",
        "    for &(x, y) in pairs.iter() {\n        sum += x + y;\n    }\n",
        "    'outer: for value in values {\n        for item in values {\n            if *item == 0 {\n                continue 'outer;\n            }\n            sum += value + item;\n        }\n    }\n",
        "    for mut value in values.iter().copied() {\n        value += 1;\n        sum += value;\n    }\n",
        "    for value in values.iter().filter(|value| **value > 0) {\n        sum += value;\n    }\n",
        "    let value = sum * 2;\n    sum += value;\n",
        "    let (x, y) = (1, 2);\n    sum += x + y;\n",
        "    let Some(item) = opt else {\n        return 0;\n    };\n    sum += item;\n",
        "    if let (Some(x), Some(y)) = (opt, opt) {\n        sum += x + y;\n    }\n",
        "    let total = { let value = 3; value + 1 };\n    sum += total;\n",
    ];
    for region in regions {
        let flagged = flagged(region);
        assert!(
            flagged.is_empty(),
            "{flagged:?} flagged as an uncaptured local in:\n{region}"
        );
    }
}

#[test]
fn a_name_the_selection_only_reads_is_still_an_uncaptured_local() {
    // `value` is read, not bound, in the region: the helper must capture it.
    let flagged = flagged("    for item in values {\n        sum += item + value;\n    }\n");
    assert_eq!(flagged, vec!["value".to_string()], "{flagged:?}");
}
