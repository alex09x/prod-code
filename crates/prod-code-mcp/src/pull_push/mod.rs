/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Pull up and push down refactorings across polyglot OOP and trait hierarchies (Roadmap 7.1.3).
//!
//! Supports class and trait hierarchies in:
//! - Python (`class Sub(Super):`)
//! - TypeScript / JavaScript (`class Sub extends Super`)
//! - C++ (`class Sub : public Super`)
//! - Swift (`class Sub: Super`)
//! - Rust (`trait Sub: Super`)
//!
//! Provides AST-aware member relocation, sibling deduplication, override modifier adjustment,
//! conflict detection, analyzer overlay verification, and transactional WorkspaceEdit application.

mod braces;
mod format;
pub mod languages;
mod pull_up;
mod push_down;
mod sibling;
mod types;
mod workspace;

#[cfg(test)]
mod tests;

pub use braces::find_matching_brace;
pub use format::{adjust_indentation, strip_override_modifiers};
pub use languages::parse_classes_in_text;
pub use pull_up::pull_up_impl;
pub use push_down::push_down_impl;
pub use types::{ClassDecl, HierarchyRefactorResult, MemberDecl, MemberKind};
pub use workspace::{find_class_in_workspace, find_subclasses_in_workspace};
