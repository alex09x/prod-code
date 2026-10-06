/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod csharp;
pub mod go;
pub mod java;
pub mod python;
pub mod rust;
pub mod typescript;

pub use csharp::parse_csharp_imports;
pub use go::parse_go_imports;
pub use java::parse_java_imports;
pub use python::parse_python_imports;
pub use rust::parse_rust_imports;
pub use typescript::parse_ts_imports;
