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
pub mod python;
pub mod swift;
pub mod typescript;

pub use cpp::transform_cpp;
pub use python::transform_python;
pub use swift::transform_swift;
pub use typescript::transform_typescript;
