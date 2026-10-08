/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use super::c_cpp_proves_import;

#[test]
fn test_c_cpp_proves_import_exact_stem_match() {
    let content_unrelated = r#"#include "recompute.h""#;
    assert!(!c_cpp_proves_import(
        content_unrelated,
        Path::new("src/caller.cpp"),
        Path::new("src/compute.cpp"),
        "do_work",
    ));

    let content_matching = r#"#include "compute.h""#;
    assert!(c_cpp_proves_import(
        content_matching,
        Path::new("src/caller.cpp"),
        Path::new("src/compute.cpp"),
        "do_work",
    ));
}

#[test]
fn test_c_cpp_proves_import_whitespace_in_include_directive() {
    let content = "# include \"compute.h\"\n";
    assert!(c_cpp_proves_import(
        content,
        Path::new("src/caller.cpp"),
        Path::new("src/compute.cpp"),
        "do_work",
    ));

    let content_tab = "#\tinclude <compute.h>\n";
    assert!(c_cpp_proves_import(
        content_tab,
        Path::new("src/caller.cpp"),
        Path::new("src/compute.cpp"),
        "do_work",
    ));
}
