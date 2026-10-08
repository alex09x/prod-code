/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::engine::cgo::*;
use crate::sync::engine::project::*;
use std::path::Path;

#[test]
fn a_new_python_file_is_routed_even_when_its_parent_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let relative = Path::new("spikes/python-overlay-ab/replay.py");
    let expected = (Some("spikes/python-overlay-ab".to_string()), Some("python"));
    assert_eq!(engine_project(root, relative), expected);
    assert_eq!(engine_project(root, &root.join(relative)), expected);
    assert!(!root.join(relative).exists());
    assert!(!root.join("spikes").exists());
}
/// Escapes and symlink parents must not be mistaken for proposed checkout sources.
#[test]
fn missing_python_paths_outside_the_checkout_are_not_routed_as_loose_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let root_engine = (None, Some("rust"));

    assert_eq!(
        engine_project(root, Path::new("../outside.py")),
        root_engine
    );
    assert_eq!(
        engine_project(root, &root.join("../outside.py")),
        root_engine
    );

    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        let link = root.join("linked");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        assert_eq!(engine_project(root, &link.join("proposed.py")), root_engine);
    }

    std::fs::create_dir(root.join("directory.py")).unwrap();
    assert_eq!(
        engine_project(root, &root.join("directory.py")),
        root_engine
    );
}

/// A loose file of another language than the checkout's goes to its own language's engine,
/// rooted at its directory; a file of the root's language, or one at the root, stays (#247).
#[test]
fn a_loose_file_of_another_language_is_served_by_its_own_engine() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"x\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("scripts/tools")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "").unwrap();
    std::fs::write(root.join("scripts/tools/cover.py"), "def f(): pass\n").unwrap();
    std::fs::write(root.join("setup.py"), "").unwrap();
    std::fs::write(root.join("scripts/notes.md"), "").unwrap();
    assert_eq!(
        engine_project(root, &root.join("scripts/tools/cover.py")),
        (Some("scripts/tools".to_string()), Some("python"))
    );
    assert_eq!(
        engine_project(root, &root.join("src/lib.rs")),
        (None, Some("rust"))
    );
    assert_eq!(
        engine_project(root, &root.join("setup.py")),
        (None, Some("rust"))
    );
    assert_eq!(
        engine_project(root, &root.join("scripts/notes.md")),
        (None, Some("rust"))
    );
    for relative in ["scripts/tools/cover.py", "src/lib.rs", "setup.py"] {
        assert_eq!(
            engine_project(root, Path::new(relative)),
            engine_project(root, &root.join(relative)),
            "relative and absolute hints must select the same project: {relative}"
        );
    }
    assert_eq!(engine_for_file(Path::new("a.TSX")), Some("typescript"));
    assert_eq!(engine_for_file(Path::new("a.hpp")), Some("cpp"));
    assert_eq!(engine_for_file(Path::new("bridge.mm")), Some("cpp"));
    assert_eq!(engine_for_file(Path::new("Makefile")), None);
}

/// A dependency manifest in `.build/checkouts/...` is ignored so the dependency's files
/// stay with the enclosing project rather than spawning an independent broken workspace (#764).
#[test]
fn dependency_directories_are_not_treated_as_nested_projects() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("go.mod"), "module example.com/app\n\ngo 1.22\n").unwrap();
    let macos_app = root.join("clients/macos/ProdUI");
    std::fs::create_dir_all(&macos_app).unwrap();
    std::fs::write(
        macos_app.join("Package.swift"),
        "// swift-tools-version:5.9\n",
    )
    .unwrap();
    let dep_file =
        macos_app.join(".build/checkouts/tako/swift/Sources/TakoCoreUI/TakoTerminalNSView.swift");
    std::fs::create_dir_all(dep_file.parent().unwrap()).unwrap();
    std::fs::write(
        macos_app.join(".build/checkouts/tako/swift/Package.swift"),
        "// swift-tools-version:5.9\n",
    )
    .unwrap();
    std::fs::write(&dep_file, "public class TakoTerminalNSView {}\n").unwrap();

    assert_eq!(
        engine_project(root, &dep_file),
        (Some("clients/macos/ProdUI".to_string()), Some("swift"))
    );
    assert!(other_checkout(root, &dep_file).is_none());
}

