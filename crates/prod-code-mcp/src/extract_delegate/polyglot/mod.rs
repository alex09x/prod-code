/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

pub mod cpp;
pub mod execute;
pub mod go;
pub mod params;
pub mod py;
pub mod rewrite;
pub mod swift;
pub mod ts;

pub use cpp::restructure_cpp;
pub use execute::extract_delegate_polyglot;
pub use go::{restructure_go, rewrite_go_literals};
pub use params::{
    extract_param_names_cpp, extract_param_names_go, extract_param_names_py,
    extract_param_names_swift, extract_param_names_ts,
};
pub use py::restructure_py;
pub use rewrite::{owner_region, rewrite_external_file};
pub use swift::restructure_swift;
pub use ts::restructure_ts;
