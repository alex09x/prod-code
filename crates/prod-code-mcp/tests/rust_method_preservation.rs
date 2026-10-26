/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "rust_method_preservation/aggregate.rs"]
mod aggregate;
#[path = "rust_method_preservation/associated.rs"]
mod associated;
#[path = "rust_method_preservation/call_trivia.rs"]
mod call_trivia;
#[path = "rust_method_preservation/conversion.rs"]
mod conversion;
#[path = "rust_method_preservation/declaration_identity.rs"]
mod declaration_identity;
#[path = "rust_method_preservation/guards.rs"]
mod guards;
#[path = "rust_method_preservation/macro_inputs.rs"]
mod macro_inputs;
#[path = "rust_method_preservation/namespace_values.rs"]
mod namespace_values;
#[path = "rust_method_preservation/owner_boundary_regressions.rs"]
mod owner_boundary_regressions;
#[path = "rust_method_preservation/owner_capture.rs"]
mod owner_capture;
#[path = "rust_method_preservation/owner_header_regressions.rs"]
mod owner_header_regressions;
#[path = "rust_method_preservation/receiver.rs"]
mod receiver;
#[path = "rust_method_preservation/remaining_regressions.rs"]
mod remaining_regressions;
#[path = "rust_method_preservation/review_regressions.rs"]
mod review_regressions;
#[path = "rust_method_preservation/support.rs"]
mod support;
#[path = "rust_method_preservation/type_identity.rs"]
mod type_identity;
