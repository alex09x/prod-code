/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Replace inheritance with delegation across polyglot OOP languages (Roadmap 7.1.3).
//!
//! Enforces "Composition over Inheritance" by:
//! 1. Decoupling the subclass from its base class (`extends`, `:`, `(...)`).
//! 2. Introducing an encapsulated private delegate field holding the base class instance.
//! 3. Initializing the delegate in constructors / `__init__` (replacing `super()` calls).
//! 4. Auto-generating forwarding methods for inherited base methods to maintain API compatibility.
//! 5. Stripping invalid `override` modifiers from subclass methods and rewriting internal `super.` calls.
//!
//! Supports Python, TypeScript / JavaScript, C++, and Swift.

mod execute;
pub mod languages;
#[cfg(test)]
mod tests;
mod types;

pub use execute::replace_inheritance_impl;
pub use types::ReplaceInheritanceResult;
