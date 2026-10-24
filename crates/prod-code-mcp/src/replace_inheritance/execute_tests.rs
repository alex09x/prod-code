/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[tokio::test]
async fn coverage_b2_inheritance_refusals_leave_source_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("classes.py");
    let remote = "127.0.0.1:1".parse().unwrap();
    for (text, sub, base, expected) in [
        ("class Plain:\n    pass\n", "Plain", None, "class 'Plain' does not declare any base class"),
        ("class Multi(Left, Right):\n    pass\n", "Multi", None, "class 'Multi' inherits from multiple classes [\"Left\", \"Right\"]; please specify 'base_type'"),
        ("class Child(Parent):\n    pass\n", "Child", Some("Other"), "class 'Child' does not inherit from 'Other' (known base classes: [\"Parent\"])"),
    ] {
        std::fs::write(&path, text).unwrap();
        let error = replace_inheritance_impl(remote, dir.path(), &path, sub, base, None, None, true, false, None).await.unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
    let error = replace_inheritance_impl(remote, dir.path(), &path, "Absent", None, None, None, false, false, None).await.unwrap_err();
    assert_eq!(error.to_string(), format!("class 'Absent' not found in {}", path.display()));
}
