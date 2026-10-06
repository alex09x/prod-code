/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The call hierarchy as a tree (roadmap 7.5): who calls a function, who calls those, and so on
//! to a depth; or what it calls, transitively.

pub mod cache;
pub mod types;
pub mod walk;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub(crate) use cache::{CALL_CACHE, CallCacheEntry};
pub use cache::{MAX_DEPTH, MAX_NODES, clear_call_hierarchy_cache, clear_call_hierarchy_cache_for};
pub use types::{CallTree, Node};
pub use walk::call_tree;
