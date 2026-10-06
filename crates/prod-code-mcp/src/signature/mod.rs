/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Change a function's parameter list, with every call site (roadmap 7.1.1).

pub mod call_sites;
pub mod change;
pub mod effects;
pub mod modifiers;
pub mod parse;
pub mod plan;
pub mod references;
pub mod render;
pub mod rewrite;
pub mod types;
pub mod util;

#[cfg(test)]
mod tests;

pub use call_sites::*;
pub use change::*;
pub use effects::*;
pub use modifiers::*;
pub use parse::*;
pub use plan::*;
pub use references::*;
pub use rewrite::*;
pub use types::*;
pub use util::*;
