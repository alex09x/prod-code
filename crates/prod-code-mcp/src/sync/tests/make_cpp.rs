/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::{engine_project, expected_engine};
use std::path::Path;

#[test]
fn make_uppercase_sources_mcp() {
    for source in [
        "main.C",
        "src/main.CPP",
        "main.HPP",
        "main.c",
        "src/main.cpp",
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let child = root.join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(child.join("Makefile"), "all:\n\ttrue\n").unwrap();
        assert_eq!(expected_engine(&child), None, "no C sources");
        let path = child.join(source);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "int sample(void);\n").unwrap();
        assert_eq!(expected_engine(&child), Some("cpp"), "{source}");
        assert_eq!(
            engine_project(root, Path::new("child")),
            (Some("child".into()), Some("cpp")),
            "nested directory {source}"
        );
        std::fs::write(child.join("go.mod"), "module example.com/app\n").unwrap();
        assert_eq!(expected_engine(&child), Some("go"), "manifest precedence");
    }
}
