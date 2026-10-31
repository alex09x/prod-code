/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::RustEngine;

#[test]
fn editor_rust_errors_link_to_the_official_explanation() {
    let (_temp, path) = super::create_test_fixture();
    std::fs::write(
        &path,
        "pub fn f(value: Option<u32>) { let _: u32 = \"not a number\"; let _ = value.missing; }\n",
    )
    .unwrap();
    let engine = RustEngine::load(path.parent().unwrap().parent().unwrap()).unwrap();
    let diagnostics = engine.editor_diagnostics(&path).unwrap();
    let error = diagnostics
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "E0308")
        .unwrap_or_else(|| panic!("expected a type mismatch: {diagnostics}"));
    assert_eq!(
        error["codeDescription"]["href"],
        "https://doc.rust-lang.org/error_codes/E0308.html"
    );
    let missing_field = diagnostics
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|text| text.contains("no field `missing`"))
        })
        .unwrap_or_else(|| panic!("expected a missing field: {diagnostics}"));
    assert_eq!(missing_field["code"], "E0609");
    assert_eq!(
        missing_field["codeDescription"]["href"],
        "https://doc.rust-lang.org/error_codes/E0609.html"
    );
    for diagnostic in diagnostics.as_array().unwrap() {
        if diagnostic["code"]
            .as_str()
            .is_some_and(|code| !code.starts_with('E'))
        {
            assert!(diagnostic.get("codeDescription").is_none());
        }
    }
}
