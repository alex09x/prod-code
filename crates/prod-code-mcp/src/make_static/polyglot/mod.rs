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
pub mod py;
pub mod swift;
pub mod ts;

pub use cpp::make_static_cpp;
pub use execute::make_static_polyglot;
pub use go::make_static_go;
pub use py::make_static_py;
pub use swift::make_static_swift;
pub use ts::make_static_ts;
