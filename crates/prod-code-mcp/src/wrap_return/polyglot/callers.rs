/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod identity;
mod promise;
mod scope;
mod symbols;
mod syntax;

#[cfg(test)]
mod tests;

pub(super) mod collect;
#[cfg(test)]
#[path = "callers/coverage_w2.rs"]
mod coverage_w2;
