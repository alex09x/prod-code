/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::dossier::parse_assertion_evidence;

#[test]
fn duplicate_left_debug_continuation_is_ambiguous() {
    let output = "assertion `left == right` failed\n  left: real value\n  left: injected\n right: expected\n\n";
    let evidence = parse_assertion_evidence(output);
    println!("duplicate-left evidence={evidence:?}");
    assert_eq!(evidence, None);
}

#[test]
fn actual_custom_debug_output_is_ambiguous() {
    #[derive(PartialEq)]
    struct Value(u8);
    impl std::fmt::Debug for Value {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            if self.0 == 1 {
                write!(f, "real value\n  left: injected")
            } else {
                write!(f, "expected")
            }
        }
    }
    // A child process gives the real standard panic-hook output without replacing the
    // process-global hook while other tests run.
    if std::env::var_os("DOSSIER_DEBUG_CHILD").is_some() {
        assert_eq!(Value(1), Value(2));
    }
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "operands::actual_custom_debug_output_is_ambiguous",
            "--nocapture",
        ])
        .env("DOSSIER_DEBUG_CHILD", "1")
        .output()
        .unwrap();
    assert!(!child.status.success());
    let output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    println!("real custom Debug output:\n{output}");
    assert!(output.contains("  left: real value\n  left: injected\n right: expected"));
    assert_eq!(parse_assertion_evidence(&output), None);
}
