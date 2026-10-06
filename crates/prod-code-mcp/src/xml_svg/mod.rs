/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Well-formedness checks for XML and SVG vector graphics files.

pub mod lexer;
pub mod parser;

#[cfg(test)]
mod tests;

pub use parser::validate_xml;
