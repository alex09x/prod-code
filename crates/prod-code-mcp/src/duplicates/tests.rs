/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use tempfile::tempdir;

use super::detect::find_duplicates;
use super::types::DuplicateOptions;

#[test]
fn test_duplicate_detection_simple() {
    let dir = tempdir().unwrap();
    let file_a = dir.path().join("a.rs");
    let file_b = dir.path().join("b.rs");

    let code = r#"
fn process_items(items: &[String]) {
    for item in items {
        let trimmed = item.trim();
        if !trimmed.is_empty() {
            println!("Item: {}", trimmed);
        }
    }
}
"#;

    std::fs::write(&file_a, code).unwrap();
    std::fs::write(&file_b, code).unwrap();

    let report = find_duplicates(
        dir.path(),
        None,
        DuplicateOptions {
            min_lines: 5,
            parameterized: true,
            type3: false,
            max_groups: 10,
        },
    )
    .unwrap();

    assert_eq!(report.total_files_scanned, 2);
    assert!(!report.groups.is_empty());
    assert_eq!(report.groups[0].occurrences.len(), 2);
}

#[test]
fn test_duplicate_detection_type3_reordered_and_gapped() {
    let dir = tempdir().unwrap();
    let file_a = dir.path().join("a.rs");
    let file_b = dir.path().join("b.rs");

    let code_a = r#"
fn func_a() {
    let alpha = 10;
    let beta = 20;
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = 60;
}
"#;
    let code_b = r#"
fn func_b() {
    let beta = 20;
    let alpha = 10;
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = 60;
}
"#;

    std::fs::write(&file_a, code_a).unwrap();
    std::fs::write(&file_b, code_b).unwrap();

    let report_no_type3 = find_duplicates(
        dir.path(),
        None,
        DuplicateOptions {
            min_lines: 6,
            parameterized: true,
            type3: false,
            max_groups: 10,
        },
    )
    .unwrap();
    assert_eq!(report_no_type3.groups.len(), 0);

    let report_type3 = find_duplicates(
        dir.path(),
        None,
        DuplicateOptions {
            min_lines: 6,
            parameterized: true,
            type3: true,
            max_groups: 10,
        },
    )
    .unwrap();
    assert_eq!(report_type3.groups.len(), 1);
    assert_eq!(
        report_type3.groups[0].clone_type,
        "Type-3 (Gapped/Reordered)"
    );
    assert_eq!(report_type3.groups[0].occurrences.len(), 2);
}

#[test]
fn test_duplicate_detection_type3_gapped() {
    let dir = tempdir().unwrap();
    let file_a = dir.path().join("a.rs");
    let file_c = dir.path().join("c.rs");

    let code_a = r#"
fn func_a() {
    let alpha = 10;
    let beta = 20;
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = 60;
}
"#;
    let code_c = r#"
fn func_c() {
    let alpha = 10;
    let beta = 20;
    println!("debugging step");
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = 60;
}
"#;

    std::fs::write(&file_a, code_a).unwrap();
    std::fs::write(&file_c, code_c).unwrap();

    let report_type3 = find_duplicates(
        dir.path(),
        None,
        DuplicateOptions {
            min_lines: 6,
            parameterized: true,
            type3: true,
            max_groups: 10,
        },
    )
    .unwrap();
    assert_eq!(report_type3.groups.len(), 1);
    assert_eq!(
        report_type3.groups[0].clone_type,
        "Type-3 (Gapped/Reordered)"
    );
    assert_eq!(report_type3.groups[0].occurrences.len(), 2);
}

#[test]
fn test_duplicate_detection_type3_large_bucket_uniform_sampling() {
    let dir = tempdir().unwrap();
    for i in 0..20 {
        let code = format!(
            r#"
fn func_{}() {{
    let alpha = 10;
    let beta = 20;
    let gamma = 30;
    let delta = 40;
    let epsilon = 50;
    let zeta = {};
}}
"#,
            i,
            if i % 2 == 0 { 60 } else { 70 }
        );
        std::fs::write(dir.path().join(format!("f{}.rs", i)), code).unwrap();
    }

    let report = find_duplicates(
        dir.path(),
        None,
        DuplicateOptions {
            min_lines: 6,
            parameterized: true,
            type3: true,
            max_groups: 10,
        },
    )
    .unwrap();

    assert_eq!(report.total_files_scanned, 20);
    assert!(!report.groups.is_empty());
    assert!(report.groups[0].occurrences.len() >= 2);
}
