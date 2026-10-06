/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod cpp;
mod python;
mod rust;
mod swift;
mod typescript;

pub use cpp::parse_cpp_classes;
pub use python::parse_python_classes;
pub use rust::parse_rust_traits;
pub use swift::parse_swift_classes;
pub use typescript::parse_ts_classes;

use crate::pull_push::types::ClassDecl;
use std::path::Path;

/// Parse classes and traits from source text based on language.
pub fn parse_classes_in_text(text: &str, language: &str, file_path: &Path) -> Vec<ClassDecl> {
    match language {
        "python" => parse_python_classes(text, file_path),
        "typescript" | "javascript" => parse_ts_classes(text, file_path, language),
        "cpp" | "c" => parse_cpp_classes(text, file_path),
        "swift" => parse_swift_classes(text, file_path),
        "rust" => parse_rust_traits(text, file_path),
        _ => Vec::new(),
    }
}
