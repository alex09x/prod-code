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
async fn coverage_w2_inheritance_missing_source_and_unsupported_language() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("absent.py");
    let remote = "127.0.0.1:1".parse().unwrap();
    let error = replace_inheritance_impl(
        remote,
        dir.path(),
        &missing,
        "Child",
        None,
        None,
        None,
        false,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("cannot read file {}", missing.display())
    );
    assert!(!missing.exists());
    let path = dir.path().join("traits.rs");
    let text = "trait Parent {}\ntrait Child: Parent {}\n";
    std::fs::write(&path, text).unwrap();
    let error = replace_inheritance_impl(
        remote,
        dir.path(),
        &path,
        "Child",
        Some("Parent"),
        Some("inner"),
        Some(&["run".into()]),
        true,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "replace_inheritance_with_delegation is not supported for language 'rust'"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
}

#[tokio::test]
async fn coverage_b2_inheritance_refusals_leave_source_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("classes.py");
    let remote = "127.0.0.1:1".parse().unwrap();
    for (text, sub, base, expected) in [
        (
            "class Plain:\n    pass\n",
            "Plain",
            None,
            "class 'Plain' does not declare any base class",
        ),
        (
            "class Multi(Left, Right):\n    pass\n",
            "Multi",
            None,
            "class 'Multi' inherits from multiple classes [\"Left\", \"Right\"]; please specify 'base_type'",
        ),
        (
            "class Child(Parent):\n    pass\n",
            "Child",
            Some("Other"),
            "class 'Child' does not inherit from 'Other' (known base classes: [\"Parent\"])",
        ),
    ] {
        std::fs::write(&path, text).unwrap();
        let error = replace_inheritance_impl(
            remote,
            dir.path(),
            &path,
            sub,
            base,
            None,
            None,
            true,
            false,
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }
    let error = replace_inheritance_impl(
        remote,
        dir.path(),
        &path,
        "Absent",
        None,
        None,
        None,
        false,
        false,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("class 'Absent' not found in {}", path.display())
    );
}
