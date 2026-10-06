/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::ExecExit;

use super::platform::{layout_only, platform_warning, subdir_of};
use super::types::{RemoteOutcome, TailBuffer};

#[test]
fn files_rewritten_on_another_platform_are_named_when_they_depend_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/pty.rs"),
        "unsafe { libc::openpty(m, s, p, t, &mut w) };\n",
    )
    .unwrap();
    std::fs::write(root.join("src/plain.rs"), "pub fn add(a: u8) -> u8 { a }\n").unwrap();
    let here = prod_code_protocol::platform();
    let other = if cfg!(target_os = "macos") {
        "linux x86_64"
    } else {
        "macos aarch64"
    };
    let both = vec!["src/pty.rs".to_string(), "src/plain.rs".to_string()];
    let plain = vec!["src/plain.rs".to_string()];
    assert_eq!(platform_warning(root, Some(&here), &both), None);
    assert_eq!(platform_warning(root, None, &both), None);
    assert_eq!(platform_warning(root, Some(other), &[]), None);
    assert_eq!(platform_warning(root, Some(other), &plain), None);
    let warning = platform_warning(root, Some(other), &both).expect("a libc call is named");
    assert!(
        warning.contains(&format!("ran on {other} and this machine is {here}")),
        "{warning}"
    );
    assert!(
        warning.contains("1 of them hold code that depends on the platform (src/pty.rs)"),
        "{warning}"
    );
    std::fs::write(root.join("Package.swift"), "// swift-tools-version:5.9\n").unwrap();
    let apple = platform_warning(root, Some(other), &plain).expect("an Apple project is named");
    assert!(apple.contains("also an Apple project"), "{apple}");
}

#[test]
fn a_formatter_s_rewrite_is_layout_and_an_edit_is_not() {
    let old = b"use b::B;\nuse a::A;\nfn f(x: u8,y: u8) { g(&mut win) }\n";
    let formatted = b"use a::A;\nuse b::B;\nfn f(x: u8, y: u8) {\n    g(&mut win,)\n}\n";
    let edited = b"use b::B;\nuse a::A;\nfn f(x: u8,y: u8) { g(&win) }\n";
    assert!(layout_only(old, formatted));
    assert!(!layout_only(old, edited));
    assert!(!layout_only(old, old), "the same text is not a rewrite");
    // rustfmt drops a closure's braces when its body is one expression (#244).
    let braced = b"x.is_some_and(|p| {\n    f(p)\n})\n";
    let unbraced = b"x.is_some_and(|p| f(p))\n";
    assert!(layout_only(braced, unbraced));
    let outcome = RemoteOutcome {
        exit: ExecExit {
            exit_code: Some(0),
            duration_ms: 1,
            server_workspace_root: String::new(),
            timed_out: false,
            error: None,
            usage: None,
            platform: None,
        },
        pulled_files: vec!["a.rs".into(), "b.rs".into()],
        relaid_files: vec!["a.rs".into()],
        kept_files: Vec::new(),
    };
    assert_eq!(outcome.changed_code(), vec!["b.rs".to_string()]);
}

#[test]
fn tail_buffer_keeps_only_the_end() {
    let mut tail = TailBuffer::new(8);
    tail.push(b"0123456789");
    tail.push(b"ab");
    assert_eq!(tail.text(), "456789ab");
    assert_eq!(tail.total, 12);
}

#[test]
fn tail_buffer_keeps_everything_under_the_limit() {
    let mut tail = TailBuffer::new(8);
    tail.push(b"ab");
    assert_eq!(tail.text(), "ab");
    assert_eq!(tail.total, 2);
}

#[test]
fn subdir_of_is_none_for_the_root_itself() {
    let root = std::env::temp_dir();
    assert_eq!(subdir_of(&root, &root), None);
}

#[test]
fn subdir_of_is_the_relative_slash_separated_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let nested = root.join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(subdir_of(&root, &nested).as_deref(), Some("a/b"));
}

#[test]
fn subdir_of_is_none_outside_the_root() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(a.path()).unwrap();
    let outside = std::fs::canonicalize(b.path()).unwrap();
    assert_eq!(subdir_of(&root, &outside), None);
}
