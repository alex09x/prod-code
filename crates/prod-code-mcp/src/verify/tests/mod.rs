/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[cfg(unix)]
mod clang_tidy_paths;
mod diagnostic_budget;
mod failure_cause;
mod matrix;
mod parse;
mod plan;
mod render;
mod wave2;
mod wave2_js;
