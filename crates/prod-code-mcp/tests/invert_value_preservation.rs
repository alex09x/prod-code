/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "invert_value_preservation/cases.rs"]
mod cases;
#[path = "invert_value_preservation/proof.rs"]
mod proof;
#[path = "invert_value_preservation/support.rs"]
mod support;

#[tokio::test]
async fn original4079730933_default_compile_gate() {
    support::matrix(cases::default_case("#[derive(Default)]", "single-default")).await;
}

#[tokio::test]
async fn original4079730939_attached_multiline_derives() {
    for case in cases::derives() {
        support::matrix(case).await;
    }
}

#[tokio::test]
async fn original4079730941_grouped_borrow_places() {
    for case in cases::borrows() {
        support::matrix(case).await;
    }
}

#[tokio::test]
async fn ordinary_inversions_remain_useful() {
    for case in cases::positive() {
        support::matrix(case).await;
    }
}

#[tokio::test]
async fn force_never_overrides_unmatched_or_unverified_references() {
    support::incomplete().await;
}
