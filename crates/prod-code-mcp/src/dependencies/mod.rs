/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod cycles;
mod format;
pub mod languages;
mod modules;
#[cfg(test)]
mod tests;
mod types;
pub mod workspace;

pub use cycles::find_cycles;
pub use format::format_dependency_report;
pub use modules::{analyze_module_dependencies, build_graph_report};
pub use types::{DependencyGraphReport, DependencyNode, DependencyScope, MAX_CYCLES_DETECTED};
pub use workspace::{analyze_crate_dependencies, kebab_to_camel};

pub fn analyze_dependencies(
    workspace_root: &std::path::Path,
    scope: DependencyScope,
    target_path: Option<&std::path::Path>,
) -> anyhow::Result<DependencyGraphReport> {
    match scope {
        DependencyScope::Crates => analyze_crate_dependencies(workspace_root, target_path),
        DependencyScope::Modules => analyze_module_dependencies(workspace_root, target_path),
    }
}
