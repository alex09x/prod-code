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
#[tokio::test]
async fn unplaced_checkout_still_validates_report_title() {
    let error = run_report_issue(
        None,
        Some("first reason\nsecond reason".into()),
        ReportArgs {
            title: "short".into(),
            body: Some(
                "Ran a neutral query, got no result, expected a matching declaration.".into(),
            ),
            body_file: None,
            private_ref: None,
            force: false,
            dry_run: true,
            allow_private_names: true,
            labels: vec![],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "the title is too short to search for: say what went wrong and where, e.g. `code_references returns nothing for a field in a Go struct`"
    );
}
