/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Extracting a delegate: some fields of a struct and the methods that work only on them move
//! into a new helper type, which the struct then holds (Extract Class).

pub mod common;
pub mod polyglot;
pub mod rust;

#[cfg(test)]
mod tests;

use std::net::SocketAddr;
use std::path::Path;

use anyhow::Result;

pub use common::{display, is_ident, is_ident_str, reindent, split_top};
pub use polyglot::{
    extract_delegate_polyglot, extract_param_names_cpp, extract_param_names_go,
    extract_param_names_py, extract_param_names_swift, extract_param_names_ts, owner_region,
    restructure_cpp, restructure_go, restructure_py, restructure_swift, restructure_ts,
    rewrite_external_file, rewrite_go_literals,
};
pub use rust::{
    Extracted, Field, StructDecl, argument_names, extract_delegate_rust, impl_blocks, parse_struct,
    restructure, rewrite_literals,
};

/// Backwards-compatible entry point for extract_delegate.
#[allow(clippy::too_many_arguments)]
pub async fn extract_delegate(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    apply: bool,
    force: bool,
) -> Result<Extracted> {
    extract_delegate_polyglot(
        remote,
        root,
        file,
        None,
        Some(line),
        Some(col),
        fields,
        methods,
        helper,
        field,
        apply,
        force,
        None,
    )
    .await
}
