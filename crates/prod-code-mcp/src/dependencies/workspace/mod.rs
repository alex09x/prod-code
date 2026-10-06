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
use std::collections::BTreeMap;
use std::path::Path;

mod cargo;
mod dotnet;
mod gradle;
pub mod helpers;
mod maven;

pub use helpers::{extract_gradle_project_block, extract_maven_artifact_id, kebab_to_camel};

use super::modules::build_graph_report;
use super::types::DependencyGraphReport;

/// Analyzes dependencies between workspace crates (Rust Cargo, Maven, Gradle, or .NET).
pub fn analyze_crate_dependencies(
    workspace_root: &Path,
    _target_path: Option<&Path>,
) -> Result<DependencyGraphReport> {
    let mut adj = BTreeMap::new();

    // 1. Rust Cargo Workspace
    cargo::collect_cargo_dependencies(workspace_root, &mut adj)?;

    // 2. Maven Multi-Module Workspace (Java)
    maven::collect_maven_dependencies(workspace_root, &mut adj);

    // 3. Gradle Multi-Project Workspace (Java / Kotlin / Android)
    gradle::collect_gradle_dependencies(workspace_root, &mut adj);

    // 4. .NET Multi-Project Workspace (.sln / *.csproj / global.json)
    dotnet::collect_dotnet_dependencies(workspace_root, &mut adj);

    let scope_label = if workspace_root.join("Cargo.toml").exists() {
        "crates"
    } else {
        "modules"
    };
    build_graph_report(scope_label, workspace_root, adj)
}
