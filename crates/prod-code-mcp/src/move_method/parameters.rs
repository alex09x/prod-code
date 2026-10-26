/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

/// Locate the selected top-level parameter using the same split as declaration
/// parsing. Consume each complete parameter from the front, so matching text in
/// a preceding function-pointer type cannot supply either semantic cursor.
pub(super) fn selected_cursors(
    list: &str,
    index: usize,
    binding: &str,
    type_name: &str,
) -> Result<(usize, usize)> {
    let params = crate::signature::split_params(list);
    let mut at = 0;
    for (i, param) in params.iter().enumerate() {
        at += list[at..].len() - list[at..].trim_start().len();
        anyhow::ensure!(
            list[at..].starts_with(param),
            "parameter span does not match its declaration"
        );
        if i == index {
            let (pattern, ty) = param.split_once(':').context("the parameter has no type")?;
            let binding_at = at
                + super::syntax::binding_offset(pattern, binding)
                    .context("the binding is not in the selected parameter")?;
            let type_at = at
                + pattern.len()
                + 1
                + ty.find(type_name)
                    .context("the type is not in the selected parameter")?;
            return Ok((binding_at, type_at));
        }
        at += param.len();
        at += list[at..].len() - list[at..].trim_start().len();
        anyhow::ensure!(list[at..].starts_with(','), "missing parameter separator");
        at += 1;
    }
    anyhow::bail!("the selected parameter is not in the list")
}
