/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "dossier_preservation/budget.rs"]
mod budget;
#[path = "dossier_preservation/gateway.rs"]
mod gateway;
#[path = "dossier_preservation/operands.rs"]
mod operands;
#[path = "dossier_preservation/targets.rs"]
mod targets;

// Exercise the existing parser and JSON contracts in the focused acceptance target too.
mod dossier {
    pub use prod_code_mcp::dossier::*;
}
#[path = "../src/dossier/tests/assertions_polyglot.rs"]
mod existing_polyglot;
#[path = "../src/dossier/tests/assertions_rust_node.rs"]
mod existing_rust_node;
#[path = "dossier_preservation/prior_fixes.rs"]
mod prior_fixes;
