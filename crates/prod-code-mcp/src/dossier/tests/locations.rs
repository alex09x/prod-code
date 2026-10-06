/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::dossier::locations::{locations_in, suggested, truncate_utf8};

#[test]
fn finds_locations_inside_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("t.py"), "x\n").unwrap();
    let abs = std::fs::canonicalize(root).unwrap();
    let text = format!(
        "thread 'x' panicked at {}/src/lib.rs:7:9:\n  --> src/lib.rs:3:5\n  File \"{}/t.py\", line 2, in f\n at /usr/lib/x.py:1\n",
        abs.display(),
        abs.display()
    );
    let locs = locations_in(root, &text);
    assert_eq!(
        locs,
        vec![
            ("src/lib.rs".to_string(), 7),
            ("src/lib.rs".to_string(), 3),
            ("t.py".to_string(), 2)
        ]
    );
}

#[test]
fn only_the_fixes_for_errors_are_suggested() {
    let fix = |level: &str, message: &str| crate::fixit::Fix {
        level: level.to_string(),
        code: None,
        message: message.to_string(),
        edits: vec![crate::fixit::Edit {
            file: "src/lib.rs".into(),
            start: 0,
            end: 1,
            line: 4,
            line_text: None,
            replacement: String::new(),
        }],
    };
    assert_eq!(
        suggested(&[
            fix("error", "mismatched types\nconsider borrowing"),
            fix("warning", "unused variable"),
            fix("error", "mismatched types\nconsider borrowing"),
        ]),
        vec!["src/lib.rs:4: mismatched types".to_string()]
    );
}

#[test]
fn truncate_utf8_respects_char_boundaries() {
    let mut s = "привет мир ".repeat(400);
    truncate_utf8(&mut s, 4001);
    assert!(s.ends_with("\n…"));
    assert!(s.len() <= 4001 + "\n…".len());
}