/// A file of another language inside a nested project is a loose file of its own language,
/// not a source of that project's server (#362): a Python script in a Swift package went to
/// sourcekit-lsp. The C family of a Swift package's C targets stays with the package, and a
/// file of the root's language with the root.
#[test]
fn a_file_of_another_language_inside_a_nested_project_is_served_by_its_own_engine() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for (rel, text) in [
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        ("pkg/Package.swift", "// swift-tools-version:5.9\n"),
        ("pkg/Sources/App/main.swift", ""),
        ("pkg/Sources/CShim/shim.h", ""),
        ("pkg/scripts/e2e/pack.py", "def f(): pass\n"),
        ("pkg/tools/gen.go", "package tools\n"),
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    assert_eq!(
        engine_project(root, &root.join("pkg/scripts/e2e/pack.py")),
        (Some("pkg/scripts/e2e".to_string()), Some("python"))
    );
    assert_eq!(
        engine_project(root, &root.join("pkg/Sources/App/main.swift")),
        (Some("pkg".to_string()), Some("swift"))
    );
    assert_eq!(
        engine_project(root, &root.join("pkg/Sources/CShim/shim.h")),
        (Some("pkg".to_string()), Some("swift"))
    );
    assert_eq!(
        engine_project(root, &root.join("pkg/tools/gen.go")),
        (None, Some("go"))
    );
}

/// A Go module with one file, `name`, holding `source`.
fn go_module(name: &str, source: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("go.mod"), "module m\n\ngo 1.22\n").unwrap();
    let path = temp.path().join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, source).unwrap();
    temp
}

const LIBPROC: &str = "package proc\n\n// #include <libproc.h>\nimport \"C\"\n";

/// cgo that includes a macOS-only header needs macOS, unless Linux skips the file by its
/// build line or its name; cgo with a portable header, a guarded include, a framework linked
/// for darwin only, or no cgo at all builds anywhere (#248).
#[test]
fn macos_only_cgo_is_found_unless_linux_skips_the_file() {
    let module = go_module("internal/proc/proc.go", LIBPROC);
    assert_eq!(
        macos_only_cgo(module.path()),
        Some(("internal/proc/proc.go".to_string(), "libproc.h".to_string()))
    );
    let block = "package ioreg\n\n/*\n#cgo LDFLAGS: -framework IOKit\n#include <stdlib.h>\n*/\nimport \"C\"\n";
    assert_eq!(
        macos_only_cgo(go_module("ioreg.go", block).path()),
        Some(("ioreg.go".to_string(), "-framework IOKit".to_string()))
    );
    let either = format!("//go:build linux || darwin\n\n{LIBPROC}");
    assert!(macos_only_cgo(go_module("proc.go", &either).path()).is_some());

    let darwin = format!("//go:build darwin\n\n{LIBPROC}");
    assert_eq!(macos_only_cgo(go_module("proc.go", &darwin).path()), None);
    assert_eq!(
        macos_only_cgo(go_module("proc_darwin.go", LIBPROC).path()),
        None
    );
    assert_eq!(
        macos_only_cgo(go_module("proc_darwin_arm64.go", LIBPROC).path()),
        None
    );
    let stdlib = "package c\n\n// #include <stdlib.h>\nimport \"C\"\n";
    assert_eq!(macos_only_cgo(go_module("c.go", stdlib).path()), None);
    let guarded =
        "package c\n\n// #ifdef __APPLE__\n// #include <libproc.h>\n// #endif\nimport \"C\"\n";
    assert_eq!(macos_only_cgo(go_module("c.go", guarded).path()), None);
    let darwin_flags =
        "package c\n\n// #cgo darwin LDFLAGS: -framework CoreFoundation\nimport \"C\"\n";
    assert_eq!(macos_only_cgo(go_module("c.go", darwin_flags).path()), None);
    let plain = "package main\n\n// libproc.h is not used here.\nfunc main() {}\n";
    assert_eq!(macos_only_cgo(go_module("main.go", plain).path()), None);
    assert_eq!(
        macos_only_cgo(go_module("vendor/x/proc.go", LIBPROC).path()),
        None
    );
    assert_eq!(
        macos_only_cgo(go_module("target/debug/proc.go", LIBPROC).path()),
        None
    );
    assert_eq!(
        macos_only_cgo(go_module("node_modules/pkg/proc.go", LIBPROC).path()),
        None
    );
}

