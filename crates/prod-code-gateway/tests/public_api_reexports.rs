/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

// This integration test compiles as a downstream crate and preserves public module paths.
use prod_code_gateway::shadow::run_in_place;
use prod_code_gateway::workspace::extract_workspace_identifier;

#[test]
fn public_helpers_keep_legacy_import_paths() {
    let _extract: fn(&str) -> String = extract_workspace_identifier;
    let _run_in_place = run_in_place;
}
