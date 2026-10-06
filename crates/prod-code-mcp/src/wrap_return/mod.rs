/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Wrapping what a function returns in `Option`, `Result`, `Promise`, or `Pointer`, with callers.
//!
//! rust-analyzer's `wrap_return_type_in_option` / `wrap_return_type_in_result` rewrite the
//! signature and every value the function returns for Rust, touching no caller. Here, for Rust:
//! a caller that itself returns an `Option` (or a `Result`) gets `?` after the call; any other
//! caller cannot, and is reported with its line, because turning a `None` or an error into
//! something else there is a decision, not a rewrite.
//!
//! Across TypeScript/JavaScript, Python, C++, Swift, and Go:
//! - TypeScript/JavaScript: supports `promise` (`Promise<T>`, adding `async` to declaration,
//!   rewriting callers to `await call(...)`), `option`/`nullable` (`T | null`), `result` (`Result<T, E>`).
//! - Python: supports `option`/`optional` (`Optional[T]`), `result` (`Result[T, E]`, wrapping return values in `Ok(...)`).
//! - C++: supports `option`/`optional` (`std::optional<T>`), `result`/`expected` (`std::expected<T, E>`).
//! - Swift: supports `option`/`optional` (`T?`), `result` (`Result<T, Error>`, wrapping return values in `.success(...)`).
//! - Go: supports `result`/`error` (`(T, error)` with `return expr, nil`), `pointer`/`option` (`*T`).
//!
//! Refuses when the function already returns the target wrapper, or when callers cannot
//! propagate without `force`.

pub mod polyglot;
pub mod rust;
#[cfg(test)]
mod tests;
pub mod types;
pub mod utils;

pub use polyglot::{
    enclosing_polyglot_info, find_polyglot_decl, restructure_declaring_file, wrap_polyglot,
    wrap_polyglot_ext,
};
pub use rust::{wrap_rust, wrap_rust_ext};
pub use types::{PolyglotFuncDecl, WrappedReturn, Wrapper};
pub use utils::{declared_return, enclosing_return_type, propagates};

use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

/// Backward compatibility wrapper.
#[allow(clippy::too_many_arguments)]
pub async fn wrap(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    wrapper: Wrapper,
    error: Option<&str>,
    apply: bool,
    force: bool,
) -> Result<WrappedReturn> {
    wrap_polyglot_ext(
        remote,
        root,
        file,
        None,
        Some(line),
        Some(col),
        wrapper,
        None,
        error,
        apply,
        force,
    )
    .await
}
