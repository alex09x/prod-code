/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{filesystem, normalize};
use anyhow::{Context, Result};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// Cargo target identity includes its manifest, kind and name, even for shared root files.
pub(super) struct Target {
    pub file: PathBuf,
    pub name: String,
    pub test: bool,
    pub identity: String,
    pub manifest: PathBuf,
    pub kind: &'static str,
}

fn target(manifest: &Path, file: PathBuf, name: String, kind: &'static str) -> Target {
    Target {
        manifest: manifest.to_path_buf(),
        identity: format!("{}:{kind}:{name}", manifest.display()),
        file,
        name: name.replace('-', "_"),
        test: kind == "test",
        kind,
    }
}

fn dep_paths(table: Option<&toml::Value>, out: &mut Vec<String>) {
    let Some(entries) = table.and_then(|v| v.as_table()) else {
        return;
    };
    for value in entries.values() {
        if let Some(path) = value.get("path").and_then(|p| p.as_str()) {
            out.push(path.to_string());
        }
    }
}

fn manifest_links(doc: &toml::Value) -> (Vec<String>, Vec<String>) {
    let mut globs = Vec::new();
    let mut paths = Vec::new();
    if let Some(ws) = doc.get("workspace") {
        for key in ["members", "default-members"] {
            if let Some(items) = ws.get(key).and_then(|m| m.as_array()) {
                globs.extend(items.iter().filter_map(|m| m.as_str().map(str::to_string)));
            }
        }
        dep_paths(ws.get("dependencies"), &mut paths);
    }
    if let Some(ws) = doc
        .get("package")
        .and_then(|p| p.get("workspace"))
        .and_then(|w| w.as_str())
    {
        paths.push(ws.to_string());
    }
    let dep_keys = [
        "dependencies",
        "dev-dependencies",
        "dev_dependencies",
        "build-dependencies",
        "build_dependencies",
    ];
    for key in dep_keys {
        dep_paths(doc.get(key), &mut paths);
    }
    if let Some(targets) = doc.get("target").and_then(|t| t.as_table()) {
        for cfg in targets.values() {
            for key in dep_keys {
                dep_paths(cfg.get(key), &mut paths);
            }
        }
    }
    (globs, paths)
}

