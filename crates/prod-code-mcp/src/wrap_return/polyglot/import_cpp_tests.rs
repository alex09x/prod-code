/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::c_cpp_proves_import;

fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("compute.cpp"),
        "#include \"compute.h\"\nint do_work() { return 1; }\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("compute.h"), "int do_work();\n").unwrap();
    tmp
}

#[test]
fn test_c_cpp_proves_import_exact_stem_match() {
    let tmp = fixture();
    let content_unrelated = r#"#include "recompute.h""#;
    assert!(!c_cpp_proves_import(
        content_unrelated,
        &tmp.path().join("caller.cpp"),
        &tmp.path().join("compute.cpp"),
        "do_work",
    ));

    let content_matching = r#"#include "compute.h""#;
    assert!(c_cpp_proves_import(
        content_matching,
        &tmp.path().join("caller.cpp"),
        &tmp.path().join("compute.cpp"),
        "do_work",
    ));
}

#[test]
fn test_c_cpp_proves_import_whitespace_in_include_directive() {
    let tmp = fixture();
    let content = "# include \"compute.h\"\n";
    assert!(c_cpp_proves_import(
        content,
        &tmp.path().join("caller.cpp"),
        &tmp.path().join("compute.cpp"),
        "do_work",
    ));

    // An angle include is searched on include paths, not next to the caller.
    let content_tab = "#\tinclude\t\"compute.h\"\n";
    assert!(c_cpp_proves_import(
        content_tab,
        &tmp.path().join("caller.cpp"),
        &tmp.path().join("compute.cpp"),
        "do_work",
    ));
}
