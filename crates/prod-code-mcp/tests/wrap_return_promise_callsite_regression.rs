/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Promise caller regressions for #1280, through planning and application.

#[path = "wrap_return_promise_callsite_regression/contextual_of.rs"]
mod contextual_of;
#[path = "wrap_return_promise_callsite_regression/identity.rs"]
mod identity;
#[path = "wrap_return_promise_callsite_regression/issue_1346.rs"]
mod issue_1346;
#[path = "wrap_return_promise_callsite_regression/outer_await_context.rs"]
mod outer_await_context;
#[path = "wrap_return_promise_callsite_regression/parameter_scope.rs"]
mod parameter_scope;
#[path = "wrap_return_promise_callsite_regression/postfix.rs"]
mod postfix;
#[path = "wrap_return_promise_callsite_regression/residual.rs"]
mod residual;
#[path = "wrap_return_promise_callsite_regression/review.rs"]
mod review;
#[path = "wrap_return_promise_callsite_regression/short_circuit.rs"]
mod short_circuit;
#[path = "wrap_return_promise_callsite_regression/support.rs"]
mod support;
#[path = "wrap_return_promise_callsite_regression/token_boundaries.rs"]
mod token_boundaries;
#[path = "wrap_return_promise_callsite_regression/yield_scope.rs"]
mod yield_scope;
