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
fn disabled_timing_does_not_record_phases() {
    let now = std::time::Instant::now();
    let mut timing = QueryTiming {
        enabled: false,
        label: "unit",
        start: now,
        last: now,
        phases: vec![],
    };
    timing.mark("ignored");
    timing.report();
    assert_eq!(timing.phases, vec![]);
    assert_eq!(timing.last, now);
}
#[test]
fn enabled_timing_records_order_and_advances_boundary() {
    let now = std::time::Instant::now();
    let mut timing = QueryTiming {
        enabled: true,
        label: "unit",
        start: now,
        last: now,
        phases: vec![],
    };
    timing.mark("connect");
    let first = timing.last;
    timing.mark("response");
    assert_eq!(
        timing
            .phases
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        ["connect", "response"]
    );
    assert_eq!(timing.phases[0].1, (first - now).as_secs_f64() * 1000.0);
    assert_eq!(
        timing.phases[1].1,
        (timing.last - first).as_secs_f64() * 1000.0
    );
    timing.report();
    assert_eq!(timing.label, "unit");
}
