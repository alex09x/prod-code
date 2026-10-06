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
pub mod python;
pub mod swift;
pub mod ts;

pub use cpp::recognise_cpp;
pub use execute::loop_to_iterator_polyglot;
pub use go::recognise_go;
pub use python::recognise_python;
pub use swift::recognise_swift;
pub use ts::recognise_ts;