pub(super) fn discover(root: &Path, file: &Path) -> Result<Vec<Target>> {
    let mut manifests = BTreeSet::new();
    let mut pending = Vec::new();
    for dir in file.ancestors().skip(1) {
        if !dir.starts_with(root) {
            break;
        }
        if dir.join("Cargo.toml").is_file() {
            pending.push(normalize(&dir.join("Cargo.toml")));
        }
    }
    if root.join("Cargo.toml").is_file() {
        pending.push(normalize(&root.join("Cargo.toml")));
    }
    while let Some(manifest) = pending.pop() {
        if !manifest.starts_with(root) || !manifest.is_file() {
            continue;
        }
        filesystem::no_links(root, &manifest)?;
        if !manifests.insert(manifest.clone()) {
            continue;
        }
        let dir = manifest.parent().unwrap();
        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&manifest)?)?;
        let (globs, paths) = manifest_links(&doc);
        for pattern in globs {
            for entry in glob::glob(&dir.join(&pattern).to_string_lossy())?.flatten() {
                let candidate = normalize(&entry.join("Cargo.toml"));
                if candidate.starts_with(root) && candidate.is_file() {
                    pending.push(candidate);
                }
            }
        }
        for rel in paths {
            let candidate_dir = dir.join(rel);
            filesystem::no_links(root, &candidate_dir)?;
            let candidate = normalize(&candidate_dir.join("Cargo.toml"));
            if candidate.starts_with(root) && candidate.is_file() {
                pending.push(candidate);
            }
        }
    }
    let mut roots = Vec::new();
    for manifest in manifests {
        filesystem::no_links(root, &manifest)?;
        let dir = manifest.parent().unwrap();
        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&manifest)?)?;
        let Some(package) = doc.get("package") else {
            continue;
        };
        let package_name = package
            .get("name")
            .and_then(|v| v.as_str())
            .context("Cargo package has no name")?;
        if doc.get("lib").is_some()
            || package.get("autolib").and_then(|v| v.as_bool()) != Some(false)
        {
            add(
                &mut roots,
                &manifest,
                doc.get("lib"),
                "src/lib.rs",
                package_name,
                "lib",
            );
        }
        for (kind, sub, auto) in [
            ("bin", "src/bin", "autobins"),
            ("test", "tests", "autotests"),
            ("example", "examples", "autoexamples"),
            ("bench", "benches", "autobenches"),
        ] {
            let explicit = doc.get(kind).and_then(|v| v.as_array());
            if let Some(entries) = explicit {
                for entry in entries {
                    let name = entry
                        .get("name")
                        .and_then(|n| n.as_str())
                        .context("Cargo target has no name")?;
                    let single = format!("{sub}/{name}.rs");
                    let nested = format!("{sub}/{name}/main.rs");
                    let inferred = if kind == "bin"
                        && name == package_name
                        && dir.join("src/main.rs").is_file()
                    {
                        "src/main.rs"
                    } else if dir.join(&single).is_file() {
                        &single
                    } else {
                        &nested
                    };
                    add(&mut roots, &manifest, Some(entry), inferred, name, kind);
                }
            }
            if package.get(auto).and_then(|v| v.as_bool()) != Some(false) {
                if kind == "bin"
                    && !roots
                        .iter()
                        .any(|t| t.kind == kind && t.file == dir.join("src/main.rs"))
                {
                    add(
                        &mut roots,
                        &manifest,
                        None,
                        "src/main.rs",
                        package_name,
                        kind,
                    );
                }
                auto_targets(dir, sub, kind, &mut roots)?;
            }
        }
        // A build script is also a Cargo crate and may own the same physical module.
        if package.get("build").and_then(|v| v.as_bool()) != Some(false) {
            let path = package
                .get("build")
                .and_then(|v| v.as_str())
                .unwrap_or("build.rs");
            add(
                &mut roots,
                &manifest,
                None,
                path,
                "build_script_build",
                "custom-build",
            );
        }
    }
    let mut seen = BTreeSet::new();
    roots.retain(|target| seen.insert(target.identity.clone()));
    let rank = |kind| match kind {
        "lib" => 0,
        "bin" => 1,
        _ => 2,
    };
    roots.sort_by(|a, b| {
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then(
                a.file
                    .components()
                    .count()
                    .cmp(&b.file.components().count()),
            )
            .then(a.file.cmp(&b.file))
            .then(a.identity.cmp(&b.identity))
    });
    anyhow::ensure!(
        !roots.is_empty(),
        "no Cargo target roots found; nothing was written"
    );
    Ok(roots)
}

fn add(
    roots: &mut Vec<Target>,
    manifest: &Path,
    section: Option<&toml::Value>,
    fallback: &str,
    name: &str,
    kind: &'static str,
) {
    let path = section
        .and_then(|s| s.get("path"))
        .and_then(|p| p.as_str())
        .unwrap_or(fallback);
    let path = normalize(&manifest.parent().unwrap().join(path));
    if path.is_file() {
        let name = section
            .and_then(|s| s.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or(name);
        roots.push(target(manifest, path, name.to_string(), kind));
    }
}

fn auto_targets(dir: &Path, sub: &str, kind: &'static str, roots: &mut Vec<Target>) -> Result<()> {
    filesystem::no_links(dir, &dir.join(sub))?;
    let entries = match std::fs::read_dir(dir.join(sub)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e).context("cannot discover all Cargo target roots; nothing was written");
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let file = if path.is_file() && path.extension().is_some_and(|e| e == "rs") {
            path
        } else if path.is_dir() && path.join("main.rs").is_file() {
            path.join("main.rs")
        } else {
            continue;
        };
        if !roots.iter().any(|t| {
            t.kind == kind
                && t.identity
                    .starts_with(&format!("{}:", dir.join("Cargo.toml").display()))
                && (t.file == file
                    || t.identity == format!("{}:{kind}:{name}", dir.join("Cargo.toml").display()))
        }) {
            roots.push(target(&dir.join("Cargo.toml"), file, name, kind));
        }
    }
    Ok(())
}
