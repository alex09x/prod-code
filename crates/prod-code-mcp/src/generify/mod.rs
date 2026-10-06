/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

mod decl;
mod param;
mod polyglot;
mod rust;
pub mod syntax;
#[cfg(test)]
mod tests;
mod types;

pub use polyglot::generify_polyglot;
pub use rust::generify_rust;
pub use syntax::{generics_span, matching_angle_bracket, split_reference};
pub use types::Generified;

/// Makes the parameter `param` of the function declared at `line`:`col` of `file` generic, as a
/// type parameter `type_param` bounded by `bound`.
#[allow(clippy::too_many_arguments)]
pub async fn generify(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    param: &str,
    bound: &str,
    type_param: &str,
    apply: bool,
    force: bool,
) -> Result<Generified> {
    generify_polyglot(
        remote,
        root,
        file,
        None,
        Some(line),
        Some(col),
        param,
        bound,
        type_param,
        apply,
        force,
    )
    .await
}
