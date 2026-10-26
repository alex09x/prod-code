/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "encapsulate_preservation/aggregate.rs"]
mod aggregate;
#[path = "encapsulate_preservation/boundaries.rs"]
mod boundaries;
#[path = "encapsulate_preservation/context.rs"]
mod context;
#[path = "encapsulate_preservation/context_bom.rs"]
mod context_bom;
#[path = "encapsulate_preservation/context_fstrings.rs"]
mod context_fstrings;
#[path = "encapsulate_preservation/context_review.rs"]
mod context_review;
#[path = "encapsulate_preservation/control_rhs.rs"]
mod control_rhs;
#[path = "refactor_shapes_a_1_encapsulate_flow.rs"]
mod existing_flow;
#[path = "encapsulate_preservation/identity.rs"]
mod identity;
#[path = "encapsulate_preservation/literals_and_generics.rs"]
mod literals_and_generics;
#[path = "encapsulate_preservation/ownership.rs"]
mod ownership;
#[path = "encapsulate_preservation/residuals.rs"]
mod residuals;
#[path = "encapsulate_preservation/review.rs"]
mod review;
#[path = "encapsulate_preservation/review_regressions.rs"]
mod review_regressions;
#[path = "encapsulate_preservation/rust.rs"]
mod rust;
#[path = "encapsulate_preservation/safety.rs"]
mod safety;
#[path = "encapsulate_preservation/support.rs"]
mod support;
