/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::impact::Symbol;

#[test]
fn same_language_nested_project_is_distinct_but_cargo_member_is_not() {
    let python = tempfile::tempdir().unwrap();
    std::fs::write(
        python.path().join("pyproject.toml"),
        "[project]\nname='root'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::create_dir_all(python.path().join("component/tests")).unwrap();
    std::fs::write(
        python.path().join("component/pyproject.toml"),
        "[project]\nname='component'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::write(
        python.path().join("component/tests/test_child.py"),
        "def test_child(): pass\n",
    )
    .unwrap();
    let tests = vec![Symbol {
        name: "test_child".into(),
        file: "component/tests/test_child.py".into(),
        line: 1,
        col: 1,
    }];
    let tools = crate::verify::detect_tools(python.path());
    let plan = crate::impact::test_plan::build_test_plan(
        python.path(),
        "python",
        &tools,
        &tests,
        &["component/pyproject.toml".into()],
    );
    let script = plan.selected.unwrap().join(" ");
    assert!(script.contains("(cd component &&"), "{script}");

    let cargo = tempfile::tempdir().unwrap();
    std::fs::write(
        cargo.path().join("Cargo.toml"),
        "[workspace]\nmembers=['member']\nresolver='2'\n",
    )
    .unwrap();
    std::fs::create_dir_all(cargo.path().join("member/src")).unwrap();
    std::fs::write(
        cargo.path().join("member/Cargo.toml"),
        "[package]\nname='member'\nversion='0.1.0'\n",
    )
    .unwrap();
    std::fs::write(
        cargo.path().join("member/src/lib.rs"),
        "#[test] fn unit() {}\n",
    )
    .unwrap();
    let tests = vec![Symbol {
        name: "unit".into(),
        file: "member/src/lib.rs".into(),
        line: 1,
        col: 12,
    }];
    let tools = crate::verify::detect_tools(cargo.path());
    let plan = crate::impact::test_plan::build_test_plan(
        cargo.path(),
        "rust",
        &tools,
        &tests,
        &["member/Cargo.toml".into()],
    );
    assert!(plan.whole_is_root_only);
    assert_eq!(plan.selected.unwrap()[0], "cargo");
}
