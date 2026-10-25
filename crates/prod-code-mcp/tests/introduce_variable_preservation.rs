/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "introduce_variable_preservation/analyzer.rs"]
mod analyzer;
#[path = "introduce_variable_preservation/cases.rs"]
mod cases;
#[path = "introduce_variable_preservation/macro_cases.rs"]
mod macro_cases;
#[path = "introduce_variable_preservation/pattern_cases.rs"]
mod pattern_cases;
#[path = "introduce_variable_preservation/support.rs"]
mod support;
#[path = "introduce_variable_preservation/unicode_cases.rs"]
mod unicode_cases;

use support::{exercise, preserve};

#[tokio::test]
async fn executable_literals_preserve_patterns() {
    pattern_cases::preserve().await;
}

#[tokio::test]
async fn canonical_identifier_freshness() {
    for case in unicode_cases::collisions() {
        exercise(case).await;
    }
    for case in unicode_cases::positive() {
        preserve(case).await;
    }
}

#[tokio::test]
async fn review_followups() {
    for case in cases::followups() {
        exercise(case).await;
    }
}

#[tokio::test]
async fn macro_capture_proofs() {
    if std::env::var_os("INTRODUCE_VARIABLE_BASELINE").is_none() {
        for case in macro_cases::uncertain() {
            exercise(case).await;
        }
        for case in macro_cases::positive() {
            preserve(case).await;
        }
    }
}

#[tokio::test]
async fn seven_public_review_regressions() {
    for case in cases::regressions() {
        exercise(case).await;
    }
}

#[tokio::test]
async fn positive_controls() {
    for case in cases::positive() {
        preserve(case).await;
    }
}

#[tokio::test]
async fn uncertainty_and_evaluation_boundaries() {
    if std::env::var_os("INTRODUCE_VARIABLE_BASELINE").is_none() {
        for case in cases::boundaries() {
            exercise(case).await;
        }
        support::missing_evidence().await;
    }
}
