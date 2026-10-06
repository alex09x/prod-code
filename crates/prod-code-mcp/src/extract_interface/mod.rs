/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Polyglot interface extraction across TypeScript/JavaScript, Go, Python, C++, Swift, and Rust (Roadmap 7.1).
//!
//! Extracts method signatures from classes/structs into an interface/protocol/abstract base class,
//! updates the type declaration to implement/inherit/conform to the interface, and pre-validates
//! edits with in-memory overlays and optional compiler verification. For Rust, dispatches directly
//! to `crate::extract_trait::extract_trait`.

pub mod execute;
pub mod languages;
pub mod types;

#[cfg(test)]
mod tests;

pub use execute::extract_interface_impl;
pub use languages::{
    extract_interface_cpp, extract_interface_go, extract_interface_python, extract_interface_swift,
    extract_interface_ts,
};
pub use types::ExtractInterfaceResult;
