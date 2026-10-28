/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail, ensure};
use std::path::{Component, Path};

/// A file hint must name a single Cargo integration-test target, never a wider suite.
pub(crate) fn narrow_rust_test_file(
    command: &mut Vec<String>,
    project: &Path,
    file: &Path,
) -> Result<()> {
    let root = std::fs::canonicalize(project)?;
    let relative = file
        .strip_prefix(project)
        .or_else(|_| file.strip_prefix(&root))
        .context("Rust test file is outside the selected project; no test was run")?;
    let mut cursor = root.clone();
    for part in relative.components() {
        let Component::Normal(part) = part else {
            bail!("Rust test path must not contain traversal; no test was run");
        };
        cursor.push(part);
        if std::fs::symlink_metadata(&cursor)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("Rust test paths containing symlinks cannot be selected safely; no test was run");
        }
    }
    let file = std::fs::canonicalize(file).with_context(|| {
        format!(
            "requested Rust test file {} cannot be read; no test was run",
            file.display()
        )
    })?;
    ensure!(
        file.starts_with(&root),
        "Rust test file is outside the selected project"
    );
    let package_dir = file
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|dir| dir.starts_with(&root))
        .find(|dir| dir.join("Cargo.toml").is_file())
        .context("no Cargo package manifest for the requested test file")?;
    let text = std::fs::read_to_string(package_dir.join("Cargo.toml"))?;
    let manifest: toml::Value =
        toml::from_str(&text).context("cannot parse Cargo package manifest")?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .context("the requested test file is not inside a Cargo package")?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .context("Cargo package has no name")?;
    let auto = package
        .get("autotests")
        .map(|v| v.as_bool().context("invalid Cargo autotests value"))
        .transpose()?
        .unwrap_or(true);
    let mut matches = Vec::new();
    let mut explicit_names = std::collections::BTreeSet::new();
    if let Some(tests) = manifest.get("test") {
        let tests = tests.as_array().context("invalid Cargo test targets")?;
        for target in tests {
            let target = target.as_table().context("invalid Cargo test target")?;
            let target_name = target
                .get("name")
                .and_then(toml::Value::as_str)
                .context("Cargo test target has no name")?;
            explicit_names.insert(target_name.to_string());
            let paths = match target.get("path") {
                Some(path) => {
                    vec![package_dir.join(path.as_str().context("invalid Cargo test target path")?)]
                }
                None => vec![
                    package_dir.join("tests").join(format!("{target_name}.rs")),
                    package_dir.join("tests").join(target_name).join("main.rs"),
                ],
            };
            if paths
                .iter()
                .any(|path| std::fs::canonicalize(path).is_ok_and(|path| path == file))
            {
                matches.push(target_name.to_string());
            }
        }
    }
    let mut automatic_target = None;
    if matches.is_empty() && auto {
        if let Ok(relative) = file.strip_prefix(package_dir) {
            let parts: Vec<_> = relative.components().collect();
            let name = match parts.as_slice() {
                [Component::Normal(tests), Component::Normal(path)] if *tests == "tests" => {
                    Path::new(path)
                        .file_stem()
                        .filter(|_| Path::new(path).extension().is_some_and(|e| e == "rs"))
                }
                [
                    Component::Normal(tests),
                    Component::Normal(dir),
                    Component::Normal(main),
                ] if *tests == "tests" && *main == "main.rs" => Some(*dir),
                _ => None,
            };
            if let Some(name) = name.and_then(|name| name.to_str()) {
                if explicit_names.contains(name) {
                    bail!("Cargo test target {name} is declared for another file; no test was run");
                }
                let flat = package_dir.join("tests").join(format!("{name}.rs"));
                let nested = package_dir.join("tests").join(name).join("main.rs");
                if flat.is_file() && nested.is_file() {
                    bail!("Cargo test target {name} has multiple candidate files; no test was run");
                }
                automatic_target = Some(name.to_string());
            }
        }
    }
    matches.sort();
    matches.dedup();
    matches.sort();
    matches.dedup();
    let target = match matches.as_slice() {
        [target] => Some(target.clone()),
        [] => automatic_target,
        _ => bail!(
            "multiple Cargo test targets select {}; no test was run",
            file.display()
        ),
    };
    let in_tests_dir = file
        .strip_prefix(&package_dir)
        .ok()
        .is_some_and(|relative| {
            relative
                .components()
                .next()
                .is_some_and(|part| part.as_os_str() == "tests")
        });
    if target.is_none() && in_tests_dir {
        bail!(
            "{} is not an enabled Cargo integration-test target; no test was run",
            file.display()
        );
    }
    let workspace = command
        .iter()
        .position(|arg| arg == "--workspace")
        .context("cannot safely narrow this Cargo test command")?;
    let mut narrowed = vec!["-p".to_string(), name.to_string()];
    if let Some(target) = target {
        narrowed.extend(["--test".to_string(), target]);
    }
    command.splice(workspace..=workspace, narrowed);
    Ok(())
}
