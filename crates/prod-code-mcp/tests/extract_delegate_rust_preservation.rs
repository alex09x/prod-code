/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "extract_delegate_rust_preservation/const_impl_header.rs"]
mod const_impl_header;
#[path = "extract_delegate_rust_preservation/modifier_and_receivers.rs"]
mod modifier_and_receivers;
#[path = "extract_delegate_rust_preservation/module_identity.rs"]
mod module_identity;
#[path = "extract_delegate_rust_preservation/nominal_identity.rs"]
mod nominal_identity;
#[path = "extract_delegate_rust_preservation/nominal_support.rs"]
mod nominal_support;
#[path = "extract_delegate_rust_preservation/refusals.rs"]
mod refusals;
#[path = "extract_delegate_rust_preservation/remaining_bindings.rs"]
mod remaining_bindings;
#[path = "extract_delegate_rust_preservation/review_regressions.rs"]
mod review_regressions;
#[path = "extract_delegate_rust_preservation/support.rs"]
mod support;
#[path = "extract_delegate_rust_preservation/target_identity.rs"]
mod target_identity;
#[path = "extract_delegate_rust_preservation/transformations.rs"]
mod transformations;
