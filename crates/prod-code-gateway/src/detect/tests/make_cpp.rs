/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::detect::{detect_all_engines, detect_engine};
use prod_code_protocol::messages::EngineKind;

#[test]
fn make_uppercase_sources_gateway() {
    for source in [
        "main.C",
        "src/main.CPP",
        "main.HPP",
        "main.c",
        "src/main.cpp",
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        std::fs::write(root.join("Makefile"), "all:\n\ttrue\n").unwrap();
        assert_eq!(detect_engine(root), EngineKind::Generic, "no C sources");
        let path = root.join(source);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "int sample(void);\n").unwrap();
        assert_eq!(detect_engine(root), EngineKind::Cpp, "{source}");
        assert_eq!(detect_all_engines(root), vec![EngineKind::Cpp], "{source}");
        std::fs::write(root.join("go.mod"), "module example.com/app\n").unwrap();
        assert_eq!(detect_engine(root), EngineKind::Go, "manifest precedence");
    }
}