#[test]

fn nested_project_of_another_language_gets_a_subpath() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    std::fs::create_dir_all(root.join("crates/a/src")).unwrap();
    std::fs::write(root.join("crates/a/Cargo.toml"), "[package]\n").unwrap();
    std::fs::write(root.join("crates/a/src/lib.rs"), "").unwrap();
    std::fs::create_dir_all(root.join("swift/Sources/App")).unwrap();
    std::fs::write(root.join("swift/Package.swift"), "").unwrap();
    std::fs::write(root.join("swift/Sources/App/main.swift"), "").unwrap();
    assert_eq!(
        engine_project(root, &root.join("crates/a/src/lib.rs")),
        (None, Some("rust"))
    );
    assert_eq!(
        engine_project(root, &root.join("swift/Sources/App/main.swift")),
        (Some("swift".to_string()), Some("swift"))
    );
    assert_eq!(engine_project(root, root), (None, Some("rust")));
}

/// What git lists is mirrored whatever its extension, `vendor/` included (#313); data
/// trees, build output and `.git` are not (#123).
#[test]

fn engine_is_found_one_directory_down() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("project")).unwrap();
    std::fs::write(root.path().join("project/go.mod"), "module x\n").unwrap();
    assert_eq!(expected_engine(root.path()), Some("go"));

    // Two children asking for different engines is not one answer.
    std::fs::create_dir_all(root.path().join("server")).unwrap();
    std::fs::write(root.path().join("server/Cargo.toml"), "[package]\n").unwrap();
    assert_eq!(expected_engine(root.path()), None);

    // A manifest at the root still wins outright.
    std::fs::write(root.path().join("Cargo.toml"), "[package]\n").unwrap();
    assert_eq!(expected_engine(root.path()), Some("rust"));
}

#[test]
fn test_expected_engine_follows_manifest() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(expected_engine(temp.path()), None);
    std::fs::write(temp.path().join("tsconfig.json"), "{}").unwrap();
    assert_eq!(expected_engine(temp.path()), Some("typescript"));
    std::fs::write(temp.path().join("requirements.txt"), "").unwrap();
    assert_eq!(expected_engine(temp.path()), Some("python"));
    std::fs::write(temp.path().join("CMakeLists.txt"), "").unwrap();
    assert_eq!(expected_engine(temp.path()), Some("cpp"));
    std::fs::write(temp.path().join("Package.swift"), "").unwrap();
    assert_eq!(expected_engine(temp.path()), Some("swift"));
    std::fs::write(temp.path().join("go.mod"), "module x\n").unwrap();
    assert_eq!(expected_engine(temp.path()), Some("go"));
    std::fs::write(temp.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    assert_eq!(expected_engine(temp.path()), Some("rust"));
}

#[test]
fn test_starlark_does_not_misclassify_workspace_or_build_directories() {
    let temp = tempfile::tempdir().unwrap();
    // A directory named "workspace" or "build" must NOT trigger Starlark detection
    std::fs::create_dir_all(temp.path().join("workspace")).unwrap();
    std::fs::create_dir_all(temp.path().join("build")).unwrap();
    assert_eq!(expected_engine(temp.path()), None);

    // A lowercase file named "workspace" or "build" must NOT trigger Starlark detection
    let temp2 = tempfile::tempdir().unwrap();
    std::fs::write(temp2.path().join("workspace"), "# script\n").unwrap();
    std::fs::write(temp2.path().join("build"), "# script\n").unwrap();
    assert_eq!(expected_engine(temp2.path()), None);

    // An uppercase file named "WORKSPACE" or "BUILD" DOES trigger Starlark detection
    std::fs::write(temp2.path().join("WORKSPACE"), "").unwrap();
    assert_eq!(expected_engine(temp2.path()), Some("starlark"));
}
