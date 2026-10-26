/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "wrap_return_nonjs_preservation/cpp.rs"]
mod cpp;
#[path = "wrap_return_nonjs_preservation/final_review.rs"]
mod final_review;
#[path = "wrap_return_nonjs_preservation/prototype_review.rs"]
mod prototype_review;
#[path = "wrap_return_nonjs_preservation/python.rs"]
mod python;
#[path = "wrap_return_nonjs_preservation/recovered.rs"]
mod recovered;
#[path = "wrap_return_nonjs_preservation/remaining_review.rs"]
mod remaining_review;
#[path = "wrap_return_nonjs_preservation/review.rs"]
mod review;
#[path = "wrap_return_nonjs_preservation/support.rs"]
mod support;
#[path = "wrap_return_nonjs_preservation/swift.rs"]
mod swift;
