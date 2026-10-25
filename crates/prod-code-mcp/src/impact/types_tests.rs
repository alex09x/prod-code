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
fn coverage_b2_gap_descriptions() {
    let symbol = Symbol {
        name: "f".into(),
        file: "src.rs".into(),
        line: 3,
        col: 4,
    };
    for (gap, expected) in [
        (
            Gap::Deleted {
                file: "src.rs".into(),
            },
            "src.rs was deleted, so what called into it is unknown",
        ),
        (
            Gap::Diff {
                file: "src.rs".into(),
                error: "binary".into(),
            },
            "the lines the change to src.rs touches are unknown: binary",
        ),
        (
            Gap::Symbols {
                file: "src.rs".into(),
                error: "offline".into(),
            },
            "the functions of src.rs could not be listed: offline",
        ),
        (
            Gap::Callers {
                symbol: symbol.clone(),
                error: "offline".into(),
            },
            "the callers of f (src.rs:3) are unknown: offline",
        ),
        (
            Gap::Depth {
                symbol: symbol.clone(),
                depth: 5,
            },
            "the walk stopped at depth 5 while f (src.rs:3) still had callers",
        ),
        (
            Gap::FanIn {
                symbol,
                callers: 31,
                limit: 30,
            },
            "f (src.rs:3) has 31 callers, exceeding the fan-in limit of 30; stopped to avoid dispatcher explosion",
        ),
        (
            Gap::Timeout {
                elapsed_ms: 12,
                budget_ms: 10,
            },
            "the impact analysis traversal reached its time budget (12ms >= 10ms); returning accumulated callers and tests",
        ),
    ] {
        assert_eq!(gap.describe(), expected);
    }
}
