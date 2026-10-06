/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod compile;
pub mod execute;
pub mod lexer;
pub mod lsp;
pub mod parser;
pub mod sources;
pub mod types;

pub use execute::delete_function;
pub use types::DeletedFunction;
