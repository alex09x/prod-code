/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};

use super::c_cpp::introduce_c;
use super::polyglot::introduce_in;
use super::rust_introduce::introduce_rust;
use super::types::{Language, ParameterObject};

/// Bundles `params` of the function at `file:line:col` into a struct called `name`.
#[allow(clippy::too_many_arguments)]
pub async fn introduce(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    params: &[String],
    name: &str,
    binding: &str,
    apply: bool,
    force: bool,
) -> Result<ParameterObject> {
    anyhow::ensure!(params.len() >= 2, "bundling one parameter is not a bundle");
    let language = Language::of(file).with_context(|| {
        format!(
            "bundling parameters works in Rust, TypeScript, JavaScript, Python, Go, C, C++, \
             Swift and Java files; \
             {} is none of them",
            file.display()
        )
    })?;
    if matches!(language, Language::C | Language::Cpp) {
        return introduce_c(
            language, remote, root, file, line, col, params, name, binding, apply, force,
        )
        .await;
    }
    if language != Language::Rust {
        return introduce_in(
            language, remote, root, file, line, col, params, name, binding, apply, force,
        )
        .await;
    }
    introduce_rust(
        remote, root, file, line, col, params, name, binding, apply, force,
    )
    .await
}
