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

#[test]
fn coverage_w2_transitive_parameters_cross_files_and_continue_downstream() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let initial = root.join("initial.rs");
    let callee = root.join("callee.rs");
    let input = "fn run(seed: u64) {\n    accept(seed);\n}\n";
    let target = "fn accept(value: u32) {\n    let copied: u32 = value;\n    let final_value: u32 = copied;\n}\n";
    std::fs::write(&initial, input).unwrap();
    std::fs::write(&callee, target).unwrap();
    let mut rewritten = BTreeMap::new();
    let mut also = vec![];
    let result = propagate_transitive(
        root,
        &mut rewritten,
        &initial,
        "seed",
        "u32",
        "u64",
        Language::Rust,
        &mut also,
    );
    assert_eq!(
        result,
        (
            3,
            vec![
                "callee.rs:accept(value) (param u32 → u64)".into(),
                "callee.rs:copied (var u32 → u64)".into(),
                "callee.rs:final_value (var u32 → u64)".into(),
            ]
        )
    );
    assert_eq!(rewritten[&callee], target.replace("u32", "u64"));
    assert_eq!(rewritten[&initial], input);
    assert_eq!(also, [callee.clone()]);
    assert_eq!(std::fs::read_to_string(callee).unwrap(), target);
}

#[test]
fn coverage_w2_transitive_cpp_return_synchronizes_header_and_callers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let source = root.join("source.cpp");
    let header = root.join("source.h");
    let caller = root.join("caller.cpp");
    let input = "int build() {\n    int stored = seed;\n    return stored;\n}\n";
    std::fs::write(&source, input).unwrap();
    std::fs::write(&header, "int build();\n").unwrap();
    std::fs::write(&caller, "void run() {\n    int output = build();\n}\n").unwrap();
    let mut rewritten = BTreeMap::new();
    let mut also = vec![];
    let (count, descriptions) = propagate_transitive(
        root,
        &mut rewritten,
        &source,
        "seed",
        "int",
        "long",
        Language::Cpp,
        &mut also,
    );
    assert_eq!(count, 3);
    assert_eq!(
        descriptions,
        [
            "source.cpp:stored (var int → long)",
            "source.cpp:build (return int → long)",
            "caller.cpp:output (caller var int → long)",
        ]
    );
    assert_eq!(
        rewritten[&source],
        "long build() {\n    long stored = seed;\n    return stored;\n}\n"
    );
    assert_eq!(rewritten[&header], "long build();\n");
    assert_eq!(
        rewritten[&caller],
        "void run() {\n    long output = build();\n}\n"
    );
    assert_eq!(also, [header, caller]);
}

#[test]
fn coverage_w2_transitive_uses_overlay_and_skips_missing_initial_file() {
    let dir = tempfile::tempdir().unwrap();
    let initial = dir.path().join("overlay.unknown");
    let mut rewritten = BTreeMap::new();
    let mut also = vec![];
    assert_eq!(
        propagate_transitive(
            dir.path(),
            &mut rewritten,
            &initial,
            "seed",
            "u32",
            "u64",
            Language::Rust,
            &mut also
        ),
        (0, vec![])
    );
    assert!(rewritten.is_empty());
    rewritten.insert(
        initial.clone(),
        "let copied: u32 = seed;\nlet copied_again: u32 = seed;\n".into(),
    );
    let (count, descriptions) = propagate_transitive(
        dir.path(),
        &mut rewritten,
        &initial,
        "seed",
        "u32",
        "u64",
        Language::Rust,
        &mut also,
    );
    assert_eq!(count, 2);
    assert_eq!(
        descriptions,
        [
            "overlay.unknown:copied_again (var u32 → u64)",
            "overlay.unknown:copied (var u32 → u64)"
        ]
    );
    assert_eq!(
        rewritten[&initial],
        "let copied: u64 = seed;\nlet copied_again: u64 = seed;\n"
    );
    assert!(also.is_empty());
}
