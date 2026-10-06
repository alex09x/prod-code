/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};

pub fn python_manifest_fingerprint(root: &Path) -> Option<String> {
    const MANIFESTS: &[&str] = &[
        "pyproject.toml",
        "uv.lock",
        "poetry.lock",
        "Pipfile.lock",
        "requirements.txt",
        "requirements-dev.txt",
        "requirements.lock",
        "constraints.txt",
        "environment.yml",
        "conda-lock.yml",
        "setup.cfg",
        "setup.py",
    ];
    let mut material = Vec::new();
    for name in MANIFESTS {
        if let Ok(contents) = fs::read(root.join(name)) {
            material.extend_from_slice(name.as_bytes());
            material.push(0);
            material.extend_from_slice(&contents);
            material.push(0xff);
        }
    }
    (!material.is_empty()).then(|| format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&material)))
}

pub fn python_installed_stub_fingerprint(stubs: &[PathBuf]) -> Option<String> {
    let site_packages: std::collections::BTreeSet<PathBuf> = stubs
        .iter()
        .filter_map(|stub| stub.parent().map(Path::to_path_buf))
        .collect();
    let mut distributions = Vec::new();
    for site in site_packages {
        let Ok(entries) = fs::read_dir(site) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            if !name.to_string_lossy().ends_with(".dist-info")
                || !entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            let metadata = entry.path().join("METADATA");
            if let Ok(contents) = fs::read(&metadata) {
                distributions.push((name, contents));
            }
        }
    }
    distributions.sort_by(|a, b| a.0.cmp(&b.0));
    let mut material = Vec::new();
    for (name, contents) in distributions {
        material.extend_from_slice(name.to_string_lossy().as_bytes());
        material.push(0);
        material.extend_from_slice(&contents);
        material.push(0xff);
    }
    (!material.is_empty()).then(|| format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&material)))
}

pub fn python_stub_cache_namespace(
    from: &Path,
    to: &Path,
    from_stubs: &[PathBuf],
    to_stubs: &[PathBuf],
    from_typings: &Path,
    to_typings: &Path,
) -> (String, bool) {
    let from_manifest = python_manifest_fingerprint(from);
    let to_manifest = python_manifest_fingerprint(to);
    let from_installed = python_installed_stub_fingerprint(from_stubs);
    let to_installed = python_installed_stub_fingerprint(to_stubs);
    let manifests_match = match (&from_manifest, &to_manifest) {
        (Some(from), Some(to)) => from == to,
        (Some(_), None) | (None, None) => true,
        (None, Some(_)) => false,
    };
    let installed_match = match (&from_installed, &to_installed) {
        (Some(from), Some(to)) => from == to,
        (Some(_), None) | (None, None) => true,
        (None, Some(_)) => false,
    };
    let compatible = manifests_match && installed_match;
    let mut material = Vec::new();
    for (label, value) in [
        ("manifest", to_manifest.as_ref().or(from_manifest.as_ref())),
        (
            "installed-stubs",
            if compatible {
                from_installed.as_ref().or(to_installed.as_ref())
            } else {
                to_installed.as_ref()
            },
        ),
    ] {
        if let Some(value) = value {
            material.extend_from_slice(label.as_bytes());
            material.push(0);
            material.extend_from_slice(value.as_bytes());
            material.push(0xff);
        }
    }
    for typings in [from_typings, to_typings] {
        if typings.is_dir()
            && !fs::symlink_metadata(typings).is_ok_and(|meta| meta.file_type().is_symlink())
        {
            let identity = typings
                .canonicalize()
                .unwrap_or_else(|_| typings.to_path_buf());
            material.extend_from_slice(b"local-typings\0");
            material.extend_from_slice(identity.to_string_lossy().as_bytes());
            material.push(0xff);
        }
    }
    if material.is_empty() {
        let identity = to.canonicalize().unwrap_or_else(|_| to.to_path_buf());
        material.extend_from_slice(identity.to_string_lossy().as_bytes());
    }
    (
        format!("{:016x}", xxhash_rust::xxh3::xxh3_64(&material)),
        compatible,
    )
}
