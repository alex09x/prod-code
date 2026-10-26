/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "constructor_preservation/aggregate.rs"]
mod aggregate;
#[path = "constructor_preservation/aggregate_cross_file.rs"]
mod aggregate_cross_file;
#[path = "constructor_preservation/aggregate_repair.rs"]
mod aggregate_repair;
#[path = "constructor_preservation/aggregate_repair_support.rs"]
mod aggregate_repair_support;
#[path = "constructor_preservation/controls.rs"]
mod controls;
#[path = "constructor_preservation/glob_owners.rs"]
mod glob_owners;
#[path = "constructor_preservation/macros.rs"]
mod macros;
#[path = "constructor_preservation/member_boundaries.rs"]
mod member_boundaries;
#[path = "constructor_preservation/module_owner.rs"]
mod module_owner;
#[path = "constructor_preservation/polyglot.rs"]
mod polyglot;
#[path = "constructor_preservation/reference_failures.rs"]
mod reference_failures;
#[path = "constructor_preservation/review_4291.rs"]
mod review_4291;
#[path = "constructor_preservation/review_4291_support.rs"]
mod review_4291_support;
#[path = "constructor_preservation/rust.rs"]
mod rust;
#[path = "constructor_preservation/rust_regressions.rs"]
mod rust_regressions;
#[path = "constructor_preservation/support.rs"]
mod support;
