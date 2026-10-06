/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub(crate) mod directives;
pub(crate) mod execute;
pub(crate) mod lex;
pub(crate) mod packages;
pub(crate) mod parse;
pub(crate) mod receiver;
pub(crate) mod references;
pub(crate) mod types;

pub use self::execute::delete_function;
pub use self::types::DeletedFunction;
