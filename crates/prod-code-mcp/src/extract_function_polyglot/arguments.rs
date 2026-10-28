/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::ExtractedParam;

pub(super) fn fresh_literal_parameter_name(
    inputs: &[ExtractedParam],
    param_indices: &[(usize, String, String)],
    ordinal: usize,
    total: usize,
) -> String {
    let base = if total == 1 {
        "value".to_string()
    } else {
        format!("value{ordinal}")
    };
    let mut candidate = base.clone();
    let mut suffix = 1;
    while inputs.iter().any(|input| input.name == candidate)
        || param_indices.iter().any(|(_, name, _)| name == &candidate)
    {
        candidate = format!("{base}_{suffix}");
        suffix += 1;
    }
    candidate
}

pub(super) fn copy_args(
    inputs: &[ExtractedParam],
    param_indices: &[(usize, String, String)],
    differs: &[(usize, String)],
) -> Vec<String> {
    inputs
        .iter()
        .map(|input| {
            let Some((token_idx, _, original_literal)) = param_indices
                .iter()
                .find(|(_, name, _)| name == &input.name)
            else {
                return input.name.clone();
            };
            differs
                .iter()
                .find(|(index, _)| index == token_idx)
                .map(|(_, literal)| literal.clone())
                .unwrap_or_else(|| original_literal.clone())
        })
        .collect()
}
