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
pub mod go;
pub mod python;
pub mod swift;
pub mod ts;

pub use cpp::extract_interface_cpp;
pub use go::extract_interface_go;
pub use python::extract_interface_python;
pub use swift::extract_interface_swift;
pub use ts::extract_interface_ts;
